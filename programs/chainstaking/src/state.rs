use anchor_lang::prelude::*;

use crate::constants::MAX_VALIDATORS;

#[account]
pub struct GlobalConfig {
    pub authority: Pubkey,
    pub is_initialized: bool,
    pub total_chains: u8,
    pub fee_bps: u16,
    pub flat_fee_lamports: u64,
    pub withdrawal_fee_bps: u16,
    pub cooldown_seconds: i64,
    pub unstake_batch_size: u8,
    pub trigger_reward_lamports: u64,
    pub bump: u8,
}

impl GlobalConfig {
    pub const INIT_SPACE: usize = 32 + 1 + 1 + 2 + 8 + 2 + 8 + 1 + 8 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct ValidatorRegistry {
    pub validator_list: Vec<Pubkey>,
    /// No separate validator_count field - always use validator_list.len().
    pub bump: u8,
}

impl ValidatorRegistry {
    // 4 (Vec length prefix) + 16 * 32 (pubkeys) + 1 (bump)
    pub const INIT_SPACE: usize = 4 + (MAX_VALIDATORS * 32) + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct TriggerVault {
    pub chain_id: u8,
    /// Logical trigger reward reserve tracked by the protocol.
    /// Not guaranteed to equal account lamports().
    /// Always use account lamports() for actual SOL balance checks.
    pub reserve_lamports: u64,
    pub bump: u8,
}

impl TriggerVault {
    pub const INIT_SPACE: usize = 1 + 8 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct ChainAccount {
    pub chain_id: u8,
    /// `u32` is required because the 10_000% chain equals 1_000_000 bps.
    pub spread_bps: u32,
    pub entry_count: u32,
    /// current_round starts at 0.
    /// Entries use current_round when created.
    /// Incremented after break execution completes.
    pub current_round: u32,
    pub last_entry_amount: u64,
    pub last_entry_timestamp: i64,
    /// Pubkey::default() (all zeros) means no entry has been made yet.
    pub last_entry_authority: Pubkey,
    pub total_staked_lamports: u64,
    pub frontend_estimated_yield_pool_lamports: u64,
    pub donation_pot_lamports: u64,
    /// Next donation index for the active round.
    pub donation_count: u16,
    pub is_broken: bool,
    /// 0 = none, 1 = manual, 2 = community.
    pub break_type: u8,
    pub bump: u8,
    /// Bump of the chain_authority PDA (seeds: [b"chain_auth", &[chain_id]]).
    /// Stored at init to avoid re-derivation cost during stake CPI signing.
    pub chain_authority_bump: u8,
    /// Set to true once compute_payouts has processed all entries for this round.
    /// Enables safe round advancement in withdraw_after_cooldown.
    pub round_settled: bool,
}

impl ChainAccount {
    // 1 + 4 + 4 + 4 + 8 + 8 + 32 + 8 + 8 + 8 + 2 + 1 + 1 + 1 + 1 + 1 (round_settled)
    pub const INIT_SPACE: usize = 1 + 4 + 4 + 4 + 8 + 8 + 32 + 8 + 8 + 8 + 2 + 1 + 1 + 1 + 1 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct EntryAccount {
    pub chain_id: u8,
    pub round: u32,
    pub position: u32,
    pub staker: Pubkey,
    pub principal_lamports: u64,
    pub stake_account: Pubkey,
    pub validator: Pubkey,
    pub is_unstaked: bool,
    pub is_withdrawn: bool,
    pub payout_lamports: u64,
    pub bump: u8,
}

impl EntryAccount {
    pub const INIT_SPACE: usize = 1 + 4 + 4 + 32 + 8 + 32 + 32 + 1 + 1 + 8 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct StakeTracker {
    pub chain_id: u8,
    pub round: u32,
    pub stake_account: Pubkey,
    pub principal_lamports: u64,
    pub source_type: u8,
    pub source_index: u32,
    pub is_active: bool,
    pub is_deactivated: bool,
    pub is_withdrawn: bool,
    pub bump: u8,
}

impl StakeTracker {
    pub const INIT_SPACE: usize = 1 + 4 + 32 + 8 + 1 + 4 + 1 + 1 + 1 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct DonationRecord {
    pub chain_id: u8,
    pub round: u32,
    pub donation_index: u16,
    pub donor: Pubkey,
    pub amount_lamports: u64,
    pub stake_account: Pubkey,
    pub validator: Pubkey,
    pub bump: u8,
}

impl DonationRecord {
    pub const INIT_SPACE: usize = 1 + 4 + 2 + 32 + 8 + 32 + 32 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}

#[account]
pub struct CompoundRecord {
    pub chain_id: u8,
    pub round: u32,
    pub compound_index: u16,
    pub amount_lamports: u64,
    pub stake_account: Pubkey,
    pub validator: Pubkey,
    pub bump: u8,
}

impl CompoundRecord {
    pub const INIT_SPACE: usize = 1 + 4 + 2 + 8 + 32 + 32 + 1;
    pub const SPACE: usize = 8 + Self::INIT_SPACE;
}
