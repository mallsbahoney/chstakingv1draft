use anchor_lang::{
    prelude::*,
    solana_program::{
        program::{invoke, invoke_signed},
        program_error::ProgramError,
        sysvar::{
            clock as clock_sysvar, rent::Rent, stake_history as stake_history_sysvar, Sysvar,
        },
    },
    system_program::{self, CreateAccount},
};
use solana_stake_interface::{
    config as stake_config_interface, instruction as stake_instruction,
    program as stake_program_interface,
    state::{Authorized, Lockup, StakeState},
};

use crate::constants::CHAIN_AUTH_SEED;

pub fn create_stake_account<'info>(
    payer: &AccountInfo<'info>,
    stake_account: &AccountInfo<'info>,
    rent: &AccountInfo<'info>,
    system_program_account: &AccountInfo<'info>,
    lamports: u64,
) -> Result<()> {
    require_program_account(system_program_account, &system_program::ID)?;
    require_signer(payer)?;
    require_signer(stake_account)?;
    require_writable(payer)?;
    require_writable(stake_account)?;

    let rent = Rent::from_account_info(rent)?;
    #[allow(deprecated)]
    let stake_account_size = StakeState::size_of();
    let minimum_balance = rent.minimum_balance(stake_account_size);

    if lamports < minimum_balance {
        return Err(ProgramError::InsufficientFunds.into());
    }

    let cpi_accounts = CreateAccount {
        from: payer.clone(),
        to: stake_account.clone(),
    };
    let cpi_context = CpiContext::new(system_program_account.clone(), cpi_accounts);

    system_program::create_account(
        cpi_context,
        lamports,
        stake_account_size as u64,
        &stake_program_interface::ID,
    )
}

pub fn initialize_stake_account<'info>(
    stake_account: &AccountInfo<'info>,
    chain_authority: &AccountInfo<'info>,
    rent: &AccountInfo<'info>,
    stake_program: &AccountInfo<'info>,
) -> Result<()> {
    require_program_account(stake_program, &stake_program_interface::ID)?;
    require_writable(stake_account)?;
    Rent::from_account_info(rent)?;

    let authorized = Authorized {
        staker: *chain_authority.key,
        withdrawer: *chain_authority.key,
    };
    let ix = stake_instruction::initialize(stake_account.key, &authorized, &Lockup::default());

    invoke(&ix, &[stake_account.clone(), rent.clone()]).map_err(Into::into)
}

pub fn delegate_stake_account<'info>(
    stake_account: &AccountInfo<'info>,
    validator_vote: &AccountInfo<'info>,
    chain_authority: &AccountInfo<'info>,
    clock: &AccountInfo<'info>,
    stake_history: &AccountInfo<'info>,
    stake_config: &AccountInfo<'info>,
    stake_program: &AccountInfo<'info>,
    chain_id: u8,
    chain_authority_bump: u8,
) -> Result<()> {
    require_program_account(stake_program, &stake_program_interface::ID)?;
    require_writable(stake_account)?;
    require_account_key(clock, &clock_sysvar::ID)?;
    require_account_key(stake_history, &stake_history_sysvar::ID)?;
    require_account_key(stake_config, &stake_config_interface::ID)?;

    let id_byte = [chain_id];
    let bump_byte = [chain_authority_bump];
    let seeds = chain_authority_signer_seeds_raw(&id_byte, &bump_byte);

    let ix = stake_instruction::delegate_stake(
        stake_account.key,
        chain_authority.key,
        validator_vote.key,
    );

    invoke_signed(
        &ix,
        &[
            stake_account.clone(),
            validator_vote.clone(),
            clock.clone(),
            stake_history.clone(),
            stake_config.clone(),
            chain_authority.clone(),
        ],
        &[&seeds],
    )
    .map_err(Into::into)
}

pub fn deactivate_stake_account<'info>(
    stake_account: &AccountInfo<'info>,
    chain_authority: &AccountInfo<'info>,
    clock: &AccountInfo<'info>,
    stake_history: &AccountInfo<'info>,
    stake_program: &AccountInfo<'info>,
    chain_id: u8,
    chain_authority_bump: u8,
) -> Result<()> {
    require_program_account(stake_program, &stake_program_interface::ID)?;
    require_writable(stake_account)?;
    require_account_key(clock, &clock_sysvar::ID)?;
    require_account_key(stake_history, &stake_history_sysvar::ID)?;

    let id_byte = [chain_id];
    let bump_byte = [chain_authority_bump];
    let seeds = chain_authority_signer_seeds_raw(&id_byte, &bump_byte);

    deactivate_stake_account_with_signer_seeds(
        stake_account,
        chain_authority,
        clock,
        stake_history,
        stake_program,
        &seeds,
    )
}

pub fn deactivate_stake_account_with_signer_seeds<'info>(
    stake_account: &AccountInfo<'info>,
    chain_authority: &AccountInfo<'info>,
    clock: &AccountInfo<'info>,
    stake_history: &AccountInfo<'info>,
    stake_program: &AccountInfo<'info>,
    signer_seeds: &[&[u8]],
) -> Result<()> {
    require_program_account(stake_program, &stake_program_interface::ID)?;
    require_writable(stake_account)?;
    require_account_key(clock, &clock_sysvar::ID)?;
    require_account_key(stake_history, &stake_history_sysvar::ID)?;

    let ix = stake_instruction::deactivate_stake(stake_account.key, chain_authority.key);

    invoke_signed(
        &ix,
        &[
            stake_account.clone(),
            clock.clone(),
            chain_authority.clone(),
        ],
        &[signer_seeds],
    )
    .map_err(Into::into)
}

pub fn withdraw_stake_account<'info>(
    stake_account: &AccountInfo<'info>,
    recipient: &AccountInfo<'info>,
    chain_authority: &AccountInfo<'info>,
    clock: &AccountInfo<'info>,
    stake_history: &AccountInfo<'info>,
    stake_program: &AccountInfo<'info>,
    chain_id: u8,
    chain_authority_bump: u8,
) -> Result<()> {
    require_program_account(stake_program, &stake_program_interface::ID)?;
    require_writable(stake_account)?;
    require_writable(recipient)?;
    require_account_key(clock, &clock_sysvar::ID)?;
    require_account_key(stake_history, &stake_history_sysvar::ID)?;

    let id_byte = [chain_id];
    let bump_byte = [chain_authority_bump];
    let seeds = chain_authority_signer_seeds_raw(&id_byte, &bump_byte);
    let lamports = stake_account.lamports();

    if lamports == 0 {
        return Err(ProgramError::InsufficientFunds.into());
    }

    let ix = stake_instruction::withdraw(
        stake_account.key,
        chain_authority.key,
        recipient.key,
        lamports,
        None,
    );

    invoke_signed(
        &ix,
        &[
            stake_account.clone(),
            recipient.clone(),
            clock.clone(),
            stake_history.clone(),
            chain_authority.clone(),
        ],
        &[&seeds],
    )
    .map_err(Into::into)
}

/// Constructs the `invoke_signed` signer seeds for a `chain_authority` PDA.
///
/// Seeds: `[b"chain_auth", &[chain_id], &[chain_authority_bump]]`
///
/// Both values must be sourced from `ChainAccount` (stored at protocol init).
/// The caller must keep the byte arrays alive for the duration of `invoke_signed`:
///
/// ```rust
/// let id_byte   = [chain_account.chain_id];
/// let bump_byte = [chain_account.chain_authority_bump];
/// let seeds = chain_authority_signer_seeds_raw(&id_byte, &bump_byte);
/// invoke_signed(&ix, accounts, &[&seeds])?;
/// ```
pub fn chain_authority_signer_seeds_raw<'a>(
    chain_id_byte: &'a [u8],
    bump_byte: &'a [u8],
) -> [&'a [u8]; 3] {
    [CHAIN_AUTH_SEED, chain_id_byte, bump_byte]
}

fn require_program_account(account: &AccountInfo<'_>, expected: &Pubkey) -> Result<()> {
    if account.key != expected || !account.executable {
        return Err(ProgramError::IncorrectProgramId.into());
    }

    Ok(())
}

fn require_account_key(account: &AccountInfo<'_>, expected: &Pubkey) -> Result<()> {
    if account.key != expected {
        return Err(ProgramError::InvalidArgument.into());
    }

    Ok(())
}

fn require_signer(account: &AccountInfo<'_>) -> Result<()> {
    if !account.is_signer {
        return Err(ProgramError::MissingRequiredSignature.into());
    }

    Ok(())
}

fn require_writable(account: &AccountInfo<'_>) -> Result<()> {
    if !account.is_writable {
        return Err(ProgramError::InvalidArgument.into());
    }

    Ok(())
}
