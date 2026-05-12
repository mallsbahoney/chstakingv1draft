use anchor_lang::prelude::*;
use anchor_lang::system_program;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{
        ChainAccount, DonationRecord, GlobalConfig, StakeTracker, TriggerVault, ValidatorRegistry,
    },
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct DonateToChainArgs {
    pub amount: u64,
    pub chain_id: u8,
    pub donation_index: u16,
}

#[derive(Accounts)]
#[instruction(args: DonateToChainArgs)]
pub struct DonateToChain<'info> {
    #[account(mut)]
    pub donor: Signer<'info>,

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
        payer = donor,
        space = DonationRecord::SPACE,
        seeds = [
            DONATION_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &args.donation_index.to_le_bytes(),
        ],
        bump,
    )]
    pub donation_record: Account<'info, DonationRecord>,

    #[account(
        init,
        payer = donor,
        space = StakeTracker::SPACE,
        seeds = [
            STAKE_TRACKER_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &[STAKE_SOURCE_TYPE_DONATION],
            &(args.donation_index as u32).to_le_bytes(),
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

    /// CHECK: protocol fee receiver - validated == global_config.authority.
    #[account(mut)]
    pub fee_receiver: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: rent sysvar - passed through to stake_utils.
    pub rent: UncheckedAccount<'info>,

    /// CHECK: stake_history sysvar - key checked by stake_utils.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: stake_config sysvar - key checked by stake_utils.
    pub stake_config: UncheckedAccount<'info>,

    /// CHECK: native stake program - key checked by stake_utils.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<DonateToChain>, args: DonateToChainArgs) -> Result<()> {
    let chain = &ctx.accounts.chain_account;
    let config = &ctx.accounts.global_config;

    require!(args.amount > 0, ChainStakingError::InvalidDonationAmount);
    require!(!chain.is_broken, ChainStakingError::ChainIsBroken);
    require!(
        chain.entry_count > 0,
        ChainStakingError::DonationRequiresActiveLeader
    );
    require!(
        args.donation_index == chain.donation_count,
        ChainStakingError::InvalidDonationIndex
    );

    require!(
        ctx.accounts
            .validator_registry
            .validator_list
            .contains(ctx.accounts.validator_vote.key),
        ChainStakingError::InvalidValidator
    );

    require_keys_eq!(
        *ctx.accounts.fee_receiver.key,
        config.authority,
        ChainStakingError::InvalidFeeReceiver
    );

    let (expected_chain_authority, _) = find_chain_authority_pda(ctx.program_id, args.chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    let protocol_fee = compute_protocol_fee(args.amount, config.fee_bps)?;
    let flat_fee = config.flat_fee_lamports;
    // Capture last_entry_amount before mutable reborrow for community-break check.
    // Per PLAN.md §4.4 the threshold is last_entry_amount (NOT the spread-inflated next entry).
    let community_break_threshold = chain.last_entry_amount;

    let chain_id = args.chain_id;
    let round = chain.current_round;
    let donation_index = args.donation_index;
    let source_index = u32::from(donation_index);
    let chain_authority_bump = chain.chain_authority_bump;

    let chain = &mut ctx.accounts.chain_account;
    chain.donation_pot_lamports = chain
        .donation_pot_lamports
        .checked_add(args.amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    chain.donation_count = chain
        .donation_count
        .checked_add(1)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    // ── FIX (M-01): Community break threshold = last_entry_amount per PLAN.md §4.4 ──
    if chain.donation_pot_lamports >= community_break_threshold {
        chain.is_broken = true;
        chain.break_type = BREAK_TYPE_COMMUNITY;
    }

    // ── FIX (M-07): Track donation stake in total_staked_lamports ──
    chain.total_staked_lamports = chain
        .total_staked_lamports
        .checked_add(args.amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    ctx.accounts.trigger_vault.reserve_lamports = ctx
        .accounts
        .trigger_vault
        .reserve_lamports
        .checked_add(flat_fee)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    ctx.accounts.donation_record.set_inner(DonationRecord {
        chain_id,
        round,
        donation_index,
        donor: ctx.accounts.donor.key(),
        amount_lamports: args.amount,
        stake_account: ctx.accounts.stake_account.key(),
        validator: *ctx.accounts.validator_vote.key,
        bump: ctx.bumps.donation_record,
    });

    ctx.accounts.stake_tracker.set_inner(StakeTracker {
        chain_id,
        round,
        stake_account: ctx.accounts.stake_account.key(),
        principal_lamports: args.amount,
        source_type: STAKE_SOURCE_TYPE_DONATION,
        source_index,
        is_active: true,
        is_deactivated: false,
        is_withdrawn: false,
        bump: ctx.bumps.stake_tracker,
    });

    if protocol_fee > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.donor.to_account_info(),
                    to: ctx.accounts.fee_receiver.to_account_info(),
                },
            ),
            protocol_fee,
        )?;
    }

    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.donor.to_account_info(),
                to: ctx.accounts.trigger_vault.to_account_info(),
            },
        ),
        flat_fee,
    )?;

    stake_utils::create_stake_account(
        &ctx.accounts.donor.to_account_info(),
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

fn compute_protocol_fee(amount: u64, fee_bps: u16) -> Result<u64> {
    let fee = (amount as u128)
        .checked_mul(fee_bps as u128)
        .and_then(|value| value.checked_div(10_000))
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    u64::try_from(fee).map_err(|_| ChainStakingError::ArithmeticOverflow.into())
}
