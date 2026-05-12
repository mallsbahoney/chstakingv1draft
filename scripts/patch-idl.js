#!/usr/bin/env node
// Patches the generated IDL to use the correct deployed program ID.
// anchor build does not always update the 'address' field from declare_id!,
// so we enforce it here before running tests.

const fs = require("fs");
const path = require("path");

const IDL_PATH = path.join(__dirname, "../target/idl/chainstaking.json");
const PROGRAM_ID = "A4XAJ3tE5HYmCSQkfbgzGSdTjrsudMEy6EUqoYLjXRCY";

const idl = JSON.parse(fs.readFileSync(IDL_PATH, "utf8"));

if (idl.address !== PROGRAM_ID) {
  console.log(`[patch-idl] Updating IDL address: ${idl.address} → ${PROGRAM_ID}`);
  idl.address = PROGRAM_ID;
  fs.writeFileSync(IDL_PATH, JSON.stringify(idl, null, 2));
} else {
  console.log(`[patch-idl] IDL address already correct: ${PROGRAM_ID}`);
}
