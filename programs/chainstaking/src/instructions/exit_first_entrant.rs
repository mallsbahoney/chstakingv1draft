use anchor_lang::prelude::*;

use crate::{
    constants::*,
    errors::ChainStakingError,
    stake_utils,
    state::{ChainAccount, EntryAccount, StakeTracker},
};

#[derive(Accounts)]
#[instruction(chain_id: u8)]
pub struct ExitFirstEntrant<'info> {
    #[account(mut)]
    pub staker: Signer<'info>,

    #[account(
        mut,
        seeds = [CHAIN_SEED, &[chain_id]],
        bump = chain_account.bump,
    )]
    pub chain_account: Account<'info, ChainAccount>,

    #[account(
        mut,
        seeds = [
            ENTRY_SEED,
            &[chain_id],
            &chain_account.current_round.to_le_bytes(),
            &0u32.to_le_bytes(),  // position 0 - always the first entry
        ],
        bump = entry_account.bump,
    )]
    pub entry_account: Account<'info, EntryAccount>,

    #[account(
        mut,
        seeds = [
            STAKE_TRACKER_SEED,
            &[chain_id],
            &chain_account.current_round.to_le_bytes(),
            &[STAKE_SOURCE_TYPE_ENTRY],
            &0u32.to_le_bytes(),  // source_index 0 - matches entry position 0
        ],
        bump = stake_tracker.bump,
    )]
    pub stake_tracker: Account<'info, StakeTracker>,

    /// CHECK: The native stake account to deactivate/withdraw.
    /// Validated by matching entry_account.stake_account.
    #[account(mut)]
    pub stake_account: UncheckedAccount<'info>,

    /// CHECK: chain_authority PDA - no backing account, verified in handler.
    pub chain_authority: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,

    /// CHECK: stake_history sysvar - key checked by stake_utils.
    pub stake_history: UncheckedAccount<'info>,

    /// CHECK: native stake program - key checked by stake_utils.
    pub stake_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<ExitFirstEntrant>, chain_id: u8) -> Result<()> {
    let chain = &ctx.accounts.chain_account;

    // -- 1. Validate exactly 1 entry exists --
    require!(chain.entry_count == 1, ChainStakingError::ExitNotAllowed);

    // -- 2. Validate caller is the entry's staker --
    require_keys_eq!(
        ctx.accounts.staker.key(),
        ctx.accounts.entry_account.staker,
        ChainStakingError::ExitWrongStaker
    );

    // -- 3. Validate chain_authority PDA --
    let (expected_chain_authority, _) = find_chain_authority_pda(ctx.program_id, chain_id);
    require_keys_eq!(
        *ctx.accounts.chain_authority.key,
        expected_chain_authority,
        ChainStakingError::InvalidChainAuthority
    );

    // -- 4. Validate stake_account matches entry record --
    require_keys_eq!(
        *ctx.accounts.stake_account.key,
        ctx.accounts.entry_account.stake_account,
    );

    let chain_authority_bump = chain.chain_authority_bump;

    // -- 5. Already withdrawn? --
    require!(
        !ctx.accounts.stake_tracker.is_withdrawn,
        ChainStakingError::AlreadyWithdrawn
    );

    // -- 6. Stake state decision: deactivate or withdraw --
    if !ctx.accounts.stake_tracker.is_deactivated {
        // Step A: Deactivate - user must call again after epoch cooldown
        stake_utils::deactivate_stake_account(
            &ctx.accounts.stake_account.to_account_info(),
            &ctx.accounts.chain_authority.to_account_info(),
            &ctx.accounts.clock.to_account_info(),
            &ctx.accounts.stake_history.to_account_info(),
            &ctx.accounts.stake_program.to_account_info(),
            chain_id,
            chain_authority_bump,
        )?;

        ctx.accounts.stake_tracker.is_deactivated = true;
        ctx.accounts.stake_tracker.is_active = false;
        ctx.accounts.entry_account.is_unstaked = true;
    } else {
        // Step B: Withdraw to staker - cooldown must have passed (stake program enforces this)
        stake_utils::withdraw_stake_account(
            &ctx.accounts.stake_account.to_account_info(),
            &ctx.accounts.staker.to_account_info(),
            &ctx.accounts.chain_authority.to_account_info(),
            &ctx.accounts.clock.to_account_info(),
            &ctx.accounts.stake_history.to_account_info(),
            &ctx.accounts.stake_program.to_account_info(),
            chain_id,
            chain_authority_bump,
        )?;

        ctx.accounts.stake_tracker.is_withdrawn = true;
        ctx.accounts.entry_account.is_withdrawn = true;

        // -- 7. Reset chain state --
        let chain = &mut ctx.accounts.chain_account;
        chain.entry_count = 0;
        chain.last_entry_amount = 0;
        chain.last_entry_timestamp = 0;
        chain.last_entry_authority = Pubkey::default();
        chain.total_staked_lamports = 0;
    }

    Ok(())
}
