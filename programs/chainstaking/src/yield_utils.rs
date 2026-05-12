use anchor_lang::prelude::*;

use crate::{
    constants::{BREAK_TYPE_COMMUNITY, BREAK_TYPE_MANUAL, MAX_PAYOUTS_PER_CALL},
    errors::ChainStakingError,
    state::StakeTracker,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldPoolAccounting {
    pub total_stake_account_balance_lamports: u64,
    pub principal_pool_lamports: u64,
    pub yield_pool_lamports: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryPayoutInput {
    pub position: u32,
    pub principal_lamports: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryPayoutBreakdown {
    pub position: u32,
    pub principal_lamports: u64,
    pub yield_lamports: u64,
    pub donation_lamports: u64,
    pub payout_lamports: u64,
}

pub fn compute_principal_pool_lamports(stake_trackers: &[StakeTracker]) -> Result<u64> {
    stake_trackers.iter().try_fold(0u64, |total, tracker| {
        total
            .checked_add(tracker.principal_lamports)
            .ok_or(ChainStakingError::ArithmeticOverflow.into())
    })
}

pub fn compute_total_stake_account_balance_lamports<'info>(
    stake_accounts: &[AccountInfo<'info>],
) -> Result<u64> {
    stake_accounts
        .iter()
        .try_fold(0u64, |total, stake_account| {
            total
                .checked_add(stake_account.lamports())
                .ok_or(ChainStakingError::ArithmeticOverflow.into())
        })
}

pub fn compute_yield_pool_lamports(
    total_stake_account_balance_lamports: u64,
    principal_pool_lamports: u64,
) -> Result<u64> {
    total_stake_account_balance_lamports
        .checked_sub(principal_pool_lamports)
        .ok_or(ChainStakingError::ArithmeticOverflow.into())
}

pub fn compute_yield_pool_accounting<'info>(
    stake_trackers: &[StakeTracker],
    stake_accounts: &[AccountInfo<'info>],
) -> Result<YieldPoolAccounting> {
    require!(
        stake_trackers.len() == stake_accounts.len(),
        ChainStakingError::MismatchedStakeBalanceCount
    );

    let principal_pool_lamports = compute_principal_pool_lamports(stake_trackers)?;
    let total_stake_account_balance_lamports =
        compute_total_stake_account_balance_lamports(stake_accounts)?;
    let yield_pool_lamports = compute_yield_pool_lamports(
        total_stake_account_balance_lamports,
        principal_pool_lamports,
    )?;

    Ok(YieldPoolAccounting {
        total_stake_account_balance_lamports,
        principal_pool_lamports,
        yield_pool_lamports,
    })
}

pub fn compute_available_liquid_rewards_lamports(
    account_info: &AccountInfo<'_>,
    rent: &Rent,
    data_space: usize,
) -> Result<u64> {
    let minimum_balance = rent.minimum_balance(data_space);
    Ok(match account_info.lamports().checked_sub(minimum_balance) {
        Some(available) => available,
        None => 0,
    })
}

pub fn compute_payouts(
    entries: &[EntryPayoutInput],
    break_type: u8,
    total_yield_pool_lamports: u64,
    donation_pot_lamports: u64,
    distributable_principal_pool_lamports: u64,
) -> Result<Vec<EntryPayoutBreakdown>> {
    require!(
        entries.len() <= MAX_PAYOUTS_PER_CALL,
        ChainStakingError::TooManyPayoutEntries
    );
    require!(
        entries.len() >= 2,
        ChainStakingError::InsufficientEntriesForPayouts
    );

    for (expected_position, entry) in entries.iter().enumerate() {
        require!(
            entry.position == expected_position as u32,
            ChainStakingError::PayoutEntriesOutOfOrder
        );
    }

    let leader_index = entries
        .len()
        .checked_sub(1)
        .ok_or(ChainStakingError::InsufficientEntriesForPayouts)?;
    let reward_pool_lamports = match break_type {
        BREAK_TYPE_MANUAL => donation_pot_lamports,
        BREAK_TYPE_COMMUNITY => total_yield_pool_lamports,
        _ => return Err(ChainStakingError::InvalidBreakType.into()),
    };

    if reward_pool_lamports > 0 && distributable_principal_pool_lamports == 0 {
        return Err(ChainStakingError::InvalidPayoutPrincipalPool.into());
    }

    let mut payouts = Vec::with_capacity(entries.len());

    for (index, entry) in entries.iter().enumerate() {
        let mut principal_lamports = 0u64;
        let mut yield_lamports = 0u64;
        let mut donation_lamports = 0u64;

        if index == leader_index {
            match break_type {
                BREAK_TYPE_MANUAL => {
                    yield_lamports = total_yield_pool_lamports;
                }
                BREAK_TYPE_COMMUNITY => {
                    donation_lamports = donation_pot_lamports;
                }
                _ => return Err(ChainStakingError::InvalidBreakType.into()),
            }
        } else if index > 0 {
            let next_entry = entries
                .get(index + 1)
                .ok_or(ChainStakingError::InsufficientEntriesForPayouts)?;
            principal_lamports = next_entry.principal_lamports;

            let proportional_share = compute_proportional_share(
                entry.principal_lamports,
                reward_pool_lamports,
                distributable_principal_pool_lamports,
            )?;

            match break_type {
                BREAK_TYPE_MANUAL => {
                    donation_lamports = proportional_share;
                }
                BREAK_TYPE_COMMUNITY => {
                    yield_lamports = proportional_share;
                }
                _ => return Err(ChainStakingError::InvalidBreakType.into()),
            }
        }

        let payout_lamports = principal_lamports
            .checked_add(yield_lamports)
            .and_then(|value| value.checked_add(donation_lamports))
            .ok_or(ChainStakingError::ArithmeticOverflow)?;

        payouts.push(EntryPayoutBreakdown {
            position: entry.position,
            principal_lamports,
            yield_lamports,
            donation_lamports,
            payout_lamports,
        });
    }

    Ok(payouts)
}

fn compute_proportional_share(
    user_principal_lamports: u64,
    pool_lamports: u64,
    distributable_principal_pool_lamports: u64,
) -> Result<u64> {
    if pool_lamports == 0 || user_principal_lamports == 0 {
        return Ok(0);
    }

    if distributable_principal_pool_lamports == 0 {
        return Err(ChainStakingError::InvalidPayoutPrincipalPool.into());
    }

    let share = (user_principal_lamports as u128)
        .checked_mul(pool_lamports as u128)
        .and_then(|value| value.checked_div(distributable_principal_pool_lamports as u128))
        .ok_or(ChainStakingError::ArithmeticOverflow)?;

    u64::try_from(share).map_err(|_| ChainStakingError::ArithmeticOverflow.into())
}
