// Bundle entry point — re-exports Anchor as window.anchor
// @solana/web3.js is loaded separately via IIFE CDN as window.solanaWeb3
import * as anchor from "@coral-xyz/anchor";
window.anchor = anchor;
