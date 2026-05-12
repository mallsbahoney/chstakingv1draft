use anchor_lang::{prelude::*, AccountsExit};
use solana_stake_interface::program as stake_program_interface;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{ChainAccount, StakeTracker, TriggerVault},
};

#[derive(Accounts)]
#[instruction(chain_id: u8)]
pub struct TriggerUnstake<'info> {
    #[account(mut)]
    pub triggerer: Signer<'info>,

    #[account(
        seeds = [CHAIN_SEED, &[chain_id]],
        bump = chain_account.bump,
    )]
    pub chain_account: Account<'info, ChainAccount>,

    #[account(
        mut,
        seeds = [TRIGGER_VAULT_SEED, &[chain_id]],
        bump = trigger_vault.bump,
    )]
    pub trigger_vault: Account<'info, TriggerVault>,

    /// CHECK: chain_authority PDA with seeds [b"chain_auth", &[chain_id]].
    /// Verified against the program-derived PDA in the handler.
    pub chain_authority: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: stake_history sysvar. Key validated by `stake_utils`.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: native stake program. Key validated by `stake_utils`.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler<'info>(
    ctx: Context<'_, '_, 'info, 'info, TriggerUnstake<'info>>,
    chain_id: u8,
) -> Result<()> {
    let chain = &ctx.accounts.chain_account;

    require!(chain.is_broken, ChainStakingError::ChainNotBroken);
    require!(
        chain.chain_id == chain_id,
        ChainStakingError::StakeTrackerChainMismatch
    );
    require!(
        ctx.accounts.trigger_vault.chain_id == chain_id,
        ChainStakingError::InvalidTriggerVault
    );

    let (expected_chain_authority, _) = find_chain_authority_pda(ctx.program_id, chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    let pair_count = validate_stake_pairs(ctx.remaining_accounts.len())?;
    let total_trigger_reward = compute_total_trigger_reward(pair_count)?;

    require!(
        ctx.accounts.trigger_vault.reserve_lamports >= total_trigger_reward,
        ChainStakingError::InsufficientTriggerRewardReserve
    );

    let chain_id_byte = [chain_id];
    let chain_authority_bump_byte = [chain.chain_authority_bump];
    let chain_authority_signer_seeds =
        stake_utils::chain_authority_signer_seeds_raw(&chain_id_byte, &chain_authority_bump_byte);

    let chain_authority_info = ctx.accounts.chain_authority.to_account_info();
    let clock_info = ctx.accounts.clock.to_account_info();
    let stake_history_info = ctx.accounts.stake_history.to_account_info();
    let stake_program_info = ctx.accounts.stake_program.to_account_info();

    let stake_pairs = ctx.remaining_accounts.chunks_exact(2);
    require!(
        stake_pairs.remainder().is_empty(),
        ChainStakingError::InvalidTriggerUnstakeAccounts
    );

    for stake_pair in stake_pairs {
        let stake_tracker_info = &stake_pair[0];
        let stake_account_info = &stake_pair[1];

        require!(
            stake_tracker_info.is_writable,
            ChainStakingError::PdaAccountNotWritable
        );

        let mut stake_tracker = Account::<StakeTracker>::try_from(stake_tracker_info)?;

        require!(
            stake_tracker.chain_id == chain_id,
            ChainStakingError::StakeTrackerChainMismatch
        );
        // ── FIX (M-09): Validate tracker belongs to current round ──
        require!(
            stake_tracker.round == chain.current_round,
            ChainStakingError::StakeTrackerRoundMismatch
        );
        require_keys_eq!(
            *stake_account_info.key,
            stake_tracker.stake_account,
            ChainStakingError::StakeTrackerStakeAccountMismatch
        );
        require!(
            !stake_tracker.is_deactivated,
            ChainStakingError::StakeAlreadyDeactivated
        );
        require!(
            !stake_tracker.is_withdrawn,
            ChainStakingError::AlreadyWithdrawn
        );
        require!(
            stake_account_info.owner == &stake_program_interface::ID,
            ChainStakingError::InvalidStakeAccountOwner
        );

        stake_utils::deactivate_stake_account_with_signer_seeds(
            stake_account_info,
            &chain_authority_info,
            &clock_info,
            &stake_history_info,
            &stake_program_info,
            &chain_authority_signer_seeds,
        )?;

        stake_tracker.is_deactivated = true;
        stake_tracker.is_active = false;
        stake_tracker.exit(ctx.program_id)?;
    }

    ctx.accounts.trigger_vault.reserve_lamports = ctx
        .accounts
        .trigger_vault
        .reserve_lamports
        .checked_sub(total_trigger_reward)
        .ok_or(ChainStakingError::InsufficientTriggerRewardReserve)?;

    transfer_program_lamports(
        &ctx.accounts.trigger_vault.to_account_info(),
        &ctx.accounts.triggerer.to_account_info(),
        total_trigger_reward,
    )?;

    Ok(())
}

fn validate_stake_pairs(remaining_account_len: usize) -> Result<usize> {
    require!(
        remaining_account_len > 0 && remaining_account_len % 2 == 0,
        ChainStakingError::InvalidTriggerUnstakeAccounts
    );

    let pair_count = remaining_account_len / 2;
    require!(
        pair_count <= usize::from(DEFAULT_UNSTAKE_BATCH_SIZE),
        ChainStakingError::TriggerUnstakeBatchTooLarge
    );

    Ok(pair_count)
}

fn compute_total_trigger_reward(pair_count: usize) -> Result<u64> {
    let pair_count =
        u64::try_from(pair_count).map_err(|_| ChainStakingError::ArithmeticOverflow)?;

    DEFAULT_TRIGGER_REWARD_LAMPORTS
        .checked_mul(pair_count)
        .ok_or_else(|| ChainStakingError::ArithmeticOverflow.into())
}

fn transfer_program_lamports(
    source: &AccountInfo<'_>,
    destination: &AccountInfo<'_>,
    amount: u64,
) -> Result<()> {
    let updated_source_lamports = source
        .lamports()
        .checked_sub(amount)
        .ok_or(ChainStakingError::InsufficientTriggerRewardReserve)?;
    let updated_destination_lamports = destination
        .lamports()
        .checked_add(amount)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    **source.try_borrow_mut_lamports()? = updated_source_lamports;
    **destination.try_borrow_mut_lamports()? = updated_destination_lamports;

    Ok(())
}
