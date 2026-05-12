use anchor_lang::prelude::*;

use crate::{
    constants::*,
    errors::ChainStakingError,
    state::{ChainAccount, GlobalConfig},
};

#[derive(Accounts)]
#[instruction(chain_id: u8)]
pub struct ManualBreakChain<'info> {
    pub breaker: Signer<'info>,

    #[account(
        seeds = [GLOBAL_CONFIG_SEED],
        bump = global_config.bump,
    )]
    pub global_config: Account<'info, GlobalConfig>,

    #[account(
        mut,
        seeds = [CHAIN_SEED, &[chain_id]],
        bump = chain_account.bump,
    )]
    pub chain_account: Account<'info, ChainAccount>,

    pub clock: Sysvar<'info, Clock>,
}

pub fn handler(ctx: Context<ManualBreakChain>, _chain_id: u8) -> Result<()> {
    let chain = &ctx.accounts.chain_account;
    let config = &ctx.accounts.global_config;
    let clock = &ctx.accounts.clock;

    // -- 1. Chain must not already be broken --
    require!(!chain.is_broken, ChainStakingError::ChainIsBroken);

    // -- 2. Must have at least 2 entries - single entrant uses exit_first_entrant --
    require!(
        chain.entry_count >= 2,
        ChainStakingError::ManualBreakRequiresMultipleEntries
    );

    // -- 3. Breaker must be the last entry authority (current chain leader) --
    require_keys_eq!(
        ctx.accounts.breaker.key(),
        chain.last_entry_authority,
        ChainStakingError::ManualBreakUnauthorized
    );

    // -- 4. Cooldown timer must have expired --
    let cooldown_expiry = chain
        .last_entry_timestamp
        .checked_add(config.cooldown_seconds)
        .ok_or(ChainStakingError::ArithmeticOverflow)?;
    require!(
        clock.unix_timestamp >= cooldown_expiry,
        ChainStakingError::CooldownNotExpired
    );

    // -- 5. Transfer 1% of estimated yield pool into donation pot --
    //
    // Per PLAN.md Section 8: "Transfer 1% of yield_pool -> donation_pot (protective redistribution)"
    // This happens BEFORE payout computation. The yield pool estimate is used here as a
    // bookkeeping signal; real yield is computed from stake balances during compute_payouts.
    let yield_transfer = (chain.frontend_estimated_yield_pool_lamports as u128)
        .checked_mul(config.fee_bps as u128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(ChainStakingError::ArithmeticOverflow)? as u64;

    let chain = &mut ctx.accounts.chain_account;

    if yield_transfer > 0 {
        chain.frontend_estimated_yield_pool_lamports = chain
            .frontend_estimated_yield_pool_lamports
            .checked_sub(yield_transfer)
            .ok_or(ChainStakingError::ArithmeticOverflow)?;
        chain.donation_pot_lamports = chain
            .donation_pot_lamports
            .checked_add(yield_transfer)
            .ok_or(ChainStakingError::ArithmeticOverflow)?;
    }

    // -- 6. Mark chain as broken --
    chain.is_broken = true;
    chain.break_type = BREAK_TYPE_MANUAL;

    // Note: current_round is NOT incremented here.
    // Per PLAN.md Section 8: "Advance current_round to allow new entries immediately"
    // Round advancement happens after compute_payouts finalizes all entry payouts,
    // so that payout PDAs can still be derived using the current round.

    Ok(())
}
