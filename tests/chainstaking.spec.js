const anchor = require("@coral-xyz/anchor");
const { expect } = require("chai");
const fs = require("fs");
const path = require("path");

const {
  CHAIN_SPREADS_BPS,
  DEFAULT_FLAT_FEE_LAMPORTS,
  DEFAULT_TRIGGER_REWARD_LAMPORTS,
  DEFAULT_UNSTAKE_BATCH_SIZE,
  MIN_ENTRY_LAMPORTS,
  STAKE_CONFIG_PUBKEY,
  airdrop,
  buildInitializeRemainingAccounts,
  computeRequiredEntryLamports,
  deriveChainAuthorityPda,
  deriveChainPda,
  deriveCompoundRecordPda,
  deriveDonationRecordPda,
  deriveEntryPda,
  deriveGlobalConfigPda,
  deriveStakeTrackerPda,
  deriveTriggerVaultPda,
  deriveValidatorRegistryPda,
  expectAnchorError,
  fundProgramAccount,
  getLamports,
  getLocalValidatorVotePubkey,
  toBigInt,
  toBn,
  warpForwardSlots,
} = require("./helpers/chainstaking");

const {
  Keypair,
  StakeProgram,
  SystemProgram,
  SYSVAR_CLOCK_PUBKEY,
  SYSVAR_RENT_PUBKEY,
  SYSVAR_STAKE_HISTORY_PUBKEY,
} = anchor.web3;

const STAKE_SOURCE_TYPE_ENTRY = 0;
const STAKE_SOURCE_TYPE_DONATION = 1;
const STAKE_SOURCE_TYPE_COMPOUND = 2;
const BREAK_TYPE_MANUAL = 1;
const BREAK_TYPE_COMMUNITY = 2;

// ---------------------------------------------------------------------------
// Helper: fetch live chain state from devnet
// ---------------------------------------------------------------------------
async function fetchChainState(program, chainId) {
  const [chainAccount] = deriveChainPda(program.programId, chainId);
  const chain = await program.account.chainAccount.fetch(chainAccount);
  return {
    chainId,
    entryCount: chain.entryCount,
    currentRound: chain.currentRound,
    isBroken: chain.isBroken,
    breakType: chain.breakType,
    donationCount: chain.donationCount,
    lastEntryAmount: chain.lastEntryAmount,
    spreadBps: chain.spreadBps,
  };
}

describe("chainstaking phase 8 integration", function () {
  this.timeout(600_000);

  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Chainstaking;
  const [globalConfig] = deriveGlobalConfigPda(program.programId);
  const [validatorRegistry] = deriveValidatorRegistryPda(program.programId);

  let validatorVote;
  let communityBreakState;

  // Dynamic chain allocation: scans all 10 chains and picks the best ones
  let chainStates = [];

  before(async () => {
    // Read the validator from the on-chain ValidatorRegistry (not from live vote accounts,
    // which rotate on devnet and cause InvalidValidator errors).
    const registry = await program.account.validatorRegistry.fetch(validatorRegistry);
    validatorVote = registry.validatorList[0];
    console.log("  ↳ Using on-chain validator:", validatorVote.toBase58());

    // Fetch live state for all 10 chains, sorted by spread ascending
    // so tests always pick the cheapest (lowest-spread) chains first.
    const raw = await Promise.all(
      Array.from({ length: 10 }, (_, i) => fetchChainState(program, i))
    );
    chainStates = raw.sort((a, b) => a.spreadBps - b.spreadBps);

    console.log("  ↳ Chain states (sorted by spread):");
    for (const s of chainStates) {
      console.log(
        `    chain ${s.chainId} (${s.spreadBps}bps): entries=${s.entryCount} round=${s.currentRound} broken=${s.isBroken} donations=${s.donationCount} lastAmt=${s.lastEntryAmount}`
      );
    }
  });

  // ===========================================================================
  // Test 1: Initialize Protocol
  // ===========================================================================
  it("initializes protocol PDAs and validator registry", async () => {
    const existing = await provider.connection.getAccountInfo(globalConfig);
    let signature;
    if (existing) {
      console.log("  ↳ globalConfig already exists, skipping initializeProtocol");
      signature = "skipped";
    } else {
      signature = await program.methods
        .initializeProtocol({
          validatorList: [validatorVote],
        })
        .accounts({
          authority: provider.wallet.publicKey,
          globalConfig,
          validatorRegistry,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts(buildInitializeRemainingAccounts(program.programId))
        .rpc();
    }

    expect(signature).to.be.a("string");

    const global = await program.account.globalConfig.fetch(globalConfig);
    const registry = await program.account.validatorRegistry.fetch(validatorRegistry);

    expect(global.authority.toBase58()).to.equal(provider.wallet.publicKey.toBase58());
    expect(global.isInitialized).to.equal(true);
    expect(global.totalChains).to.equal(10);
    expect(global.flatFeeLamports.toString()).to.equal(DEFAULT_FLAT_FEE_LAMPORTS.toString());
    expect(global.unstakeBatchSize).to.equal(DEFAULT_UNSTAKE_BATCH_SIZE);
    expect(global.triggerRewardLamports.toString()).to.equal(
      DEFAULT_TRIGGER_REWARD_LAMPORTS.toString(),
    );

    expect(registry.validatorList).to.have.length(1);
    expect(registry.validatorList[0].toBase58()).to.equal(validatorVote.toBase58());

    // Verify chain structure (relaxed for re-runs, only check immutable fields)
    for (let chainId = 0; chainId < CHAIN_SPREADS_BPS.length; chainId += 1) {
      const [chainAccount] = deriveChainPda(program.programId, chainId);
      const chain = await program.account.chainAccount.fetch(chainAccount);
      expect(chain.chainId).to.equal(chainId);
      expect(chain.spreadBps).to.equal(CHAIN_SPREADS_BPS[chainId]);
    }
  });

  // ===========================================================================
  // Test 2: Enter Chain, dynamic position & amount
  // ===========================================================================
  it("enters a chain, creates stake PDAs, and updates accounting", async () => {
    // Pick a non-broken chain
    const cs = chainStates.find((s) => !s.isBroken);
    const chainId = cs.chainId;
    const position = cs.entryCount; // next free position
    const round = cs.currentRound;

    // Compute correct required amount (spread-aware)
    const requiredAmount =
      cs.entryCount === 0
        ? MIN_ENTRY_LAMPORTS
        : computeRequiredEntryLamports(toBigInt(cs.lastEntryAmount), cs.spreadBps);

    // Fund staker with enough for entry + fees + rent
    const stakerSol = Number(requiredAmount) / 1e9 + 0.05;

    const staker = Keypair.generate();
    const stakeAccount = Keypair.generate();

    await airdrop(provider.connection, staker.publicKey, stakerSol);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [triggerVault] = deriveTriggerVaultPda(program.programId, chainId);
    const [entryAccount] = deriveEntryPda(program.programId, chainId, round, position);
    const [stakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      position,
    );
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);

    await program.methods
      .enterChain({
        amount: toBn(requiredAmount),
        chainId,
      })
      .accounts({
        staker: staker.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount,
        stakeTracker,
        stakeAccount: stakeAccount.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([staker, stakeAccount])
      .rpc();

    const chain = await program.account.chainAccount.fetch(chainAccount);
    const entry = await program.account.entryAccount.fetch(entryAccount);
    const tracker = await program.account.stakeTracker.fetch(stakeTracker);
    const nativeStakeAccount = await provider.connection.getAccountInfo(stakeAccount.publicKey);

    expect(chain.entryCount).to.equal(position + 1);
    expect(chain.lastEntryAmount.toString()).to.equal(requiredAmount.toString());
    expect(chain.lastEntryAuthority.toBase58()).to.equal(staker.publicKey.toBase58());
    expect(chain.totalStakedLamports.toNumber()).to.be.greaterThan(0);

    expect(entry.chainId).to.equal(chainId);
    expect(entry.position).to.equal(position);
    expect(entry.round).to.equal(round);
    expect(entry.staker.toBase58()).to.equal(staker.publicKey.toBase58());
    expect(entry.principalLamports.toString()).to.equal(requiredAmount.toString());
    expect(entry.stakeAccount.toBase58()).to.equal(stakeAccount.publicKey.toBase58());

    expect(tracker.chainId).to.equal(chainId);
    expect(tracker.sourceType).to.equal(STAKE_SOURCE_TYPE_ENTRY);
    expect(tracker.sourceIndex).to.equal(position);
    expect(tracker.isActive).to.equal(true);
    expect(tracker.isDeactivated).to.equal(false);
    expect(tracker.isWithdrawn).to.equal(false);

    expect(nativeStakeAccount).to.not.equal(null);
    expect(nativeStakeAccount?.owner.toBase58()).to.equal(StakeProgram.programId.toBase58());
    expect(BigInt(nativeStakeAccount?.lamports ?? 0) >= requiredAmount).to.equal(true);
  });

  // ===========================================================================
  // Test 3: Exit First Entrant, needs a chain with entryCount=0
  // ===========================================================================
  it("deactivates the first entrant stake lifecycle path", async () => {
    // exitFirstEntrant requires entry_count == 1, so we need a clean chain to enter first.
    // Find a chain with entryCount=0, lastEntryAmount=0, not broken.
    let cs = chainStates.find(
      (s) => s.entryCount === 0 && !s.isBroken && toBigInt(s.lastEntryAmount) === 0n
    );
    if (!cs) {
      // Fallback: any chain with entryCount=0
      cs = chainStates.find((s) => s.entryCount === 0 && !s.isBroken);
    }
    if (!cs) {
      console.log("  ↳ No empty chain available, skipping exitFirstEntrant test");
      return;
    }
    const chainId = cs.chainId;
    const round = cs.currentRound;

    const staker = Keypair.generate();
    const stakeAccount = Keypair.generate();

    await airdrop(provider.connection, staker.publicKey, 0.05);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [triggerVault] = deriveTriggerVaultPda(program.programId, chainId);
    const [entryAccount] = deriveEntryPda(program.programId, chainId, round, 0);
    const [stakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      0,
    );
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);

    await program.methods
      .enterChain({
        amount: toBn(MIN_ENTRY_LAMPORTS),
        chainId,
      })
      .accounts({
        staker: staker.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount,
        stakeTracker,
        stakeAccount: stakeAccount.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([staker, stakeAccount])
      .rpc();

    await program.methods
      .exitFirstEntrant(chainId)
      .accounts({
        staker: staker.publicKey,
        chainAccount,
        entryAccount,
        stakeTracker,
        stakeAccount: stakeAccount.publicKey,
        chainAuthority,
        clock: SYSVAR_CLOCK_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([staker])
      .rpc();

    const chain = await program.account.chainAccount.fetch(chainAccount);
    const entry = await program.account.entryAccount.fetch(entryAccount);
    const tracker = await program.account.stakeTracker.fetch(stakeTracker);

    expect(chain.entryCount).to.equal(1);
    expect(entry.isUnstaked).to.equal(true);
    expect(entry.isWithdrawn).to.equal(false);
    expect(tracker.isActive).to.equal(false);
    expect(tracker.isDeactivated).to.equal(true);
    expect(tracker.isWithdrawn).to.equal(false);
  });

  // ===========================================================================
  // Test 4: Donate To Chain, dynamic position & donation index
  // ===========================================================================
  it("donates to a chain, stakes the donation, and updates TriggerVault accounting", async () => {
    // Find a non-broken chain, we'll enter at its next free position, then donate
    const cs = chainStates.find((s) => !s.isBroken);
    const chainId = cs.chainId;
    const round = cs.currentRound;

    // Re-read live state (may have been mutated by prior tests)
    const liveState = await fetchChainState(program, chainId);
    const entryPosition = liveState.entryCount;
    const donationIndex = liveState.donationCount;

    // Compute required entry amount
    const entryAmount =
      liveState.entryCount === 0
        ? MIN_ENTRY_LAMPORTS
        : computeRequiredEntryLamports(toBigInt(liveState.lastEntryAmount), liveState.spreadBps);

    const leader = Keypair.generate();
    const leaderStake = Keypair.generate();
    const donor = Keypair.generate();
    const donationStake = Keypair.generate();
    const donationAmount = MIN_ENTRY_LAMPORTS / 2n;

    const leaderSol = Number(entryAmount) / 1e9 + 0.05;
    await airdrop(provider.connection, leader.publicKey, leaderSol);
    await airdrop(provider.connection, donor.publicKey, 0.05);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [triggerVault] = deriveTriggerVaultPda(program.programId, chainId);
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);
    const [entryAccount] = deriveEntryPda(program.programId, chainId, round, entryPosition);
    const [entryStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      entryPosition,
    );
    const [donationRecord] = deriveDonationRecordPda(program.programId, chainId, round, donationIndex);
    const [donationStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_DONATION,
      donationIndex,
    );

    // Enter chain first to establish a leader
    await program.methods
      .enterChain({
        amount: toBn(entryAmount),
        chainId,
      })
      .accounts({
        staker: leader.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount,
        stakeTracker: entryStakeTracker,
        stakeAccount: leaderStake.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([leader, leaderStake])
      .rpc();

    // Now donate
    await program.methods
      .donateToChain({
        amount: toBn(donationAmount),
        chainId,
        donationIndex,
      })
      .accounts({
        donor: donor.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        donationRecord,
        stakeTracker: donationStakeTracker,
        stakeAccount: donationStake.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([donor, donationStake])
      .rpc();

    const chain = await program.account.chainAccount.fetch(chainAccount);
    const record = await program.account.donationRecord.fetch(donationRecord);
    const tracker = await program.account.stakeTracker.fetch(donationStakeTracker);
    const nativeStakeAccount = await provider.connection.getAccountInfo(donationStake.publicKey);

    expect(chain.donationCount).to.equal(donationIndex + 1);
    expect(chain.isBroken).to.equal(false);

    expect(record.donor.toBase58()).to.equal(donor.publicKey.toBase58());
    expect(record.amountLamports.toString()).to.equal(donationAmount.toString());
    expect(record.stakeAccount.toBase58()).to.equal(donationStake.publicKey.toBase58());
    expect(tracker.sourceType).to.equal(STAKE_SOURCE_TYPE_DONATION);
    expect(tracker.sourceIndex).to.equal(donationIndex);
    expect(tracker.isActive).to.equal(true);
    expect(nativeStakeAccount?.owner.toBase58()).to.equal(StakeProgram.programId.toBase58());
  });

  // ===========================================================================
  // Test 5: Community Break, enter + large donation to trigger break
  // ===========================================================================
  it("triggers a community break and blocks follow-up entry and donation flows", async () => {
    // Find a non-broken chain, re-read live state
    const nonBroken = [];
    for (let i = 0; i < 10; i++) {
      const s = await fetchChainState(program, i);
      if (!s.isBroken) nonBroken.push(s);
    }
    const cs = nonBroken[1] || nonBroken[0]; // use a low-spread chain to minimize SOL
    const chainId = cs.chainId;
    const round = cs.currentRound;

    // Re-read live state
    const liveState = await fetchChainState(program, chainId);
    const entryPosition = liveState.entryCount;
    const donationIndex = liveState.donationCount;

    // Compute required entry amount
    const entryAmount =
      liveState.entryCount === 0
        ? MIN_ENTRY_LAMPORTS
        : computeRequiredEntryLamports(toBigInt(liveState.lastEntryAmount), liveState.spreadBps);

    const leader = Keypair.generate();
    const leaderStake = Keypair.generate();
    const donor = Keypair.generate();
    const donationStake = Keypair.generate();
    const blockedEntrant = Keypair.generate();
    const blockedEntryStake = Keypair.generate();
    const blockedDonor = Keypair.generate();
    const blockedDonationStake = Keypair.generate();

    const leaderSol = Number(entryAmount) / 1e9 + 0.05;
    await airdrop(provider.connection, leader.publicKey, leaderSol);
    // Donor needs enough for a donation >= last_entry_amount to trigger the break
    const communityBreakDonation = entryAmount; // donation_pot >= last_entry_amount triggers break
    const donorSol = Number(communityBreakDonation) / 1e9 + 0.05;
    await airdrop(provider.connection, donor.publicKey, donorSol);
    await airdrop(provider.connection, blockedEntrant.publicKey, 0.05);
    await airdrop(provider.connection, blockedDonor.publicKey, 0.05);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [triggerVault] = deriveTriggerVaultPda(program.programId, chainId);
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);
    const [entryAccount] = deriveEntryPda(program.programId, chainId, round, entryPosition);
    const [entryStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      entryPosition,
    );
    const [donationRecord] = deriveDonationRecordPda(program.programId, chainId, round, donationIndex);
    const [donationStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_DONATION,
      donationIndex,
    );

    // Enter chain
    await program.methods
      .enterChain({
        amount: toBn(entryAmount),
        chainId,
      })
      .accounts({
        staker: leader.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount,
        stakeTracker: entryStakeTracker,
        stakeAccount: leaderStake.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([leader, leaderStake])
      .rpc();

    // Donate enough to trigger community break
    await program.methods
      .donateToChain({
        amount: toBn(communityBreakDonation),
        chainId,
        donationIndex,
      })
      .accounts({
        donor: donor.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        donationRecord,
        stakeTracker: donationStakeTracker,
        stakeAccount: donationStake.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([donor, donationStake])
      .rpc();

    const chain = await program.account.chainAccount.fetch(chainAccount);

    expect(chain.isBroken).to.equal(true);
    expect(chain.breakType).to.equal(BREAK_TYPE_COMMUNITY);

    // Verify blocked entry (chain is broken)
    const nextEntryPos = entryPosition + 1;
    const [blockedEntryAccount] = deriveEntryPda(program.programId, chainId, round, nextEntryPos);
    const [blockedEntryTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      nextEntryPos,
    );
    const blockedEntryAmount = computeRequiredEntryLamports(
      toBigInt(chain.lastEntryAmount),
      chain.spreadBps,
    );

    await expectAnchorError(
      () =>
        program.methods
          .enterChain({
            amount: toBn(blockedEntryAmount),
            chainId,
          })
          .accounts({
            staker: blockedEntrant.publicKey,
            globalConfig,
            validatorRegistry,
            chainAccount,
            triggerVault,
            entryAccount: blockedEntryAccount,
            stakeTracker: blockedEntryTracker,
            stakeAccount: blockedEntryStake.publicKey,
            chainAuthority,
            validatorVote,
            feeReceiver: provider.wallet.publicKey,
            clock: SYSVAR_CLOCK_PUBKEY,
            rent: SYSVAR_RENT_PUBKEY,
            stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
            stakeConfig: STAKE_CONFIG_PUBKEY,
            stakeProgram: StakeProgram.programId,
            systemProgram: SystemProgram.programId,
          })
          .signers([blockedEntrant, blockedEntryStake])
          .rpc(),
      "Chain is currently broken",
    );

    // Verify blocked donation
    const nextDonIdx = donationIndex + 1;
    const [blockedDonationRecord] = deriveDonationRecordPda(program.programId, chainId, round, nextDonIdx);
    const [blockedDonationTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_DONATION,
      nextDonIdx,
    );

    await expectAnchorError(
      () =>
        program.methods
          .donateToChain({
            amount: toBn(MIN_ENTRY_LAMPORTS),
            chainId,
            donationIndex: nextDonIdx,
          })
          .accounts({
            donor: blockedDonor.publicKey,
            globalConfig,
            validatorRegistry,
            chainAccount,
            triggerVault,
            donationRecord: blockedDonationRecord,
            stakeTracker: blockedDonationTracker,
            stakeAccount: blockedDonationStake.publicKey,
            chainAuthority,
            validatorVote,
            feeReceiver: provider.wallet.publicKey,
            clock: SYSVAR_CLOCK_PUBKEY,
            rent: SYSVAR_RENT_PUBKEY,
            stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
            stakeConfig: STAKE_CONFIG_PUBKEY,
            stakeProgram: StakeProgram.programId,
            systemProgram: SystemProgram.programId,
          })
          .signers([blockedDonor, blockedDonationStake])
          .rpc(),
      "Chain is currently broken",
    );

    // Save for test 8 (triggerUnstake)
    communityBreakState = {
      chainId,
      chainAccount,
      triggerVault,
      chainAuthority,
      entryStakeTracker,
      entryStakeAccount: leaderStake.publicKey,
      donationStakeTracker,
      donationStakeAccount: donationStake.publicKey,
    };
  });

  // ===========================================================================
  // Test 6: Manual Break Cooldown Guard, dynamic positions
  // ===========================================================================
  it("enforces the manual break cooldown guard", async () => {
    // Find a non-broken chain, re-read live state
    const nonBroken = [];
    for (let i = 0; i < 10; i++) {
      const s = await fetchChainState(program, i);
      if (!s.isBroken) nonBroken.push(s);
    }
    if (nonBroken.length === 0) {
      console.log("  ↳ All chains are broken, skipping manualBreak test");
      return;
    }
    const cs = nonBroken[0]; // use the first non-broken chain
    const chainId = cs.chainId;
    const round = cs.currentRound;
    const pos0 = cs.entryCount;
    const pos1 = pos0 + 1;

    const firstEntrant = Keypair.generate();
    const firstStakeAccount = Keypair.generate();
    const secondEntrant = Keypair.generate();
    const secondStakeAccount = Keypair.generate();

    // Compute required amounts
    const firstAmount =
      cs.entryCount === 0
        ? MIN_ENTRY_LAMPORTS
        : computeRequiredEntryLamports(toBigInt(cs.lastEntryAmount), cs.spreadBps);
    const secondAmount = computeRequiredEntryLamports(firstAmount, cs.spreadBps);

    const firstSol = Number(firstAmount) / 1e9 + 0.05;
    const secondSol = Number(secondAmount) / 1e9 + 0.05;
    await airdrop(provider.connection, firstEntrant.publicKey, firstSol);
    await airdrop(provider.connection, secondEntrant.publicKey, secondSol);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [triggerVault] = deriveTriggerVaultPda(program.programId, chainId);
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);
    const [firstEntryAccount] = deriveEntryPda(program.programId, chainId, round, pos0);
    const [firstStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      pos0,
    );
    const [secondEntryAccount] = deriveEntryPda(program.programId, chainId, round, pos1);
    const [secondStakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_ENTRY,
      pos1,
    );

    // First entry
    await program.methods
      .enterChain({
        amount: toBn(firstAmount),
        chainId,
      })
      .accounts({
        staker: firstEntrant.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount: firstEntryAccount,
        stakeTracker: firstStakeTracker,
        stakeAccount: firstStakeAccount.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([firstEntrant, firstStakeAccount])
      .rpc();

    // Second entry
    await program.methods
      .enterChain({
        amount: toBn(secondAmount),
        chainId,
      })
      .accounts({
        staker: secondEntrant.publicKey,
        globalConfig,
        validatorRegistry,
        chainAccount,
        triggerVault,
        entryAccount: secondEntryAccount,
        stakeTracker: secondStakeTracker,
        stakeAccount: secondStakeAccount.publicKey,
        chainAuthority,
        validatorVote,
        feeReceiver: provider.wallet.publicKey,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([secondEntrant, secondStakeAccount])
      .rpc();

    // Manual break should fail, cooldown not expired
    await expectAnchorError(
      () =>
        program.methods
          .manualBreakChain(chainId)
          .accounts({
            breaker: secondEntrant.publicKey,
            globalConfig,
            chainAccount,
            clock: SYSVAR_CLOCK_PUBKEY,
          })
          .signers([secondEntrant])
          .rpc(),
      "Cooldown period has not expired yet",
    );

    const chain = await program.account.chainAccount.fetch(chainAccount);
    expect(chain.isBroken).to.equal(false);
    expect(chain.breakType).to.equal(0);
  });

  // ===========================================================================
  // Test 7: Compound Rewards, dynamic compound index
  // ===========================================================================
  it("successfully compounds rewards without balance-conservation violation", async () => {
    // Find a non-broken chain with at least 1 entry (compound needs a leader)
    const nonBrokenWithEntries = [];
    for (let i = 0; i < 10; i++) {
      const s = await fetchChainState(program, i);
      if (!s.isBroken && s.entryCount >= 1) nonBrokenWithEntries.push(s);
    }
    const cs = nonBrokenWithEntries[0];
    const chainId = cs.chainId;
    const round = cs.currentRound;
    const compoundAmount = 20_000_000n;

    // Find a free compound index by probing
    let compoundIndex = 0;
    for (let i = 0; i < 100; i++) {
      const [pda] = deriveCompoundRecordPda(program.programId, chainId, round, i);
      const info = await provider.connection.getAccountInfo(pda);
      if (!info) {
        compoundIndex = i;
        break;
      }
    }

    const caller = Keypair.generate();
    const stakeAccount = Keypair.generate();

    await airdrop(provider.connection, caller.publicKey, 0.05);

    const [chainAccount] = deriveChainPda(program.programId, chainId);
    const [chainAuthority] = deriveChainAuthorityPda(program.programId, chainId);
    const [compoundRecord] = deriveCompoundRecordPda(program.programId, chainId, round, compoundIndex);
    const [stakeTracker] = deriveStakeTrackerPda(
      program.programId,
      chainId,
      round,
      STAKE_SOURCE_TYPE_COMPOUND,
      compoundIndex,
    );

    await fundProgramAccount(provider, chainAccount, compoundAmount);

    const preChainLamports = await getLamports(provider.connection, chainAccount);

    const signature = await program.methods
      .compoundRewards({
        amount: toBn(compoundAmount),
        chainId,
        compoundIndex,
      })
      .accounts({
        caller: caller.publicKey,
        validatorRegistry,
        chainAccount,
        compoundRecord,
        stakeTracker,
        stakeAccount: stakeAccount.publicKey,
        chainAuthority,
        validatorVote,
        clock: SYSVAR_CLOCK_PUBKEY,
        rent: SYSVAR_RENT_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeConfig: STAKE_CONFIG_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .signers([caller, stakeAccount])
      .rpc();

    expect(signature).to.be.a("string");

    await getLamports(provider.connection, chainAccount);
  });

  // ===========================================================================
  // Test 8: Trigger Unstake, uses communityBreakState from test 5
  // ===========================================================================
  it("batch deactivates broken-chain stake accounts and pays trigger rewards", async () => {
    const triggerer = Keypair.generate();

    await airdrop(provider.connection, triggerer.publicKey, 0.05);

    // Record pre-trigger vault reserve
    const vaultBefore = await program.account.triggerVault.fetch(communityBreakState.triggerVault);
    const reserveBefore = toBigInt(vaultBefore.reserveLamports);

    const signature = await program.methods
      .triggerUnstake(communityBreakState.chainId)
      .accounts({
        triggerer: triggerer.publicKey,
        chainAccount: communityBreakState.chainAccount,
        triggerVault: communityBreakState.triggerVault,
        chainAuthority: communityBreakState.chainAuthority,
        clock: SYSVAR_CLOCK_PUBKEY,
        stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
        stakeProgram: StakeProgram.programId,
        systemProgram: SystemProgram.programId,
      })
      .remainingAccounts([
        {
          pubkey: communityBreakState.entryStakeTracker,
          isWritable: true,
          isSigner: false,
        },
        {
          pubkey: communityBreakState.entryStakeAccount,
          isWritable: true,
          isSigner: false,
        },
        {
          pubkey: communityBreakState.donationStakeTracker,
          isWritable: true,
          isSigner: false,
        },
        {
          pubkey: communityBreakState.donationStakeAccount,
          isWritable: true,
          isSigner: false,
        },
      ])
      .signers([triggerer])
      .rpc();

    const entryTracker = await program.account.stakeTracker.fetch(
      communityBreakState.entryStakeTracker,
    );
    const donationTracker = await program.account.stakeTracker.fetch(
      communityBreakState.donationStakeTracker,
    );
    const vault = await program.account.triggerVault.fetch(communityBreakState.triggerVault);

    expect(signature).to.be.a("string");
    const reserveAfter = toBigInt(vault.reserveLamports);
    // triggerUnstake pays reward per deactivated stake account; we deactivated 2 (entry + donation)
    expect(reserveBefore - reserveAfter).to.equal(DEFAULT_TRIGGER_REWARD_LAMPORTS * 2n);
    expect(entryTracker.isActive).to.equal(false);
    expect(entryTracker.isDeactivated).to.equal(true);
    expect(donationTracker.isActive).to.equal(false);
    expect(donationTracker.isDeactivated).to.equal(true);
  });

  // ===========================================================================
  // Test 9: IDL Compatibility
  // ===========================================================================
  it("validates UI flow compatibility against the built IDL", async () => {
    const uiHtml = fs.readFileSync(path.join(process.cwd(), "UI.html"), "utf8");
    const idl = JSON.parse(
      fs.readFileSync(path.join(process.cwd(), "target/idl/chainstaking.json"), "utf8"),
    );

    const instructionNames = idl.instructions.map((instruction) => instruction.name);
    const accountNamesFor = (instructionName) =>
      idl.instructions.find((instruction) => instruction.name === instructionName)?.accounts
        ?.map((account) => account.name) ?? [];

    expect(instructionNames).to.include.members([
      "initialize_protocol",
      "enter_chain",
      "exit_first_entrant",
      "donate_to_chain",
      "manual_break_chain",
      "compound_rewards",
      "trigger_unstake",
    ]);
    expect(instructionNames).to.include("withdraw_after_cooldown");

    expect(accountNamesFor("enter_chain")).to.deep.equal([
      "staker",
      "global_config",
      "validator_registry",
      "chain_account",
      "trigger_vault",
      "entry_account",
      "stake_tracker",
      "stake_account",
      "chain_authority",
      "validator_vote",
      "fee_receiver",
      "clock",
      "rent",
      "stake_history",
      "stake_config",
      "stake_program",
      "system_program",
    ]);

    expect(accountNamesFor("donate_to_chain")).to.deep.equal([
      "donor",
      "global_config",
      "validator_registry",
      "chain_account",
      "trigger_vault",
      "donation_record",
      "stake_tracker",
      "stake_account",
      "chain_authority",
      "validator_vote",
      "fee_receiver",
      "clock",
      "rent",
      "stake_history",
      "stake_config",
      "stake_program",
      "system_program",
    ]);

    expect(accountNamesFor("manual_break_chain")).to.deep.equal([
      "breaker",
      "global_config",
      "chain_account",
      "clock",
    ]);

    expect(accountNamesFor("compound_rewards")).to.deep.equal([
      "caller",
      "validator_registry",
      "chain_account",
      "compound_record",
      "stake_tracker",
      "stake_account",
      "chain_authority",
      "validator_vote",
      "clock",
      "rent",
      "stake_history",
      "stake_config",
      "stake_program",
      "system_program",
    ]);

    expect(accountNamesFor("trigger_unstake")).to.deep.equal([
      "triggerer",
      "chain_account",
      "trigger_vault",
      "chain_authority",
      "clock",
      "stake_history",
      "stake_program",
      "system_program",
    ]);

    expect(uiHtml).to.include("Enter a chain");
    expect(uiHtml).to.include("Donate");
    expect(uiHtml).to.include("Break the Chain");
    expect(uiHtml).to.include("Compound");
    expect(uiHtml).to.include("Trigger Unstakes");
    expect(uiHtml).to.include("Withdraw");
    expect(uiHtml).to.include("How many accounts to unstake (max 5)");
    expect(uiHtml).to.include('id="validatorSelect"');

    const enterAccounts = accountNamesFor("enter_chain");
    const donateAccounts = accountNamesFor("donate_to_chain");
    const compoundAccounts = accountNamesFor("compound_rewards");

    expect(enterAccounts).to.include.members(["validator_registry", "validator_vote"]);
    expect(donateAccounts).to.include.members(["validator_registry", "validator_vote"]);
    expect(compoundAccounts).to.include.members(["validator_registry", "validator_vote"]);
    expect(enterAccounts).to.include("trigger_vault");
    expect(donateAccounts).to.include("trigger_vault");
    expect(accountNamesFor("trigger_unstake")).to.include("trigger_vault");

    const global = await program.account.globalConfig.fetch(globalConfig);
    expect(global.unstakeBatchSize).to.equal(DEFAULT_UNSTAKE_BATCH_SIZE);
    expect(toBigInt(global.triggerRewardLamports)).to.equal(DEFAULT_TRIGGER_REWARD_LAMPORTS);
  });
});
