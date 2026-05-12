use anchor_lang::prelude::Pubkey;

pub const TOTAL_CHAINS: usize = 10;
pub const TOTAL_CHAINS_U8: u8 = TOTAL_CHAINS as u8;
pub const MAX_VALIDATORS: usize = 16;
pub const MAX_PAYOUTS_PER_CALL: usize = 10;
pub const EXPECTED_INITIALIZE_REMAINING_ACCOUNTS: usize = TOTAL_CHAINS * 2;
/// remaining_accounts[TRIGGER_VAULT_OFFSET .. TRIGGER_VAULT_OFFSET + TOTAL_CHAINS] = TriggerVault PDAs
pub const TRIGGER_VAULT_OFFSET: usize = 0;
/// remaining_accounts[CHAIN_ACCOUNT_OFFSET .. CHAIN_ACCOUNT_OFFSET + TOTAL_CHAINS] = ChainAccount PDAs
pub const CHAIN_ACCOUNT_OFFSET: usize = TOTAL_CHAINS;

pub const DEFAULT_FEE_BPS: u16 = 100;
pub const DEFAULT_FLAT_FEE_LAMPORTS: u64 = 10_000_000;
pub const DEFAULT_WITHDRAWAL_FEE_BPS: u16 = 100;
pub const DEFAULT_COOLDOWN_SECONDS: i64 = 300;
pub const DEFAULT_UNSTAKE_BATCH_SIZE: u8 = 5;
pub const DEFAULT_TRIGGER_REWARD_LAMPORTS: u64 = 10_000_000;
pub const MIN_ENTRY_LAMPORTS: u64 = 10_000_000;

pub const STAKE_SOURCE_TYPE_ENTRY: u8 = 0;
pub const STAKE_SOURCE_TYPE_DONATION: u8 = 1;
pub const STAKE_SOURCE_TYPE_COMPOUND: u8 = 2;

pub const BREAK_TYPE_NONE: u8 = 0;
pub const BREAK_TYPE_MANUAL: u8 = 1;
pub const BREAK_TYPE_COMMUNITY: u8 = 2;

pub const CHAIN_SPREADS_BPS: [u32; TOTAL_CHAINS] = [
    500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 500000, 1000000,
];

pub const GLOBAL_CONFIG_SEED: &[u8] = b"global_config";
pub const VALIDATOR_REGISTRY_SEED: &[u8] = b"validator_registry";
pub const TRIGGER_VAULT_SEED: &[u8] = b"trigger_vault";
pub const CHAIN_SEED: &[u8] = b"chain";
pub const ENTRY_SEED: &[u8] = b"entry";
pub const STAKE_TRACKER_SEED: &[u8] = b"stake_tracker";
pub const DONATION_SEED: &[u8] = b"donation";
pub const COMPOUND_SEED: &[u8] = b"compound";
pub const CHAIN_AUTH_SEED: &[u8] = b"chain_auth";

pub fn find_global_config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[GLOBAL_CONFIG_SEED], program_id)
}

pub fn find_validator_registry_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VALIDATOR_REGISTRY_SEED], program_id)
}

pub fn find_trigger_vault_pda(program_id: &Pubkey, chain_id: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TRIGGER_VAULT_SEED, &[chain_id]], program_id)
}

pub fn find_chain_pda(program_id: &Pubkey, chain_id: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CHAIN_SEED, &[chain_id]], program_id)
}

pub fn find_entry_pda(
    program_id: &Pubkey,
    chain_id: u8,
    round: u32,
    position: u32,
) -> (Pubkey, u8) {
    let round_bytes = round.to_le_bytes();
    let position_bytes = position.to_le_bytes();
    Pubkey::find_program_address(
        &[ENTRY_SEED, &[chain_id], &round_bytes, &position_bytes],
        program_id,
    )
}

pub fn find_stake_tracker_pda(
    program_id: &Pubkey,
    chain_id: u8,
    round: u32,
    source_type: u8,
    source_index: u32,
) -> (Pubkey, u8) {
    let round_bytes = round.to_le_bytes();
    let source_index_bytes = source_index.to_le_bytes();
    Pubkey::find_program_address(
        &[
            STAKE_TRACKER_SEED,
            &[chain_id],
            &round_bytes,
            &[source_type],
            &source_index_bytes,
        ],
        program_id,
    )
}

pub fn find_donation_record_pda(
    program_id: &Pubkey,
    chain_id: u8,
    round: u32,
    donation_index: u16,
) -> (Pubkey, u8) {
    let round_bytes = round.to_le_bytes();
    let donation_index_bytes = donation_index.to_le_bytes();
    Pubkey::find_program_address(
        &[
            DONATION_SEED,
            &[chain_id],
            &round_bytes,
            &donation_index_bytes,
        ],
        program_id,
    )
}

pub fn find_compound_record_pda(
    program_id: &Pubkey,
    chain_id: u8,
    round: u32,
    compound_index: u16,
) -> (Pubkey, u8) {
    let round_bytes = round.to_le_bytes();
    let compound_index_bytes = compound_index.to_le_bytes();
    Pubkey::find_program_address(
        &[
            COMPOUND_SEED,
            &[chain_id],
            &round_bytes,
            &compound_index_bytes,
        ],
        program_id,
    )
}

pub fn find_chain_authority_pda(program_id: &Pubkey, chain_id: u8) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CHAIN_AUTH_SEED, &[chain_id]], program_id)
}

/// `chain_authority` has no backing account, so its bump is derived on demand
/// whenever a stake-program CPI needs signer seeds.
#[derive(Clone, Copy, Debug)]
pub struct ChainAuthoritySigner {
    chain_id: [u8; 1],
    bump: [u8; 1],
}

impl ChainAuthoritySigner {
    pub fn new(chain_id: u8, bump: u8) -> Self {
        Self {
            chain_id: [chain_id],
            bump: [bump],
        }
    }

    pub fn signer_seeds(&self) -> [&[u8]; 3] {
        [CHAIN_AUTH_SEED, &self.chain_id, &self.bump]
    }
}
