use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, CreateAccount};

use crate::{
    constants::*,
    errors::ChainStakingError,
    state::{ChainAccount, GlobalConfig, TriggerVault, ValidatorRegistry},
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct InitializeProtocolArgs {
    pub validator_list: Vec<Pubkey>,
}

#[derive(Accounts)]
pub struct InitializeProtocol<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = GlobalConfig::SPACE,
        seeds = [GLOBAL_CONFIG_SEED],
        bump
    )]
    pub global_config: Account<'info, GlobalConfig>,
    #[account(
        init,
        payer = authority,
        space = ValidatorRegistry::SPACE,
        seeds = [VALIDATOR_REGISTRY_SEED],
        bump
    )]
    pub validator_registry: Account<'info, ValidatorRegistry>,
    pub system_program: Program<'info, System>,
}

pub fn handler<'info>(
    ctx: Context<'_, '_, '_, 'info, InitializeProtocol<'info>>,
    args: InitializeProtocolArgs,
) -> Result<()> {
    validate_validator_list(&args.validator_list)?;

    ctx.accounts.global_config.set_inner(GlobalConfig {
        authority: ctx.accounts.authority.key(),
        is_initialized: true,
        total_chains: TOTAL_CHAINS_U8,
        fee_bps: DEFAULT_FEE_BPS,
        flat_fee_lamports: DEFAULT_FLAT_FEE_LAMPORTS,
        withdrawal_fee_bps: DEFAULT_WITHDRAWAL_FEE_BPS,
        cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
        unstake_batch_size: DEFAULT_UNSTAKE_BATCH_SIZE,
        trigger_reward_lamports: DEFAULT_TRIGGER_REWARD_LAMPORTS,
        bump: ctx.bumps.global_config,
    });

    ctx.accounts
        .validator_registry
        .set_inner(ValidatorRegistry {
            validator_list: args.validator_list,
            bump: ctx.bumps.validator_registry,
        });

    initialize_per_chain_accounts(&ctx)
}

fn validate_validator_list(validator_list: &[Pubkey]) -> Result<()> {
    require!(
        !validator_list.is_empty(),
        ChainStakingError::EmptyValidatorList
    );
    require!(
        validator_list.len() <= MAX_VALIDATORS,
        ChainStakingError::TooManyValidators
    );

    for (index, validator) in validator_list.iter().enumerate() {
        for other in validator_list.iter().skip(index + 1) {
            require!(validator != other, ChainStakingError::DuplicateValidator);
        }
    }

    Ok(())
}

fn initialize_per_chain_accounts<'info>(
    ctx: &Context<'_, '_, '_, 'info, InitializeProtocol<'info>>,
) -> Result<()> {
    require!(
        ctx.remaining_accounts.len() == EXPECTED_INITIALIZE_REMAINING_ACCOUNTS,
        ChainStakingError::InvalidRemainingAccountsLen
    );

    let (trigger_vault_accounts, chain_accounts) = ctx.remaining_accounts.split_at(TOTAL_CHAINS);

    for (chain_index, spread_bps) in CHAIN_SPREADS_BPS.iter().copied().enumerate() {
        let chain_id = chain_index as u8;

        create_trigger_vault(
            ctx.program_id,
            &ctx.accounts.authority,
            &ctx.accounts.system_program,
            &trigger_vault_accounts[chain_index],
            chain_id,
        )?;

        create_chain_account(
            ctx.program_id,
            &ctx.accounts.authority,
            &ctx.accounts.system_program,
            &chain_accounts[chain_index],
            chain_id,
            spread_bps,
        )?;
    }

    Ok(())
}

fn create_trigger_vault<'info>(
    program_id: &Pubkey,
    authority: &Signer<'info>,
    system_program_program: &Program<'info, System>,
    trigger_vault_info: &AccountInfo<'info>,
    chain_id: u8,
) -> Result<()> {
    let (expected_trigger_vault, bump) = find_trigger_vault_pda(program_id, chain_id);
    require_keys_eq!(
        expected_trigger_vault,
        *trigger_vault_info.key,
        ChainStakingError::InvalidTriggerVaultPda
    );

    let chain_id_seed = [chain_id];
    let bump_seed = [bump];
    create_program_pda_account(
        authority,
        trigger_vault_info,
        system_program_program,
        &[TRIGGER_VAULT_SEED, &chain_id_seed, &bump_seed],
        TriggerVault::SPACE,
        program_id,
    )?;

    write_account_data(
        trigger_vault_info,
        &TriggerVault {
            chain_id,
            reserve_lamports: 0,
            bump,
        },
    )
}

fn create_chain_account<'info>(
    program_id: &Pubkey,
    authority: &Signer<'info>,
    system_program_program: &Program<'info, System>,
    chain_account_info: &AccountInfo<'info>,
    chain_id: u8,
    spread_bps: u32,
) -> Result<()> {
    let (expected_chain_account, bump) = find_chain_pda(program_id, chain_id);
    require_keys_eq!(
        expected_chain_account,
        *chain_account_info.key,
        ChainStakingError::InvalidChainAccountPda
    );

    let chain_id_seed = [chain_id];
    let bump_seed = [bump];
    create_program_pda_account(
        authority,
        chain_account_info,
        system_program_program,
        &[CHAIN_SEED, &chain_id_seed, &bump_seed],
        ChainAccount::SPACE,
        program_id,
    )?;

    let (_, chain_authority_bump) = find_chain_authority_pda(program_id, chain_id);

    write_account_data(
        chain_account_info,
        &ChainAccount {
            chain_id,
            spread_bps,
            entry_count: 0,
            current_round: 0,
            last_entry_amount: 0,
            last_entry_timestamp: 0,
            last_entry_authority: Pubkey::default(),
            total_staked_lamports: 0,
            frontend_estimated_yield_pool_lamports: 0,
            donation_pot_lamports: 0,
            donation_count: 0,
            is_broken: false,
            break_type: BREAK_TYPE_NONE,
            bump,
            chain_authority_bump,
            round_settled: false,
        },
    )
}

fn create_program_pda_account<'info>(
    payer: &Signer<'info>,
    target: &AccountInfo<'info>,
    system_program_program: &Program<'info, System>,
    signer_seeds: &[&[u8]],
    space: usize,
    owner: &Pubkey,
) -> Result<()> {
    require!(target.is_writable, ChainStakingError::PdaAccountNotWritable);
    require_keys_eq!(
        *target.owner,
        system_program::ID,
        ChainStakingError::PdaAccountAlreadyInitialized
    );
    require!(
        target.data_is_empty() && target.lamports() == 0,
        ChainStakingError::PdaAccountAlreadyInitialized
    );

    let minimum_balance = Rent::get()?.minimum_balance(space);
    let signer_seed_groups = &[signer_seeds];
    let cpi_accounts = CreateAccount {
        from: payer.to_account_info(),
        to: target.clone(),
    };
    let cpi_context = CpiContext::new_with_signer(
        system_program_program.to_account_info(),
        cpi_accounts,
        signer_seed_groups,
    );

    system_program::create_account(cpi_context, minimum_balance, space as u64, owner)
}

fn write_account_data<T: AccountSerialize>(
    account_info: &AccountInfo<'_>,
    account_data: &T,
) -> Result<()> {
    let mut data = account_info.try_borrow_mut_data()?;
    let mut dst: &mut [u8] = &mut data;
    account_data.try_serialize(&mut dst)
}
