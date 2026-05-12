use anchor_lang::prelude::*;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{ChainAccount, CompoundRecord, StakeTracker, ValidatorRegistry},
    yield_utils,
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct CompoundRewardsArgs {
    pub amount: u64,
    pub chain_id: u8,
    pub compound_index: u16,
}

#[derive(Accounts)]
#[instruction(args: CompoundRewardsArgs)]
pub struct CompoundRewards<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(
        seeds = [VALIDATOR_REGISTRY_SEED],
        bump = validator_registry.bump,
    )]
    pub validator_registry: Account<'info, ValidatorRegistry>,

    #[account(
        mut,
        seeds = [CHAIN_SEED, &[args.chain_id]],
        bump = chain_account.bump,
    )]
    pub chain_account: Account<'info, ChainAccount>,

    #[account(
        init,
        payer = caller,
        space = CompoundRecord::SPACE,
        seeds = [
            COMPOUND_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &args.compound_index.to_le_bytes(),
        ],
        bump,
    )]
    pub compound_record: Account<'info, CompoundRecord>,

    #[account(
        init,
        payer = caller,
        space = StakeTracker::SPACE,
        seeds = [
            STAKE_TRACKER_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &[STAKE_SOURCE_TYPE_COMPOUND],
            &(args.compound_index as u32).to_le_bytes(),
        ],
        bump,
    )]
    pub stake_tracker: Account<'info, StakeTracker>,

    /// Client-generated stake account keypair - must be a signer.
    #[account(mut)]
    pub stake_account: Signer<'info>,

    /// CHECK: chain_authority PDA - validated in handler.
    pub chain_authority: UncheckedAccount<'info>,

    /// CHECK: validator vote account - validated against ValidatorRegistry.
    pub validator_vote: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: rent sysvar - used for stake creation and liquid reward accounting.
    pub rent: UncheckedAccount<'info>,

    /// CHECK: stake_history sysvar - key checked by stake_utils.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: stake_config sysvar - key checked by stake_utils.
    pub stake_config: UncheckedAccount<'info>,

    /// CHECK: native stake program - key checked by stake_utils.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CompoundRewards>, args: CompoundRewardsArgs) -> Result<()> {
    require!(args.amount > 0, ChainStakingError::InvalidCompoundAmount);

    require!(
        ctx.accounts
            .validator_registry
            .validator_list
            .contains(ctx.accounts.validator_vote.key),
        ChainStakingError::InvalidValidator
    );

    let (expected_chain_authority, _) = find_chain_authority_pda(ctx.program_id, args.chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    let rent = Rent::from_account_info(&ctx.accounts.rent.to_account_info())?;
    let available_liquid_rewards = yield_utils::compute_available_liquid_rewards_lamports(
        &ctx.accounts.chain_account.to_account_info(),
        &rent,
        ChainAccount::SPACE,
    )?;
    require!(
        args.amount <= available_liquid_rewards,
        ChainStakingError::InsufficientLiquidRewards
    );

    let chain_id = args.chain_id;
    let round = ctx.accounts.chain_account.current_round;
    let chain_authority_bump = ctx.accounts.chain_account.chain_authority_bump;
    let source_index = u32::from(args.compound_index);

    // ── FIX (C-03): Transfer compound lamports from chain_account PDA directly
    // into the stake_account. The caller creates the stake account (paying rent)
    // and then the chain_account PDA transfers the compound principal into it.
    // This keeps total lamports conserved within the instruction boundary
    // because both accounts are present in the transaction. ──

    // Step 1: Caller creates a stake account, funded with the compound amount.
    stake_utils::create_stake_account(
        &ctx.accounts.caller.to_account_info(),
        &ctx.accounts.stake_account.to_account_info(),
        &ctx.accounts.rent.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        args.amount,
    )?;

    // Step 2: Reimburse the caller from chain_account PDA's excess lamports.
    // The caller paid `args.amount` to create the stake account above.
    // We now move that same amount from chain_account → caller, so the net
    // effect is: chain_account loses `amount`, stake_account gains `amount`,
    // caller is unchanged. All accounts are present → balance is conserved.
    transfer_program_lamports(
        &ctx.accounts.chain_account.to_account_info(),
        &ctx.accounts.caller.to_account_info(),
        args.amount,
    )?;

    stake_utils::initialize_stake_account(
        &ctx.accounts.stake_account.to_account_info(),
        &ctx.accounts.chain_authority.to_account_info(),
        &ctx.accounts.rent.to_account_info(),
        &ctx.accounts.stake_program.to_account_info(),
    )?;

    stake_utils::delegate_stake_account(
        &ctx.accounts.stake_account.to_account_info(),
        &ctx.accounts.validator_vote.to_account_info(),
        &ctx.accounts.chain_authority.to_account_info(),
        &ctx.accounts.clock.to_account_info(),
        &ctx.accounts.stake_history.to_account_info(),
        &ctx.accounts.stake_config.to_account_info(),
        &ctx.accounts.stake_program.to_account_info(),
        chain_id,
        chain_authority_bump,
    )?;

    ctx.accounts.compound_record.set_inner(CompoundRecord {
        chain_id,
        round,
        compound_index: args.compound_index,
        amount_lamports: args.amount,
        stake_account: ctx.accounts.stake_account.key(),
        validator: *ctx.accounts.validator_vote.key,
        bump: ctx.bumps.compound_record,
    });

    ctx.accounts.stake_tracker.set_inner(StakeTracker {
        chain_id,
        round,
        stake_account: ctx.accounts.stake_account.key(),
        principal_lamports: args.amount,
        source_type: STAKE_SOURCE_TYPE_COMPOUND,
        source_index,
        is_active: true,
        is_deactivated: false,
        is_withdrawn: false,
        bump: ctx.bumps.stake_tracker,
    });

    // ── FIX (M-07): Track compound amount in total_staked_lamports ──
    ctx.accounts.chain_account.total_staked_lamports = ctx
        .accounts
        .chain_account
        .total_staked_lamports
        .checked_add(args.amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    Ok(())
}

fn transfer_program_lamports(
    source: &AccountInfo<'_>,
    destination: &AccountInfo<'_>,
    amount: u64,
) -> Result<()> {
    let updated_source_lamports = source
        .lamports()
        .checked_sub(amount)
        .ok_or(ChainStakingError::InsufficientLiquidRewards)?;
    let updated_destination_lamports = destination
        .lamports()
        .checked_add(amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    **source.try_borrow_mut_lamports()? = updated_source_lamports;
    **destination.try_borrow_mut_lamports()? = updated_destination_lamports;

    Ok(())
}
