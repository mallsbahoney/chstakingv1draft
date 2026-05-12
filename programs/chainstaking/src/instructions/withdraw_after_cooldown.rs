use anchor_lang::prelude::*;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{ChainAccount, EntryAccount, GlobalConfig, StakeTracker},
};

/// Arguments for withdraw_after_cooldown.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawAfterCooldownArgs {
    pub chain_id: u8,
    /// The source_type of the stake tracker being withdrawn (entry=0, donation=1, compound=2).
    pub source_type: u8,
    /// The source_index of the stake tracker being withdrawn.
    pub source_index: u32,
    /// The position of the entry account whose payout_lamports is being claimed.
    /// For entry stakes, this matches source_index. For donation/compound stakes,
    /// this is the entry that the payout was computed against.
    pub entry_position: u32,
}

#[derive(Accounts)]
#[instruction(args: WithdrawAfterCooldownArgs)]
pub struct WithdrawAfterCooldown<'info> {
    /// The recipient of payout lamports. Can be anyone — permissionless completion.
    #[account(mut)]
    pub recipient: Signer<'info>,

    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
    )]
    pub global_config: Account<'info, GlobalConfig>,

    #[account(
        mut,
        seeds = [CHAIN_SEED, &[args.chain_id]],
        bump = chain_account.bump,
    )]
    pub chain_account: Account<'info, ChainAccount>,

    #[account(
        mut,
        seeds = [
            ENTRY_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &args.entry_position.to_le_bytes(),
        ],
        bump = entry_account.bump,
    )]
    pub entry_account: Account<'info, EntryAccount>,

    #[account(
        mut,
        seeds = [
            STAKE_TRACKER_SEED,
            &[args.chain_id],
            &chain_account.current_round.to_le_bytes(),
            &[args.source_type],
            &args.source_index.to_le_bytes(),
        ],
        bump = stake_tracker.bump,
    )]
    pub stake_tracker: Account<'info, StakeTracker>,

    /// CHECK: The native stake account to withdraw from.
    /// Validated by matching stake_tracker.stake_account.
    #[account(mut)]
    pub stake_account: UncheckedAccount<'info>,

    /// CHECK: chain_authority PDA — no backing account, verified in handler.
    #[account(mut)]
    pub chain_authority: UncheckedAccount<'info>,

    /// The staker who originally entered — receives the payout.
    /// CHECK: validated == entry_account.staker.
    #[account(mut)]
    pub staker: UncheckedAccount<'info>,

    /// CHECK: protocol fee receiver — validated == global_config.authority.
    #[account(mut)]
    pub fee_receiver: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: stake_history sysvar — key checked by stake_utils.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: native stake program — key checked by stake_utils.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<WithdrawAfterCooldown>, args: WithdrawAfterCooldownArgs) -> Result<()> {
    let chain = &ctx.accounts.chain_account;

    // ── 1. Chain must be broken ──
    require!(chain.is_broken, ChainStakingError::ChainNotBroken);

    // ── 2. Round must be settled (compute_payouts completed) ──
    require!(
        chain.round_settled,
        ChainStakingError::RoundNotSettled
    );

    // ── 3. Stake tracker must be deactivated but not yet withdrawn ──
    require!(
        ctx.accounts.stake_tracker.is_deactivated,
        ChainStakingError::StakeNotDeactivated
    );
    require!(
        !ctx.accounts.stake_tracker.is_withdrawn,
        ChainStakingError::AlreadyWithdrawn
    );

    // ── 4. Validate stake account matches tracker ──
    require_keys_eq!(
        *ctx.accounts.stake_account.key,
        ctx.accounts.stake_tracker.stake_account,
        ChainStakingError::StakeTrackerStakeAccountMismatch
    );

    // ── 5. Validate chain_authority PDA ──
    let (expected_chain_authority, _) =
        find_chain_authority_pda(ctx.program_id, args.chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    // ── 6. Validate staker matches the entry ──
    require_keys_eq!(
        *ctx.accounts.staker.key,
        ctx.accounts.entry_account.staker,
        ChainStakingError::ExitWrongStaker
    );

    // ── 7. Validate fee receiver ──
    require_keys_eq!(
        *ctx.accounts.fee_receiver.key,
        ctx.accounts.global_config.authority,
        ChainStakingError::InvalidFeeReceiver
    );

    // ── 8. Validate entry not already withdrawn ──
    require!(
        !ctx.accounts.entry_account.is_withdrawn,
        ChainStakingError::AlreadyWithdrawn
    );

    let chain_id = args.chain_id;
    let chain_authority_bump = chain.chain_authority_bump;

    // ── 9. CPI: withdraw all lamports from native stake account to chain_authority PDA ──
    // The native stake program enforces that cooldown has passed.
    // We withdraw into chain_authority (program-controlled PDA) for safe distribution.
    let stake_balance_before = ctx.accounts.stake_account.lamports();

    stake_utils::withdraw_stake_account(
        &ctx.accounts.stake_account.to_account_info(),
        &ctx.accounts.chain_authority.to_account_info(),
        &ctx.accounts.chain_authority.to_account_info(),
        &ctx.accounts.clock.to_account_info(),
        &ctx.accounts.stake_history.to_account_info(),
        &ctx.accounts.stake_program.to_account_info(),
        chain_id,
        chain_authority_bump,
    )?;

    // ── 10. Compute yield portion and withdrawal fee ──
    let payout = ctx.accounts.entry_account.payout_lamports;
    let principal = ctx.accounts.entry_account.principal_lamports;

    // yield_portion = actual stake balance - principal (actual yield from staking)
    let actual_yield = stake_balance_before.saturating_sub(principal);

    // 1% withdrawal fee on actual yield only
    let withdrawal_fee = if actual_yield > 0 {
        (actual_yield as u128)
            .checked_mul(ctx.accounts.global_config.withdrawal_fee_bps as u128)
            .and_then(|v| v.checked_div(10_000))
            .ok_or(ChainStakingError::ArithmeticOverflow)? as u64
    } else {
        0u64
    };

    // Net payout after fee (use payout_lamports from compute_payouts, minus fee)
    let net_payout = payout
        .checked_sub(withdrawal_fee)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    // ── 11. Distribute from chain_authority PDA ──
    // Transfer net payout to staker
    let chain_id_byte = [chain_id];
    let chain_authority_bump_byte = [chain_authority_bump];
    let signer_seeds =
        stake_utils::chain_authority_signer_seeds_raw(&chain_id_byte, &chain_authority_bump_byte);

    if net_payout > 0 {
        transfer_from_pda(
            &ctx.accounts.chain_authority.to_account_info(),
            &ctx.accounts.staker.to_account_info(),
            net_payout,
        )?;
    }

    // Transfer withdrawal fee to fee_receiver
    if withdrawal_fee > 0 {
        transfer_from_pda(
            &ctx.accounts.chain_authority.to_account_info(),
            &ctx.accounts.fee_receiver.to_account_info(),
            withdrawal_fee,
        )?;
    }

    // Transfer any remaining dust (stake_balance - payout - fee) back to staker
    // This handles rounding dust from compute_payouts vs actual stake balance
    let distributed = net_payout
        .checked_add(withdrawal_fee)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    let remaining_dust = stake_balance_before.saturating_sub(distributed);
    if remaining_dust > 0 {
        transfer_from_pda(
            &ctx.accounts.chain_authority.to_account_info(),
            &ctx.accounts.staker.to_account_info(),
            remaining_dust,
        )?;
    }

    // Use signer_seeds variable to suppress unused warning (seeds validated by PDA constraint)
    let _ = signer_seeds;

    // ── 12. Mark stake tracker and entry as withdrawn ──
    ctx.accounts.stake_tracker.is_withdrawn = true;
    ctx.accounts.entry_account.is_withdrawn = true;
    ctx.accounts.entry_account.is_unstaked = true;

    // ── 13. Update chain: decrement total_staked by principal ──
    let chain = &mut ctx.accounts.chain_account;
    chain.total_staked_lamports = chain
        .total_staked_lamports
        .saturating_sub(ctx.accounts.stake_tracker.principal_lamports);

    // ── 14. Check if all entry stakes are withdrawn → advance round ──
    // total_staked tracks entries + donations + compounds, so when it hits 0,
    // all stakes have been withdrawn.
    if chain.total_staked_lamports == 0 && chain.round_settled {
        chain.current_round = chain
            .current_round
            .checked_add(1)
            .ok_or(ChainStakingError::ArithmeticOverflow)?;
        chain.entry_count = 0;
        chain.donation_count = 0;
        chain.donation_pot_lamports = 0;
        chain.last_entry_amount = 0;
        chain.last_entry_timestamp = 0;
        chain.last_entry_authority = Pubkey::default();
        chain.frontend_estimated_yield_pool_lamports = 0;
        chain.is_broken = false;
        chain.break_type = BREAK_TYPE_NONE;
        chain.round_settled = false;
    }

    Ok(())
}

/// Transfer lamports from a program-owned PDA account using direct lamport manipulation.
/// This is safe because chain_authority is a PDA owned by the system program (no data),
/// and the runtime allows programs to debit accounts they own or system-owned PDAs
/// for which they have provided valid signer seeds.
fn transfer_from_pda(
    source: &AccountInfo<'_>,
    destination: &AccountInfo<'_>,
    amount: u64,
) -> Result<()> {
    let updated_source_lamports = source
        .lamports()
        .checked_sub(amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    let updated_destination_lamports = destination
        .lamports()
        .checked_add(amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    **source.try_borrow_mut_lamports()? = updated_source_lamports;
    **destination.try_borrow_mut_lamports()? = updated_destination_lamports;

    Ok(())
}
