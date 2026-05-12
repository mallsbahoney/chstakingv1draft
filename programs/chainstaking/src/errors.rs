use anchor_lang::prelude::*;

#[error_code]
pub enum ChainStakingError {
    #[msg("At least one validator must be provided during protocol initialization.")]
    EmptyValidatorList,
    #[msg("ValidatorRegistry cannot store more than 16 validators.")]
    TooManyValidators,
    #[msg("ValidatorRegistry contains a duplicate validator pubkey.")]
    DuplicateValidator,
    #[msg("initialize_protocol expects 20 remaining accounts: 10 TriggerVault PDAs then 10 ChainAccount PDAs.")]
    InvalidRemainingAccountsLen,
    #[msg("The provided TriggerVault account does not match the expected PDA for its chain.")]
    InvalidTriggerVaultPda,
    #[msg("The provided ChainAccount does not match the expected PDA for its chain.")]
    InvalidChainAccountPda,
    #[msg("The PDA account must be writable.")]
    PdaAccountNotWritable,
    #[msg("The PDA account is already initialized or not system-owned.")]
    PdaAccountAlreadyInitialized,
    #[msg("Chain is currently broken - no new entries allowed until the next round.")]
    ChainIsBroken,
    #[msg("Entry amount is below the required minimum (spread or minimum entry).")]
    InsufficientEntryAmount,
    #[msg("Validator vote account is not in the approved ValidatorRegistry list.")]
    InvalidValidator,
    #[msg("Fee receiver does not match GlobalConfig.authority.")]
    InvalidFeeReceiver,
    #[msg("Chain authority PDA does not match the expected derivation.")]
    InvalidChainAuthority,
    #[msg("TriggerVault PDA does not match the expected derivation for this chain.")]
    InvalidTriggerVault,
    #[msg("exit_first_entrant requires exactly 1 entry on the chain.")]
    ExitNotAllowed,
    #[msg("Caller does not match the entry's staker.")]
    ExitWrongStaker,
    #[msg("Arithmetic overflow in checked math operation.")]
    ArithmeticOverflow,
    #[msg("Stake account has already been withdrawn.")]
    AlreadyWithdrawn,
    #[msg("Donation amount must be greater than zero.")]
    InvalidDonationAmount,
    #[msg("Donations require an active chain leader.")]
    DonationRequiresActiveLeader,
    #[msg("Donation index does not match the chain's next donation slot.")]
    InvalidDonationIndex,
    #[msg("Compound amount must be greater than zero.")]
    InvalidCompoundAmount,
    #[msg("Not enough liquid rewards are available to compound.")]
    InsufficientLiquidRewards,
    #[msg("Break type is invalid for payout computation.")]
    InvalidBreakType,
    #[msg("At least two entries are required to compute payouts.")]
    InsufficientEntriesForPayouts,
    #[msg("Entry payout inputs must be ordered by contiguous positions.")]
    PayoutEntriesOutOfOrder,
    #[msg("Payout computation received more entries than allowed in one call.")]
    TooManyPayoutEntries,
    #[msg("Yield accounting requires the same number of stake trackers and stake accounts.")]
    MismatchedStakeBalanceCount,
    #[msg("The payout principal pool must be non-zero when distributing pooled rewards.")]
    InvalidPayoutPrincipalPool,
    #[msg("Only the current chain leader (last_entry_authority) can trigger a manual break.")]
    ManualBreakUnauthorized,
    #[msg(
        "Manual break requires at least 2 entries - single entrant must use exit_first_entrant."
    )]
    ManualBreakRequiresMultipleEntries,
    #[msg("Cooldown period has not expired yet.")]
    CooldownNotExpired,
    #[msg("trigger_unstake requires a broken chain.")]
    ChainNotBroken,
    #[msg("trigger_unstake expects 1 to 5 remaining account pairs ordered as [stake_tracker, stake_account].")]
    InvalidTriggerUnstakeAccounts,
    #[msg("trigger_unstake cannot process more stake accounts than the configured batch size.")]
    TriggerUnstakeBatchTooLarge,
    #[msg("The provided StakeTracker belongs to a different chain.")]
    StakeTrackerChainMismatch,
    #[msg("The provided native stake account does not match the StakeTracker record.")]
    StakeTrackerStakeAccountMismatch,
    #[msg("The provided stake account is not owned by the native Stake Program.")]
    InvalidStakeAccountOwner,
    #[msg("Stake account has already been deactivated.")]
    StakeAlreadyDeactivated,
    #[msg("TriggerVault does not have enough reserved lamports to pay the trigger reward.")]
    InsufficientTriggerRewardReserve,
    #[msg("Stake account has not been deactivated yet — call trigger_unstake first.")]
    StakeNotDeactivated,
    #[msg("The provided StakeTracker belongs to a different round.")]
    StakeTrackerRoundMismatch,
    #[msg("compute_payouts: the chain must be broken before payouts can be computed.")]
    PayoutsRequireBrokenChain,
    #[msg("compute_payouts: the round has already been settled — payouts already computed.")]
    RoundAlreadySettled,
    #[msg("compute_payouts: remaining_accounts must contain entry_account + stake_account pairs.")]
    InvalidComputePayoutsAccounts,
    #[msg("compute_payouts: the entry account does not belong to the current chain/round.")]
    PayoutEntryMismatch,
    #[msg("withdraw_after_cooldown: round payouts have not been settled yet — call compute_payouts first.")]
    RoundNotSettled,
}
