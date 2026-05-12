/**
 * frontend/pda.js
 * Browser-safe PDA derivation helpers for the ChainStaking protocol.
 *
 * Mirrors tests/helpers/chainstaking.js without Node.js Buffer API.
 * Requires @solana/web3.js to be loaded globally (window.solanaWeb3).
 *
 * Usage:
 *   const { deriveChainPda, computeRequiredEntryLamports } = window.ChainStakingPDA;
 */
(function (global) {
  "use strict";

  // ─── Seed constants (UTF-8 encoded) ───────────────────────────────────────
  const enc = new TextEncoder();
  const GLOBAL_CONFIG_SEED    = enc.encode("global_config");
  const VALIDATOR_REGISTRY_SEED = enc.encode("validator_registry");
  const CHAIN_SEED            = enc.encode("chain");
  const ENTRY_SEED            = enc.encode("entry");
  const STAKE_TRACKER_SEED    = enc.encode("stake_tracker");
  const DONATION_SEED         = enc.encode("donation");
  const COMPOUND_SEED         = enc.encode("compound");
  const TRIGGER_VAULT_SEED    = enc.encode("trigger_vault");
  const CHAIN_AUTH_SEED       = enc.encode("chain_auth");

  // ─── Protocol constants ───────────────────────────────────────────────────
  const CHAIN_SPREADS_BPS = [
    500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 500000, 1000000,
  ];

  const DEFAULT_FLAT_FEE_LAMPORTS     = BigInt(10_000_000);
  const DEFAULT_TRIGGER_REWARD_LAMPORTS = BigInt(10_000_000);
  const DEFAULT_UNSTAKE_BATCH_SIZE    = 5;
  const MIN_ENTRY_LAMPORTS            = BigInt(10_000_000);

  // ─── Little-endian integer helpers ────────────────────────────────────────
  /** 4‑byte little-endian Uint8Array for a u32 value. */
  function u32LE(value) {
    const buf = new Uint8Array(4);
    const v = value >>> 0; // coerce to u32
    buf[0] = (v)       & 0xff;
    buf[1] = (v >>  8) & 0xff;
    buf[2] = (v >> 16) & 0xff;
    buf[3] = (v >> 24) & 0xff;
    return buf;
  }

  /** 2‑byte little-endian Uint8Array for a u16 value. */
  function u16LE(value) {
    const v = value & 0xffff;
    return new Uint8Array([v & 0xff, (v >> 8) & 0xff]);
  }

  /** 1‑byte Uint8Array for a u8 value. */
  function u8(value) {
    return new Uint8Array([value & 0xff]);
  }

  // ─── PDA derivation ───────────────────────────────────────────────────────
  function web3() {
    if (!global.solanaWeb3) throw new Error("[ChainStakingPDA] window.solanaWeb3 not found. Load @solana/web3.js first.");
    return global.solanaWeb3;
  }

  function deriveGlobalConfigPda(programId) {
    return web3().PublicKey.findProgramAddressSync(
      [GLOBAL_CONFIG_SEED],
      programId,
    );
  }

  function deriveValidatorRegistryPda(programId) {
    return web3().PublicKey.findProgramAddressSync(
      [VALIDATOR_REGISTRY_SEED],
      programId,
    );
  }

  function deriveChainPda(programId, chainId) {
    return web3().PublicKey.findProgramAddressSync(
      [CHAIN_SEED, u8(chainId)],
      programId,
    );
  }

  function deriveTriggerVaultPda(programId, chainId) {
    return web3().PublicKey.findProgramAddressSync(
      [TRIGGER_VAULT_SEED, u8(chainId)],
      programId,
    );
  }

  function deriveChainAuthorityPda(programId, chainId) {
    return web3().PublicKey.findProgramAddressSync(
      [CHAIN_AUTH_SEED, u8(chainId)],
      programId,
    );
  }

  function deriveEntryPda(programId, chainId, round, position) {
    return web3().PublicKey.findProgramAddressSync(
      [ENTRY_SEED, u8(chainId), u32LE(round), u32LE(position)],
      programId,
    );
  }

  /**
   * sourceType: 0 = entry, 1 = donation, 2 = compound
   * sourceIndex: sequential index of that type within the round
   */
  function deriveStakeTrackerPda(programId, chainId, round, sourceType, sourceIndex) {
    return web3().PublicKey.findProgramAddressSync(
      [
        STAKE_TRACKER_SEED,
        u8(chainId),
        u32LE(round),
        u8(sourceType),
        u32LE(sourceIndex),
      ],
      programId,
    );
  }

  function deriveDonationRecordPda(programId, chainId, round, donationIndex) {
    return web3().PublicKey.findProgramAddressSync(
      [DONATION_SEED, u8(chainId), u32LE(round), u16LE(donationIndex)],
      programId,
    );
  }

  function deriveCompoundRecordPda(programId, chainId, round, compoundIndex) {
    return web3().PublicKey.findProgramAddressSync(
      [COMPOUND_SEED, u8(chainId), u32LE(round), u16LE(compoundIndex)],
      programId,
    );
  }

  // ─── Entry amount computation ─────────────────────────────────────────────
  /**
   * Compute the required entry amount in lamports for a new entrant.
   * @param {BigInt|number|string} lastEntryLamports  Previous entry principal (0 if first entrant).
   * @param {number}               spreadBps          Chain spread in basis points.
   * @returns {BigInt}
   */
  function computeRequiredEntryLamports(lastEntryLamports, spreadBps) {
    const last = BigInt(lastEntryLamports.toString());
    if (last === 0n) return MIN_ENTRY_LAMPORTS;
    return (last * (10_000n + BigInt(spreadBps))) / 10_000n;
  }

  // ─── Export ───────────────────────────────────────────────────────────────
  global.ChainStakingPDA = {
    // Seeds (for debugging / advanced use)
    GLOBAL_CONFIG_SEED,
    VALIDATOR_REGISTRY_SEED,
    CHAIN_SEED,
    ENTRY_SEED,
    STAKE_TRACKER_SEED,
    DONATION_SEED,
    COMPOUND_SEED,
    TRIGGER_VAULT_SEED,
    CHAIN_AUTH_SEED,

    // Constants
    CHAIN_SPREADS_BPS,
    DEFAULT_FLAT_FEE_LAMPORTS,
    DEFAULT_TRIGGER_REWARD_LAMPORTS,
    DEFAULT_UNSTAKE_BATCH_SIZE,
    MIN_ENTRY_LAMPORTS,

    // LE helpers
    u8,
    u16LE,
    u32LE,

    // PDA derivers
    deriveGlobalConfigPda,
    deriveValidatorRegistryPda,
    deriveChainPda,
    deriveEntryPda,
    deriveStakeTrackerPda,
    deriveDonationRecordPda,
    deriveCompoundRecordPda,
    deriveTriggerVaultPda,
    deriveChainAuthorityPda,

    // Amount helpers
    computeRequiredEntryLamports,
  };
})(window);
