/**
 * frontend/app.js
 * ChainStaking, Anchor transaction layer for the browser.
 *
 * Prerequisites (loaded before this script):
 *   1. https://unpkg.com/@solana/web3.js@1.95.8/lib/index.iife.min.js  → window.solanaWeb3
 *   2. https://unpkg.com/@coral-xyz/anchor@0.31.1/dist/browser/index.js → window.anchor
 *   3. frontend/pda.js                                                  → window.ChainStakingPDA
 *
 * All handlers are exposed on window.CS so the existing UI inline handlers
 * can delegate to them. If the libraries are not loaded the UI falls back
 * to the existing mock behaviour silently.
 */
(async function (global) {
  "use strict";

  // ─── Debug flag ──────────────────────────────────────────────────────────
  // Set window.CS_DEBUG = true in DevTools to enable verbose logging.
  let DEBUG = !!global.CS_DEBUG;

  function log(...args) {
    if (DEBUG) console.log("[CS]", ...args);
  }

  // CS.log, public alias so callers outside the IIFE can emit debug output
  // Usage: CS.log("my message")  (only prints when CS.debug === true)
  function csLog(...args) { log(...args); }

  // ─── Dependency guards ───────────────────────────────────────────────────
  if (!global.solanaWeb3) {
    console.error("[CS] @solana/web3.js not loaded. Anchor integration disabled.");
    return;
  }
  if (!global.anchor) {
    console.error("[CS] @coral-xyz/anchor not loaded. Anchor integration disabled.");
    return;
  }
  if (!global.ChainStakingPDA) {
    console.error("[CS] frontend/pda.js not loaded. Anchor integration disabled.");
    return;
  }

  const web3 = global.solanaWeb3;
  const anchor = global.anchor;
  const PDA = global.ChainStakingPDA;

  const { PublicKey, Keypair, Transaction, SystemProgram, LAMPORTS_PER_SOL,
    SYSVAR_CLOCK_PUBKEY, SYSVAR_RENT_PUBKEY, SYSVAR_STAKE_HISTORY_PUBKEY,
    StakeProgram } = web3;

  // ─── Well-known Solana programme addresses ─────────────────────────────
  const STAKE_PROGRAM_ID = StakeProgram.programId;           // BPF Stake Program
  const STAKE_CONFIG_ID = new PublicKey("StakeConfig11111111111111111111111111111111");

  // ─── Config ──────────────────────────────────────────────────────────────
  // RPC URL resolution order:
  //   1. window.__CS_RPC  (set before page load or via inline <script>)
  //   2. localStorage("__CS_RPC")  (persists across reloads)
  //   3. Helius Devnet default (generous free-tier, no 429s)
  // Override at runtime:  localStorage.setItem("__CS_RPC","https://…"); location.reload();
  const RPC_URL =
    global.__CS_RPC ||
    (typeof localStorage !== "undefined" && localStorage.getItem("__CS_RPC")) ||
    "https://devnet.helius-rpc.com/?api-key=88f2e498-4493-4e33-a057-592cbed743e0";
  console.log("[CS] RPC endpoint:", RPC_URL);

  // IDL loaded from absolute server path (works when served via npx serve / nginx)
  const IDL_PATH = "./target/idl/chainstaking.json";

  // ─── Module-level state ──────────────────────────────────────────────────
  let connection = null;
  let provider = null;
  let program = null;
  let walletPubkey = null;
  let IDL = null;
  let PROGRAM_ID = null;

  // ─── Retry helper for RPC calls (avoids 429 throttling) ─────────────────
  async function retryRpc(fn, retries = 2, delayMs = 1200) {
    for (let attempt = 0; attempt <= retries; attempt++) {
      try {
        return await fn();
      } catch (err) {
        const is429 = err?.message?.includes("429") || err?.message?.includes("Too Many Requests");
        if (attempt < retries && is429) {
          log(`RPC 429, retry ${attempt + 1}/${retries} in ${delayMs}ms`);
          await new Promise(r => setTimeout(r, delayMs * (attempt + 1)));
        } else {
          throw err;
        }
      }
    }
  }

  // ─── Status UI ───────────────────────────────────────────────────────────
  function setStatus(msg, type = "info") {
    const el = document.getElementById("txStatus");
    if (!el) return;
    el.style.display = msg ? "flex" : "none";
    el.textContent = msg;
    el.className = `tx-status tx-status--${type}`;
    if (type === "ok" || type === "err") {
      setTimeout(() => { el.style.display = "none"; }, 6000);
    }
  }

  // ─── IDL loading ─────────────────────────────────────────────────────────
  async function loadIDL() {
    if (IDL) return IDL;
    log("Fetching IDL from", IDL_PATH);
    const res = await fetch(IDL_PATH);
    if (!res.ok) throw new Error(`Failed to fetch IDL: ${res.status} ${res.statusText}`);
    IDL = await res.json();

    // Extract program ID from IDL metadata, never hardcoded
    const rawAddress = IDL.metadata?.address ?? IDL.address;
    if (!rawAddress) {
      throw new Error(
        "Program ID missing from IDL metadata. " +
        "Ensure target/idl/chainstaking.json contains metadata.address."
      );
    }
    PROGRAM_ID = new PublicKey(rawAddress);
    log("Program ID loaded from IDL:", PROGRAM_ID.toBase58());
    return IDL;
  }

  // ─── Wallet connection ───────────────────────────────────────────────────
  async function connectWallet() {
    try {
      const solana = global.solana;
      if (!solana || !solana.isPhantom) {
        setStatus("Phantom wallet not found. Install Phantom extension.", "err");
        toast("Phantom wallet not found", "err");
        return;
      }
      setStatus("Connecting wallet…", "info");
      const resp = await solana.connect();
      walletPubkey = resp.publicKey;
      log("Wallet connected:", walletPubkey.toBase58());

      // Update existing walletConnected flag used by the rest of the UI
      global.walletConnected = true;

      // Update wallet button
      const btn = document.getElementById("walletBtn");
      if (btn) {
        btn.className = "wbtn on";
        const short = walletPubkey.toBase58().slice(0, 4) + "..." + walletPubkey.toBase58().slice(-4);
        btn.innerHTML = `<div class="wdot"></div>${short}`;
      }

      setStatus("", "info");
      toast("Wallet connected", "ok");

      // Initialise Anchor provider + program
      await initProgram();

      // Pull on-chain data, retry once on RPC throttle
      try {
        await Promise.all([fetchValidatorRegistry(), fetchAllChains()]);
      } catch (loadErr) {
        log("Initial data load failed, retrying in 1.5s…", loadErr.message);
        await new Promise(r => setTimeout(r, 1500));
        await Promise.all([fetchValidatorRegistry(), fetchAllChains()]);
      }
      log("Chain state loaded.");

      // Re-render UI
      if (typeof renderChainPage === "function") renderChainPage();
      if (typeof renderProfile === "function") renderProfile();

    } catch (err) {
      console.error("[CS] connectWallet error:", err);
      setStatus("Wallet connection failed: " + err.message, "err");
      toast("Wallet connection failed", "err");
    }
  }

  async function disconnectWallet() {
    try {
      if (global.solana && global.solana.disconnect) await global.solana.disconnect();
    } catch (_) { }
    walletPubkey = null;
    provider = null;
    program = null;
    global.walletConnected = false;
    const btn = document.getElementById("walletBtn");
    if (btn) { btn.className = "wbtn"; btn.innerHTML = "Connect Wallet"; }
    setStatus("", "info");
    toast("Wallet disconnected", "ok");
  }

  // ─── Provider + Program initialisation ──────────────────────────────────
  async function initProgram() {
    if (!walletPubkey) throw new Error("Wallet not connected");
    connection = new web3.Connection(RPC_URL, "confirmed");
    const idl = await loadIDL();

    // Build an AnchorProvider-compatible wallet wrapper around window.solana
    const walletAdapter = {
      publicKey: walletPubkey,
      signTransaction: async (tx) => global.solana.signTransaction(tx),
      signAllTransactions: async (txs) => global.solana.signAllTransactions(txs),
    };

    const opts = anchor.AnchorProvider.defaultOptions();
    provider = new anchor.AnchorProvider(connection, walletAdapter, opts);
    anchor.setProvider(provider);

    // Anchor 0.31.x: 2-arg constructor, reads programId from idl.address
    program = new anchor.Program(idl, provider);
    log("Anchor program initialised:", program.programId.toBase58());
  }

  // ─── Low-level tx helper ─────────────────────────────────────────────────
  async function sendTx(txPromise, extraSigners = []) {
    setStatus("Transaction pending…", "pending");
    try {
      // txPromise is the result of program.methods.xxx().transaction()
      const tx = await txPromise;
      const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash();
      tx.recentBlockhash = blockhash;
      tx.feePayer = walletPubkey;

      if (extraSigners.length > 0) {
        tx.partialSign(...extraSigners);
      }
      const signed = await provider.wallet.signTransaction(tx);
      const rawTx = signed.serialize();
      const sig = await connection.sendRawTransaction(rawTx, { skipPreflight: false });
      await connection.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, "confirmed");
      log("Transaction confirmed:", sig);
      setStatus("✓ Transaction confirmed: " + sig.slice(0, 12) + "…", "ok");
      return sig;
    } catch (err) {
      const msg = extractAnchorError(err);
      setStatus("✗ " + msg, "err");
      throw err;
    }
  }

  /** Try to extract a readable Anchor custom error message. */
  function extractAnchorError(err) {
    if (err && err.message) {
      // Anchor wraps error logs, look for 'Error Message: '
      const m = err.message.match(/Error Message: (.+)/);
      if (m) return m[1];
      return err.message.slice(0, 120);
    }
    return String(err).slice(0, 120);
  }

  // ─── State refresh ───────────────────────────────────────────────────────
  async function fetchChainState(chainId) {
    if (!program) return;
    try {
      const [chainPda] = PDA.deriveChainPda(PROGRAM_ID, chainId);
      const acct = await retryRpc(() => program.account.chainAccount.fetch(chainPda));
      log(`fetchChainState(${chainId}):`, acct);

      // Update the global CHAINS array used by the existing UI
      if (!global.CHAINS || !global.CHAINS[chainId]) return;
      const c = global.CHAINS[chainId];
      c.totalStaked = Number(acct.totalStakedLamports) / LAMPORTS_PER_SOL;
      c.yieldPool = Number(acct.frontendEstimatedYieldPoolLamports) / LAMPORTS_PER_SOL;
      c.donPot = Number(acct.donationPotLamports) / LAMPORTS_PER_SOL;
      c.isBroken = acct.isBroken;
      c.round = acct.currentRound;
      c.entryCount = acct.entryCount;
      c.donationCount = acct.donationCount;
      c.lastEntryAmount = Number(acct.lastEntryAmount);
      c.lastEntryAuthority = acct.lastEntryAuthority.toBase58();
      c.lastEntryTimestamp = Number(acct.lastEntryTimestamp);
      // Determine status string
      if (acct.isBroken) { c.status = "empty"; }
      else if (c.entryCount === 0) { c.status = "empty"; }
      else if (c.totalStaked > 50) { c.status = "whale"; }
      else if (c.entryCount >= 5) { c.status = "hot"; }
      else { c.status = "active"; }

      return acct;
    } catch (err) {
      // Account may not exist yet (not initialised)
      log(`fetchChainState(${chainId}) failed:`, err.message);
    }
  }

  async function fetchAllChains() {
    if (!program) return;
    // Stagger fetches in groups of 3 to avoid 429 bursts on public RPC
    for (let i = 0; i < 10; i += 3) {
      const batch = [];
      for (let j = i; j < Math.min(i + 3, 10); j++) batch.push(fetchChainState(j));
      await Promise.allSettled(batch);
    }
  }

  async function fetchValidatorRegistry() {
    if (!program) return;
    try {
      const [regPda] = PDA.deriveValidatorRegistryPda(PROGRAM_ID);
      const reg = await retryRpc(() => program.account.validatorRegistry.fetch(regPda));
      const list = reg.validatorList.map((pk) => pk.toBase58());
      log("Validator registry:", list);

      // Overwrite the global VALIDATORS array used by validatorSelectHTML()
      if (global.VALIDATORS) {
        global.VALIDATORS.length = 0;
        list.forEach((pk) => global.VALIDATORS.push(pk));
      }

      // Inject a persistent hidden dropdown so getSelectedValidatorPubkey()
      // works even when no modal is open (e.g. console calls).
      ensureValidatorDropdown(list);

      return list;
    } catch (err) {
      log("fetchValidatorRegistry failed:", err.message);
      // Keep existing mock VALIDATORS list
    }
  }

  /**
   * Ensure a #validatorSelect element exists in the DOM at all times.
   * If the modal creates its own, that one takes priority via getElementById.
   * This fallback is hidden and lives at the end of <body>.
   */
  function ensureValidatorDropdown(validators) {
    let sel = document.getElementById("validatorSelect");
    if (sel) {
      // Dropdown already exists (modal is open), refresh its options
      sel.innerHTML = validators.map((v, i) => {
        const d = v.length > 20 ? v.slice(0, 4) + '...' + v.slice(-4) : v;
        return `<option value="${v}"${i === 0 ? ' selected' : ''}>${d}</option>`;
      }).join('');
      return;
    }
    // Create a persistent hidden dropdown
    const wrap = document.createElement('div');
    wrap.id = 'csValidatorFallback';
    wrap.style.cssText = 'position:absolute;left:-9999px;';
    wrap.innerHTML = `<select id="validatorSelect">
      ${validators.map((v, i) => {
      const d = v.length > 20 ? v.slice(0, 4) + '...' + v.slice(-4) : v;
      return `<option value="${v}"${i === 0 ? ' selected' : ''}>${d}</option>`;
    }).join('')}
    </select>`;
    document.body.appendChild(wrap);
    log("Injected fallback validator dropdown, count:", validators.length);
  }

  // ─── Helper: get validator pubkey from modal select ──────────────────────
  function getSelectedValidatorPubkey() {
    const sel = document.getElementById("validatorSelect");
    if (sel && sel.value) {
      try {
        return new PublicKey(sel.value);
      } catch (_) {
        throw new Error("Invalid validator pubkey: " + sel.value);
      }
    }
    // Fallback: if only one validator in registry, use it directly
    if (global.VALIDATORS && global.VALIDATORS.length > 0) {
      log("No dropdown found, using first registry validator:", global.VALIDATORS[0]);
      return new PublicKey(global.VALIDATORS[0]);
    }
    throw new Error("Please select a validator, no dropdown and no registry loaded.");
  }

  // ─── Helper: get chain state from cache OR live fetch ──────────────────
  async function getChainState(chainId) {
    // Prefer cached data from global.CHAINS (populated by fetchAllChains)
    const cached = global.CHAINS && global.CHAINS[chainId];
    if (cached && cached.round !== undefined && cached.entryCount !== undefined) {
      return cached;
    }
    // Fallback: fetch directly from chain account PDA
    log(`Cache miss for chain ${chainId}, fetching live…`);
    const [chainPda] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const acct = await retryRpc(() => program.account.chainAccount.fetch(chainPda));
    return {
      round: acct.currentRound,
      entryCount: acct.entryCount,
      donationCount: acct.donationCount,
      compoundCount: acct.compoundCount ?? 0,
      spreadBps: acct.spreadBps,
      lastEntryAmount: Number(acct.lastEntryAmount),
      isBroken: acct.isBroken,
    };
  }

  /**
   * Compute the minimum entry lamports from on-chain state.
   * Formula: lastEntryAmount * (1 + spreadBps/10000) + 1% fee + 0.01 SOL flat fee
   * When chain is empty (entryCount=0), minimum is 0.01 SOL.
   */
  function computeMinEntryLamports(chain) {
    if (!chain || chain.entryCount === 0 || chain.isBroken) {
      // Base entry: 0.01 SOL + 1% fee + 0.01 flat
      const base = 0.01 * LAMPORTS_PER_SOL;
      return BigInt(Math.ceil(base * 1.01 + 0.01 * LAMPORTS_PER_SOL));
    }
    const last = chain.lastEntryAmount; // already in lamports
    const spread = chain.spreadBps || 500;
    const nextStake = Math.ceil(last * (1 + spread / 10000));
    const fee = Math.ceil(nextStake * 0.01);         // 1% protocol fee
    const flat = 0.01 * LAMPORTS_PER_SOL;            // 0.01 SOL flat fee
    return BigInt(nextStake + fee + flat);
  }

  // ─── Instruction: enter_chain ─────────────────────────────────────────────
  /**
   * Called by the existing doEnter(chainId) after amount validation.
   * Replaces the mock state update with a real Anchor call.
   */
  async function enterChain(chainId, amountOrLamports) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");

    const chain = await getChainState(chainId);
    if (chain.isBroken) throw new Error(`Chain ${chainId} is currently broken, no new entries allowed.`);

    // Auto-calculate entry amount when not provided
    let amountLamports;
    if (amountOrLamports == null || amountOrLamports === undefined) {
      amountLamports = computeMinEntryLamports(chain);
      log("enterChain: auto-calculated entry amount", amountLamports.toString(), "lamports");
    } else {
      const raw = Number(amountOrLamports);
      if (Number.isNaN(raw) || raw <= 0) {
        throw new Error("Invalid entry amount.");
      }
      // Smart detection: >= 1_000_000 treat as lamports, otherwise as SOL
      amountLamports = raw >= 1_000_000
        ? BigInt(Math.round(raw))
        : BigInt(Math.round(raw * LAMPORTS_PER_SOL));
    }
    log("enterChain", { chainId, amountLamports: amountLamports.toString() });
    const validatorVote = getSelectedValidatorPubkey();

    const [globalConfig] = PDA.deriveGlobalConfigPda(PROGRAM_ID);
    const [validatorReg] = PDA.deriveValidatorRegistryPda(PROGRAM_ID);
    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [triggerVault] = PDA.deriveTriggerVaultPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);

    // Derive entry PDA using current on-chain round + entry_count
    const round = chain.round ?? 0;
    const position = chain.entryCount ?? 0;
    const [entryAccount] = PDA.deriveEntryPda(PROGRAM_ID, chainId, round, position);

    // Stake tracker: sourceType=0 (entry), sourceIndex = position
    const [stakeTracker] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, 0, position);

    // Client-generated stake account keypair (funded via CPI create_account in the program)
    const stakeAccountKp = Keypair.generate();
    log("enter_chain stake keypair:", stakeAccountKp.publicKey.toBase58());

    // Fee receiver is GlobalConfig.authority, fetch it
    const gcAcct = await program.account.globalConfig.fetch(globalConfig);
    const feeReceiver = gcAcct.authority;

    const sig = await sendTx(
      program.methods
        .enterChain({ amount: new anchor.BN(amountLamports.toString()), chainId })
        .accounts({
          staker: walletPubkey,
          globalConfig,
          validatorRegistry: validatorReg,
          chainAccount,
          triggerVault,
          entryAccount,
          stakeTracker,
          stakeAccount: stakeAccountKp.publicKey,
          chainAuthority,
          validatorVote,
          feeReceiver,
          clock: SYSVAR_CLOCK_PUBKEY,
          rent: SYSVAR_RENT_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeConfig: STAKE_CONFIG_ID,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .transaction(),
      [stakeAccountKp],
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Instruction: donate_to_chain ─────────────────────────────────────────
  async function donateToChain(chainId, amountOrLamports) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");

    const chain = await getChainState(chainId);
    if (chain.isBroken) throw new Error(`Chain ${chainId} is currently broken, cannot donate.`);

    // Auto-calculate minimum donation when not provided
    let amountLamports;
    if (amountOrLamports == null || amountOrLamports === undefined) {
      // Default to 0.01 SOL donation + fees
      const base = 0.01 * LAMPORTS_PER_SOL;
      amountLamports = BigInt(Math.ceil(base * 1.01 + 0.01 * LAMPORTS_PER_SOL));
      log("donateToChain: auto-calculated donation amount", amountLamports.toString(), "lamports");
    } else {
      const raw = Number(amountOrLamports);
      if (Number.isNaN(raw) || raw <= 0) {
        throw new Error("Invalid donation amount.");
      }
      amountLamports = raw >= 1_000_000
        ? BigInt(Math.round(raw))
        : BigInt(Math.round(raw * LAMPORTS_PER_SOL));
    }
    log("donateToChain", { chainId, amountLamports: amountLamports.toString() });
    const validatorVote = getSelectedValidatorPubkey();

    const [globalConfig] = PDA.deriveGlobalConfigPda(PROGRAM_ID);
    const [validatorReg] = PDA.deriveValidatorRegistryPda(PROGRAM_ID);
    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [triggerVault] = PDA.deriveTriggerVaultPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);

    const round = chain.round ?? 0;
    const donationIndex = chain.donationCount ?? 0;
    const [donationRecord] = PDA.deriveDonationRecordPda(PROGRAM_ID, chainId, round, donationIndex);

    // Stake tracker: sourceType=1 (donation), sourceIndex = donationIndex
    const [stakeTracker] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, 1, donationIndex);

    const stakeAccountKp = Keypair.generate();
    log("donate_to_chain stake keypair:", stakeAccountKp.publicKey.toBase58());

    const gcAcct = await program.account.globalConfig.fetch(globalConfig);
    const feeReceiver = gcAcct.authority;

    const sig = await sendTx(
      program.methods
        .donateToChain({
          amount: new anchor.BN(amountLamports.toString()),
          chainId,
          donationIndex,
        })
        .accounts({
          donor: walletPubkey,
          globalConfig,
          validatorRegistry: validatorReg,
          chainAccount,
          triggerVault,
          donationRecord,
          stakeTracker,
          stakeAccount: stakeAccountKp.publicKey,
          chainAuthority,
          validatorVote,
          feeReceiver,
          clock: SYSVAR_CLOCK_PUBKEY,
          rent: SYSVAR_RENT_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeConfig: STAKE_CONFIG_ID,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .transaction(),
      [stakeAccountKp],
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Instruction: manual_break_chain ────────────────────────────────────
  async function manualBreakChain(chainId) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");

    const [globalConfig] = PDA.deriveGlobalConfigPda(PROGRAM_ID);
    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);

    const sig = await sendTx(
      program.methods
        .manualBreakChain(chainId)
        .accounts({
          breaker: walletPubkey,
          globalConfig,
          chainAccount,
          clock: SYSVAR_CLOCK_PUBKEY,
        })
        .transaction(),
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Instruction: compound_rewards ──────────────────────────────────────
  async function compoundRewards(chainId, amountLamports, compoundIndex, validatorVoteOverride) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");
    // ── NaN guard ──
    if (amountLamports == null || Number.isNaN(Number(amountLamports)) || Number(amountLamports) <= 0) {
      throw new Error("Compound amount unavailable, chain state may not be loaded yet.");
    }
    log("compoundRewards", { chainId, amountLamports, compoundIndex });
    const validatorVote = validatorVoteOverride || getSelectedValidatorPubkey();

    const [validatorReg] = PDA.deriveValidatorRegistryPda(PROGRAM_ID);
    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);

    const chain = await getChainState(chainId);
    const round = chain.round ?? 0;

    const [compoundRecord] = PDA.deriveCompoundRecordPda(PROGRAM_ID, chainId, round, compoundIndex);
    // Stake tracker: sourceType=2 (compound), sourceIndex = compoundIndex
    const [stakeTracker] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, 2, compoundIndex);

    const stakeAccountKp = Keypair.generate();
    log("compound_rewards stake keypair:", stakeAccountKp.publicKey.toBase58());

    const sig = await sendTx(
      program.methods
        .compoundRewards({
          amount: new anchor.BN(amountLamports.toString()),
          chainId,
          compoundIndex,
        })
        .accounts({
          caller: walletPubkey,
          validatorRegistry: validatorReg,
          chainAccount,
          compoundRecord,
          stakeTracker,
          stakeAccount: stakeAccountKp.publicKey,
          chainAuthority,
          validatorVote,
          clock: SYSVAR_CLOCK_PUBKEY,
          rent: SYSVAR_RENT_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeConfig: STAKE_CONFIG_ID,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .transaction(),
      [stakeAccountKp],
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Instruction: trigger_unstake ────────────────────────────────────────
  /**
   * Scans chain stake trackers for deactivated-but-not-withdrawn accounts
   * and passes up to `batchSize` (max 5) pairs as remaining accounts.
   */
  async function triggerUnstake(chainId, batchSize) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");
    batchSize = Math.min(Math.max(1, batchSize || 1), 5);

    const chain = global.CHAINS && global.CHAINS[chainId];
    const round = chain ? (chain.round ?? 0) : 0;

    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [triggerVault] = PDA.deriveTriggerVaultPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);

    // Best-effort scan: probe entry, donation, compound stake trackers
    const pairs = await scanDeactivatedTrackers(chainId, round, batchSize);
    if (pairs.length === 0) {
      toast("No deactivated stake accounts found to trigger", "err");
      setStatus("No deactivated accounts found", "err");
      return;
    }

    // remainingAccounts: [stakeTracker, stakeAccount] × N
    const remainingAccounts = pairs.flatMap(({ trackerPda, stakeAccountPubkey }) => [
      { pubkey: trackerPda, isSigner: false, isWritable: true },
      { pubkey: stakeAccountPubkey, isSigner: false, isWritable: true },
    ]);

    log("trigger_unstake pairs:", pairs.map(p => p.trackerPda.toBase58()));

    const sig = await sendTx(
      program.methods
        .triggerUnstake(chainId)
        .accounts({
          triggerer: walletPubkey,
          chainAccount,
          triggerVault,
          chainAuthority,
          clock: SYSVAR_CLOCK_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts(remainingAccounts)
        .transaction(),
    );

    await fetchChainState(chainId);
    return sig;
  }

  /**
   * Probe StakeTracker PDAs for active (not yet deactivated, not withdrawn)
   * accounts. trigger_unstake requires accounts that have NOT been deactivated.
   * Checks entry (type 0), donation (type 1), compound (type 2) trackers.
   *
   * FIX (M-05): Inverted the condition, trigger_unstake targets accounts
   * where is_deactivated=false (still active, need deactivation).
   */
  async function scanDeactivatedTrackers(chainId, round, limit) {
    const chain = global.CHAINS && global.CHAINS[chainId];
    const entryCount = chain ? (chain.entryCount ?? 0) : 0;
    const donationCount = chain ? (chain.donationCount ?? 0) : 0;

    const candidates = [];
    // Entry trackers
    for (let i = 0; i < entryCount; i++) {
      candidates.push({ type: 0, idx: i });
    }
    // Donation trackers
    for (let i = 0; i < donationCount; i++) {
      candidates.push({ type: 1, idx: i });
    }
    // Compound trackers, probe a reasonable range (no count stored on chain)
    for (let i = 0; i < 20; i++) {
      candidates.push({ type: 2, idx: i });
    }

    const results = [];
    for (const { type, idx } of candidates) {
      if (results.length >= limit) break;
      try {
        const [trackerPda] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, type, idx);
        const tracker = await program.account.stakeTracker.fetch(trackerPda);
        if (!tracker.isDeactivated && !tracker.isWithdrawn) {
          results.push({ trackerPda, stakeAccountPubkey: tracker.stakeAccount });
          log(`Found active tracker type=${type} idx=${idx}:`, trackerPda.toBase58());
        }
      } catch (_) {
        // Account not found, stop probing this type
        if (type !== 2 || idx > 2) break;
      }
    }
    return results;
  }

  // ─── Instruction: exit_first_entrant ──────────────────────────────────
  /**
   * FIX (M-04): Added frontend support for exit_first_entrant.
   * Allows the sole entrant (entry_count == 1) to deactivate/withdraw their
   * stake and exit the chain. This is a two-phase operation:
   *   - First call: deactivates the stake
   *   - Second call: withdraws after cooldown
   */
  async function exitFirstEntrant(chainId) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");

    const chain = global.CHAINS && global.CHAINS[chainId];
    const round = chain ? (chain.round ?? 0) : 0;

    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);
    const [entryAccount] = PDA.deriveEntryPda(PROGRAM_ID, chainId, round, 0);
    const [stakeTracker] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, 0, 0);

    // Fetch stake account from entry
    const entryData = await program.account.entryAccount.fetch(entryAccount);
    const stakeAccountPk = entryData.stakeAccount;

    const sig = await sendTx(
      program.methods
        .exitFirstEntrant(chainId)
        .accounts({
          staker: walletPubkey,
          chainAccount,
          entryAccount,
          stakeTracker,
          stakeAccount: stakeAccountPk,
          chainAuthority,
          clock: SYSVAR_CLOCK_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .transaction(),
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Instruction: withdraw_after_cooldown ───────────────────────────────
  /**
   * Scans the caller's EntryAccount PDAs to find a withdrawable position.
   * Parameters from the UI: chainId, sourceType, sourceIndex, entryPosition.
   */
  async function withdrawAfterCooldown(chainId, sourceType, sourceIndex, entryPosition) {
    if (!program) throw new Error("Program not initialised. Connect wallet first.");

    const [globalConfig] = PDA.deriveGlobalConfigPda(PROGRAM_ID);
    const [chainAccount] = PDA.deriveChainPda(PROGRAM_ID, chainId);
    const [chainAuthority] = PDA.deriveChainAuthorityPda(PROGRAM_ID, chainId);

    const chain = global.CHAINS && global.CHAINS[chainId];
    const round = chain ? (chain.round ?? 0) : 0;

    const [entryAccount] = PDA.deriveEntryPda(PROGRAM_ID, chainId, round, entryPosition);
    const [stakeTracker] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, sourceType, sourceIndex);

    // Fetch stakeAccount from the tracker
    const trackerData = await program.account.stakeTracker.fetch(stakeTracker);
    const stakeAccountPk = trackerData.stakeAccount;

    // Fetch staker from entry account
    const entryData = await program.account.entryAccount.fetch(entryAccount);
    const stakerPk = entryData.staker;

    const gcAcct = await program.account.globalConfig.fetch(globalConfig);
    const feeReceiver = gcAcct.authority;

    const sig = await sendTx(
      program.methods
        .withdrawAfterCooldown({ chainId, sourceType, sourceIndex, entryPosition })
        .accounts({
          recipient: walletPubkey,
          globalConfig,
          chainAccount,
          entryAccount,
          stakeTracker,
          stakeAccount: stakeAccountPk,
          chainAuthority,
          staker: stakerPk,
          feeReceiver,
          clock: SYSVAR_CLOCK_PUBKEY,
          stakeHistory: SYSVAR_STAKE_HISTORY_PUBKEY,
          stakeProgram: STAKE_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
        })
        .transaction(),
    );

    await fetchChainState(chainId);
    return sig;
  }

  // ─── Scan my withdrawable positions ────────────────────────────────────
  /**
   * Returns a list of withdrawable positions for the connected wallet.
   * Each element: { chainId, round, sourceType, sourceIndex, entryPosition, payoutLamports }
   */
  async function findMyWithdrawablePositions() {
    if (!program || !walletPubkey) return [];
    const results = [];

    for (let chainId = 0; chainId < 10; chainId++) {
      const chain = global.CHAINS && global.CHAINS[chainId];
      if (!chain) continue;
      const round = chain.round ?? 0;
      const entryCount = chain.entryCount ?? 0;

      for (let pos = 0; pos < entryCount; pos++) {
        try {
          const [entryPda] = PDA.deriveEntryPda(PROGRAM_ID, chainId, round, pos);
          const entry = await program.account.entryAccount.fetch(entryPda);
          if (entry.staker.toBase58() !== walletPubkey.toBase58()) continue;
          if (entry.isWithdrawn) continue;
          if (entry.payoutLamports.toString() === "0") continue;

          // sourceType=0 (entry), sourceIndex=position for entry stakes
          const [trackerPda] = PDA.deriveStakeTrackerPda(PROGRAM_ID, chainId, round, 0, pos);
          try {
            const tracker = await program.account.stakeTracker.fetch(trackerPda);
            if (!tracker.isWithdrawn) {
              results.push({
                chainId,
                round,
                sourceType: 0,
                sourceIndex: pos,
                entryPosition: pos,
                payoutLamports: Number(entry.payoutLamports),
              });
            }
          } catch (_) { }
        } catch (_) { }
      }
    }
    log("findMyWithdrawablePositions:", results);
    return results;
  }

  // ─── Public API ──────────────────────────────────────────────────────────
  global.CS = {
    // State
    get walletPubkey() { return walletPubkey; },
    get program() { return program; },
    get connection() { return connection; },
    get PROGRAM_ID() { return PROGRAM_ID; },

    // Debug toggle
    get debug() { return DEBUG; },
    set debug(val) { DEBUG = !!val; },

    // Wallet
    connectWallet,
    disconnectWallet,

    // Program
    initProgram,

    // State
    fetchChainState,
    fetchAllChains,
    fetchValidatorRegistry,
    findMyWithdrawablePositions,

    // Instructions
    enterChain,
    exitFirstEntrant,
    donateToChain,
    manualBreakChain,
    compoundRewards,
    triggerUnstake,
    withdrawAfterCooldown,

    // Misc
    setStatus,
    log: csLog,
    loadIDL,
    scanDeactivatedTrackers,
  };

  log("ChainStaking frontend client initialised. RPC:", RPC_URL);
  log("Set CS.debug = true for verbose logging. Set __CS_RPC to override RPC URL.");
})(window);
