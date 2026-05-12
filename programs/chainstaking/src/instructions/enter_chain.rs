use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{
        ChainAccount, EntryAccount, GlobalConfig, StakeTracker, TriggerVault, ValidatorRegistry,
    },
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct EnterChainArgs {
    pub amount: u64,
    pub chain_id: u8,
}

#[derive(Accounts)]
#[instruction(args: EnterChainArgs)]
pub struct EnterChain<'info> {
    #[account(mut)]
    pub staker: Signer<'info>,

    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
    )]
    pub global_config: Account<'info, GlobalConfig>,

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
        mut,
        seeds = [TRIGGER_VAULT_SEED, &[args.chain_id]],
        bump = trigger_vault.bump,
    )]
    pub trigger_vault: Account<'info, TriggerVault>,

    #[account(
        init,
        payer = staker,
        space = EntryAccount::SPACE,
        seeds = [
            ENTRY_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &chain_account.entry_count.to_le_bytes(),
        ],
        bump,
    )]
    pub entry_account: Account<'info, EntryAccount>,

    #[account(
        init,
        payer = staker,
        space = StakeTracker::SPACE,
        seeds = [
            STAKE_TRACKER_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &[STAKE_SOURCE_TYPE_ENTRY],
            &chain_account.entry_count.to_le_bytes(),
        ],
        bump,
    )]
    pub stake_tracker: Account<'info, StakeTracker>,

    /// Client-generated stake account keypair - must be a signer.
    #[account(mut)]
    pub stake_account: Signer<'info>,

    /// CHECK: chain_authority PDA - no backing account, verified in handler.
    pub chain_authority: UncheckedAccount<'info>,

    /// CHECK: validator vote account - verified against ValidatorRegistry in handler.
    pub validator_vote: UncheckedAccount<'info>,

    /// CHECK: protocol fee receiver - verified == global_config.authority in handler.
    #[account(mut)]
    pub fee_receiver: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: rent sysvar - passed to stake_utils for CPI.
    pub rent: UncheckedAccount<'info>,

    /// CHECK: stake_history sysvar - key checked by stake_utils.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: stake_config sysvar - key checked by stake_utils.
    pub stake_config: UncheckedAccount<'info>,

    /// CHECK: native stake program - key checked by stake_utils.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<EnterChain>, args: EnterChainArgs) -> Result<()> {
    let chain = &ctx.accounts.chain_account;
    let config = &ctx.accounts.global_config;

    // -- 1. Validate chain is not broken --
    require!(!chain.is_broken, ChainStakingError::ChainIsBroken);

    // -- 2. Compute required entry amount --
    let required_amount = if chain.entry_count == 0 {
        MIN_ENTRY_LAMPORTS
    } else {
        compute_required_entry(chain.last_entry_amount, chain.spread_bps)?
    };
    require!(
        args.amount >= required_amount,
        ChainStakingError::InsufficientEntryAmount
    );

    // -- 3. Validate validator is in approved list --
    require!(
        ctx.accounts
            .validator_registry
            .validator_list
            .contains(ctx.accounts.validator_vote.key),
        ChainStakingError::InvalidValidator
    );

    // -- 4. Validate fee receiver --
    require_keys_eq!(
        *ctx.accounts.fee_receiver.key,
        config.authority,
        ChainStakingError::InvalidFeeReceiver
    );

    // -- 5. Validate chain_authority PDA --
    let (expected_chain_authority, _) = find_chain_authority_pda(ctx.program_id, args.chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    // -- 6. Compute fees --
    let protocol_fee = (args.amount as u128)
        .checked_mul(config.fee_bps as u128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(ChainStakingError::ArithmeticOverflow)? as u64;

    let flat_fee = config.flat_fee_lamports;

    // -- 7. Capture position before mutation --
    let position = chain.entry_count;
    let round = chain.current_round;
    let chain_id = args.chain_id;
    let chain_authority_bump = chain.chain_authority_bump;

    // -- 8. Update chain state (effects before interactions) --
    let chain = &mut ctx.accounts.chain_account;
    chain.entry_count = position
        .checked_add(1)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    chain.last_entry_amount = args.amount;
    chain.last_entry_timestamp = ctx.accounts.clock.unix_timestamp;
    chain.last_entry_authority = ctx.accounts.staker.key();
    chain.total_staked_lamports = chain
        .total_staked_lamports
        .checked_add(args.amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    // -- 9. Write entry account --
    ctx.accounts.entry_account.set_inner(EntryAccount {
        chain_id,
        round,
        position,
        staker: ctx.accounts.staker.key(),
        principal_lamports: args.amount,
        stake_account: ctx.accounts.stake_account.key(),
        validator: *ctx.accounts.validator_vote.key,
        is_unstaked: false,
        is_withdrawn: false,
        payout_lamports: 0,
        bump: ctx.bumps.entry_account,
    });

    // -- 10. Write stake tracker --
    ctx.accounts.stake_tracker.set_inner(StakeTracker {
        chain_id,
        round,
        stake_account: ctx.accounts.stake_account.key(),
        principal_lamports: args.amount,
        source_type: STAKE_SOURCE_TYPE_ENTRY,
        source_index: position,
        is_active: true,
        is_deactivated: false,
        is_withdrawn: false,
        bump: ctx.bumps.stake_tracker,
    });

    // -- 11. Update trigger vault reserve --
    ctx.accounts.trigger_vault.reserve_lamports = ctx
        .accounts
        .trigger_vault
        .reserve_lamports
        .checked_add(flat_fee)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    // -- 12. Transfer protocol fee to fee_receiver --
    if protocol_fee > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.staker.to_account_info(),
                    to: ctx.accounts.fee_receiver.to_account_info(),
                },
            ),
            protocol_fee,
        )?;
    }

    // -- 13. Transfer flat fee to trigger vault --
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.staker.to_account_info(),
                to: ctx.accounts.trigger_vault.to_account_info(),
            },
        ),
        flat_fee,
    )?;

    // -- 14. Create + initialize + delegate stake account via CPI --
    stake_utils::create_stake_account(
        &ctx.accounts.staker.to_account_info(),
        &ctx.accounts.stake_account.to_account_info(),
        &ctx.accounts.rent.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
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

    Ok(())
}

/// Compute required entry: `last_entry_amount * (10_000 + spread_bps) / 10_000`
/// Uses u128 to prevent overflow.
fn compute_required_entry(last_entry_amount: u64, spread_bps: u32) -> Result<u64> {
    let multiplier = 10_000u128
        .checked_add(spread_bps as u128)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    let result = (last_entry_amount as u128)
        .checked_mul(multiplier)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    u64::try_from(result).map_err(|_| ChainStakingError::ArithmeticOverflow.into())
}
