use anchor_lang::prelude::*;

use crate::{
    constants::*,
    errors::ChainStakingError,
    state::{ChainAccount, EntryAccount, GlobalConfig, StakeTracker},
    yield_utils::{self, EntryPayoutInput},
};

/// Arguments for compute_payouts.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ComputePayoutsArgs {
    pub chain_id: u8,
    /// The starting entry position for this paginated batch (inclusive).
    pub start_position: u32,
}

#[derive(Accounts)]
#[instruction(args: ComputePayoutsArgs)]
pub struct ComputePayouts<'info> {
    /// Anyone can crank — permissionless.
    pub caller: Signer<'info>,

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
}

/// Compute payouts for a batch of entries on a broken chain.
///
/// remaining_accounts layout (per entry to process):
///   [entry_account_0, stake_tracker_0, stake_account_0,
///    entry_account_1, stake_tracker_1, stake_account_1,
///    ...]
///
/// Each triplet consists of:
///   1. EntryAccount PDA (writable — receives payout_lamports)
///   2. StakeTracker PDA (read-only — provides principal_lamports)
///   3. Native stake account (read-only — provides current balance)
///
/// Supports pagination: MAX_PAYOUTS_PER_CALL entries per invocation.
/// When start_position + batch_size >= entry_count, sets chain.round_settled = true.
pub fn handler<'info>(
    ctx: Context<'_, '_, 'info, 'info, ComputePayouts<'info>>,
    args: ComputePayoutsArgs,
) -> Result<()> {
    let chain = &ctx.accounts.chain_account;

    // ── 1. Chain must be broken ──
    require!(chain.is_broken, ChainStakingError::PayoutsRequireBrokenChain);
    require!(
        !chain.round_settled,
        ChainStakingError::RoundAlreadySettled
    );
    require!(
        chain.entry_count >= 2,
        ChainStakingError::InsufficientEntriesForPayouts
    );
    require!(
        chain.chain_id == args.chain_id,
        ChainStakingError::StakeTrackerChainMismatch
    );

    // ── 2. Validate remaining_accounts triplets ──
    let remaining = ctx.remaining_accounts;
    require!(
        remaining.len() > 0 && remaining.len() % 3 == 0,
        ChainStakingError::InvalidComputePayoutsAccounts
    );
    let batch_size = remaining.len() / 3;
    require!(
        batch_size <= MAX_PAYOUTS_PER_CALL,
        ChainStakingError::TooManyPayoutEntries
    );

    let chain_id = args.chain_id;
    let round = chain.current_round;
    let break_type = chain.break_type;
    let donation_pot = chain.donation_pot_lamports;

    // ── 3. Parse triplets and compute yield pool accounting ──
    let mut entry_inputs: Vec<EntryPayoutInput> = Vec::with_capacity(batch_size);
    let mut total_principal: u64 = 0;
    let mut total_stake_balance: u64 = 0;

    let triplets = remaining.chunks_exact(3);
    require!(
        triplets.remainder().is_empty(),
        ChainStakingError::InvalidComputePayoutsAccounts
    );

    for (batch_idx, triplet) in triplets.enumerate() {
        let entry_info = &triplet[0];
        let tracker_info = &triplet[1];
        let stake_info = &triplet[2];

        let expected_position = args
            .start_position
            .checked_add(batch_idx as u32)
            .ok_or(ChainStakingError::ArithmeticOverflow)?;

        // Validate entry account PDA
        let (expected_entry_pda, _) =
            find_entry_pda(ctx.program_id, chain_id, round, expected_position);
        require_keys_eq!(
            *entry_info.key,
            expected_entry_pda,
            ChainStakingError::PayoutEntryMismatch
        );
        require!(
            entry_info.is_writable,
            ChainStakingError::PdaAccountNotWritable
        );

        // Deserialise entry
        let entry = Account::<EntryAccount>::try_from(entry_info)?;
        require!(
            entry.chain_id == chain_id && entry.round == round,
            ChainStakingError::PayoutEntryMismatch
        );
        require!(
            entry.position == expected_position,
            ChainStakingError::PayoutEntriesOutOfOrder
        );

        // Validate stake tracker PDA
        let (expected_tracker_pda, _) = find_stake_tracker_pda(
            ctx.program_id,
            chain_id,
            round,
            STAKE_SOURCE_TYPE_ENTRY,
            expected_position,
        );
        require_keys_eq!(
            *tracker_info.key,
            expected_tracker_pda,
            ChainStakingError::StakeTrackerStakeAccountMismatch
        );

        let tracker = Account::<StakeTracker>::try_from(tracker_info)?;

        // Validate stake account matches tracker
        require_keys_eq!(
            *stake_info.key,
            tracker.stake_account,
            ChainStakingError::StakeTrackerStakeAccountMismatch
        );

        total_principal = total_principal
            .checked_add(tracker.principal_lamports)
            .ok_or(ChainStakingError::ArithmeticOverflow)?;
        total_stake_balance = total_stake_balance
            .checked_add(stake_info.lamports())
            .ok_or(ChainStakingError::ArithmeticOverflow)?;

        entry_inputs.push(EntryPayoutInput {
            position: expected_position,
            principal_lamports: entry.principal_lamports,
        });
    }

    // ── 4. Compute yield pool ──
    let yield_pool = total_stake_balance.saturating_sub(total_principal);

    // ── 5. Compute payouts using yield_utils ──
    let payouts = yield_utils::compute_payouts(
        &entry_inputs,
        break_type,
        yield_pool,
        donation_pot,
        total_principal,
    )?;

    // ── 6. Write payout_lamports back to each entry account ──
    let triplets = remaining.chunks_exact(3);
    for (batch_idx, triplet) in triplets.enumerate() {
        let entry_info = &triplet[0];
        let payout = &payouts[batch_idx];

        // Re-borrow the entry account mutably and update payout_lamports
        let mut entry = Account::<EntryAccount>::try_from(entry_info)?;
        entry.payout_lamports = payout.payout_lamports;
        entry.exit(ctx.program_id)?;
    }

    // ── 7. Check if this batch completes the round ──
    let end_position = args
        .start_position
        .checked_add(batch_size as u32)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    let chain = &mut ctx.accounts.chain_account;
    if end_position >= chain.entry_count {
        chain.round_settled = true;
    }

    Ok(())
}
