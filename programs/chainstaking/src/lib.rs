use anchor_lang::prelude::*;

pub mod constants;
pub mod errors;
pub mod instructions;
pub mod stake_utils;
pub mod state;
pub mod yield_utils;

pub use instructions::*;

declare_id!("A4XAJ3tE5HYmCSQkfbgzGSdTjrsudMEy6EUqoYLjXRCY");

#[program]
pub mod chainstaking {
    use super::*;

    pub fn initialize_protocol<'info>(
        ctx: Context<'_, '_, '_, 'info, InitializeProtocol<'info>>,
        args: InitializeProtocolArgs,
    ) -> Result<()> {
        instructions::initialize_protocol::handler(ctx, args)
    }

    pub fn enter_chain(ctx: Context<EnterChain>, args: EnterChainArgs) -> Result<()> {
        instructions::enter_chain::handler(ctx, args)
    }

    pub fn exit_first_entrant(ctx: Context<ExitFirstEntrant>, chain_id: u8) -> Result<()> {
        instructions::exit_first_entrant::handler(ctx, chain_id)
    }

    pub fn donate_to_chain(ctx: Context<DonateToChain>, args: DonateToChainArgs) -> Result<()> {
        instructions::donate_to_chain::handler(ctx, args)
    }

    pub fn compound_rewards(
        ctx: Context<CompoundRewards>,
        args: CompoundRewardsArgs,
    ) -> Result<()> {
        instructions::compound_rewards::handler(ctx, args)
    }

    pub fn manual_break_chain(ctx: Context<ManualBreakChain>, chain_id: u8) -> Result<()> {
        instructions::manual_break_chain::handler(ctx, chain_id)
    }

    pub fn compute_payouts<'info>(
        ctx: Context<'_, '_, 'info, 'info, ComputePayouts<'info>>,
        args: ComputePayoutsArgs,
    ) -> Result<()> {
        instructions::compute_payouts::handler(ctx, args)
    }

    pub fn trigger_unstake<'info>(
        ctx: Context<'_, '_, 'info, 'info, TriggerUnstake<'info>>,
        chain_id: u8,
    ) -> Result<()> {
        instructions::trigger_unstake::handler(ctx, chain_id)
    }

    pub fn withdraw_after_cooldown(
        ctx: Context<WithdrawAfterCooldown>,
        args: WithdrawAfterCooldownArgs,
    ) -> Result<()> {
        instructions::withdraw_after_cooldown::handler(ctx, args)
    }
}
