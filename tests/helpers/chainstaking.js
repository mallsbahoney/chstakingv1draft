const anchor = require("@coral-xyz/anchor");

const { PublicKey, SystemProgram, LAMPORTS_PER_SOL, Transaction, sendAndConfirmTransaction } = anchor.web3;

const GLOBAL_CONFIG_SEED = Buffer.from("global_config");
const VALIDATOR_REGISTRY_SEED = Buffer.from("validator_registry");
const CHAIN_SEED = Buffer.from("chain");
const ENTRY_SEED = Buffer.from("entry");
const STAKE_TRACKER_SEED = Buffer.from("stake_tracker");
const DONATION_SEED = Buffer.from("donation");
const COMPOUND_SEED = Buffer.from("compound");
const TRIGGER_VAULT_SEED = Buffer.from("trigger_vault");
const CHAIN_AUTH_SEED = Buffer.from("chain_auth");

const CHAIN_SPREADS_BPS = [
  500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 500000, 1000000,
];

const DEFAULT_FLAT_FEE_LAMPORTS = 10_000_000n;
const DEFAULT_TRIGGER_REWARD_LAMPORTS = 10_000_000n;
const DEFAULT_UNSTAKE_BATCH_SIZE = 5;
const MIN_ENTRY_LAMPORTS = 10_000_000n;
const STAKE_CONFIG_PUBKEY = new PublicKey(
  "StakeConfig11111111111111111111111111111111",
);

function toBn(value) {
  return new anchor.BN(value.toString());
}

function toBigInt(value) {
  return BigInt(value.toString());
}

function u32Buffer(value) {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32LE(value, 0);
  return buffer;
}

function u16Buffer(value) {
  const buffer = Buffer.alloc(2);
  buffer.writeUInt16LE(value, 0);
  return buffer;
}

function deriveGlobalConfigPda(programId) {
  return PublicKey.findProgramAddressSync([GLOBAL_CONFIG_SEED], programId);
}

function deriveValidatorRegistryPda(programId) {
  return PublicKey.findProgramAddressSync([VALIDATOR_REGISTRY_SEED], programId);
}

function deriveChainPda(programId, chainId) {
  return PublicKey.findProgramAddressSync([CHAIN_SEED, Buffer.from([chainId])], programId);
}

function deriveTriggerVaultPda(programId, chainId) {
  return PublicKey.findProgramAddressSync(
    [TRIGGER_VAULT_SEED, Buffer.from([chainId])],
    programId,
  );
}

function deriveChainAuthorityPda(programId, chainId) {
  return PublicKey.findProgramAddressSync(
    [CHAIN_AUTH_SEED, Buffer.from([chainId])],
    programId,
  );
}

function deriveEntryPda(programId, chainId, round, position) {
  return PublicKey.findProgramAddressSync(
    [ENTRY_SEED, Buffer.from([chainId]), u32Buffer(round), u32Buffer(position)],
    programId,
  );
}

function deriveStakeTrackerPda(programId, chainId, round, sourceType, sourceIndex) {
  return PublicKey.findProgramAddressSync(
    [
      STAKE_TRACKER_SEED,
      Buffer.from([chainId]),
      u32Buffer(round),
      Buffer.from([sourceType]),
      u32Buffer(sourceIndex),
    ],
    programId,
  );
}

function deriveDonationRecordPda(programId, chainId, round, donationIndex) {
  return PublicKey.findProgramAddressSync(
    [DONATION_SEED, Buffer.from([chainId]), u32Buffer(round), u16Buffer(donationIndex)],
    programId,
  );
}

function deriveCompoundRecordPda(programId, chainId, round, compoundIndex) {
  return PublicKey.findProgramAddressSync(
    [COMPOUND_SEED, Buffer.from([chainId]), u32Buffer(round), u16Buffer(compoundIndex)],
    programId,
  );
}

function buildInitializeRemainingAccounts(programId) {
  const accounts = [];

  for (let chainId = 0; chainId < CHAIN_SPREADS_BPS.length; chainId += 1) {
    const [triggerVault] = deriveTriggerVaultPda(programId, chainId);
    accounts.push({ pubkey: triggerVault, isWritable: true, isSigner: false });
  }

  for (let chainId = 0; chainId < CHAIN_SPREADS_BPS.length; chainId += 1) {
    const [chainAccount] = deriveChainPda(programId, chainId);
    accounts.push({ pubkey: chainAccount, isWritable: true, isSigner: false });
  }

  return accounts;
}

function computeRequiredEntryLamports(lastEntryLamports, spreadBps) {
  const lastEntry =
    typeof lastEntryLamports === "bigint" ? lastEntryLamports : BigInt(lastEntryLamports);

  if (lastEntry === 0n) {
    return MIN_ENTRY_LAMPORTS;
  }

  return (lastEntry * (10_000n + BigInt(spreadBps))) / 10_000n;
}

async function airdrop(connection, pubkey, sol = 5) {
  // Use wallet transfer instead of faucet to avoid devnet rate limits
  const provider = anchor.getProvider();
  const lamports = Math.trunc(sol * LAMPORTS_PER_SOL);

  const tx = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: provider.wallet.publicKey,
      toPubkey: pubkey,
      lamports,
    }),
  );

  const latestBlockhash = await connection.getLatestBlockhash();
  tx.recentBlockhash = latestBlockhash.blockhash;
  tx.feePayer = provider.wallet.publicKey;

  const signed = await provider.wallet.signTransaction(tx);
  const signature = await connection.sendRawTransaction(signed.serialize());

  await connection.confirmTransaction(
    { signature, ...latestBlockhash },
    "confirmed",
  );
}

async function getLocalValidatorVotePubkey(connection) {
  const voteAccounts = await connection.getVoteAccounts();
  const votePubkey =
    voteAccounts.current[0]?.votePubkey ?? voteAccounts.delinquent[0]?.votePubkey;

  if (!votePubkey) {
    throw new Error("Local validator did not expose any vote accounts.");
  }

  return new PublicKey(votePubkey);
}

async function fundProgramAccount(provider, destination, lamports) {
  const amount = typeof lamports === "bigint" ? Number(lamports) : lamports;
  const transaction = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: provider.wallet.publicKey,
      toPubkey: destination,
      lamports: amount,
    }),
  );

  await provider.sendAndConfirm(transaction, []);
}

async function getLamports(connection, pubkey) {
  const accountInfo = await connection.getAccountInfo(pubkey, "confirmed");

  if (!accountInfo) {
    throw new Error(`Missing account ${pubkey.toBase58()}`);
  }

  return BigInt(accountInfo.lamports);
}

async function warpForwardSlots(connection, slots) {
  const currentSlot = await connection.getSlot("processed");
  const targetSlot = currentSlot + slots;
  const response = await connection._rpcRequest("warpSlot", [targetSlot]);

  if (response?.error) {
    throw new Error(`warpSlot failed: ${JSON.stringify(response.error)}`);
  }

  for (let attempt = 0; attempt < 20; attempt += 1) {
    const observedSlot = await connection.getSlot("processed");

    if (observedSlot >= targetSlot) {
      return observedSlot;
    }

    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`warpSlot did not advance to ${targetSlot}`);
}

async function expectAnchorError(callback, expectedMessage) {
  try {
    await callback();
  } catch (error) {
    const renderedError = `${error}`;

    if (!renderedError.includes(expectedMessage)) {
      throw new Error(
        `Expected error containing "${expectedMessage}", got "${renderedError}"`,
      );
    }

    return;
  }

  throw new Error(`Expected failure containing "${expectedMessage}"`);
}

module.exports = {
  CHAIN_SPREADS_BPS,
  DEFAULT_FLAT_FEE_LAMPORTS,
  DEFAULT_TRIGGER_REWARD_LAMPORTS,
  DEFAULT_UNSTAKE_BATCH_SIZE,
  GLOBAL_CONFIG_SEED,
  MIN_ENTRY_LAMPORTS,
  STAKE_CONFIG_PUBKEY,
  VALIDATOR_REGISTRY_SEED,
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
};
