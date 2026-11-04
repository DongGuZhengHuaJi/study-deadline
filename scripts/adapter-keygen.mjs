#!/usr/bin/env node
/**
 * Generate an Ed25519 key pair for signing local adapter packages.
 *
 * Usage:
 *   node scripts/adapter-keygen.mjs [keyId] [outputPath]
 *
 * The private key file is written under signing/ by default and must never be
 * committed. Put the printed public key into TRUSTED_ADAPTER_KEYS in
 * apps/desktop/src-tauri/src/adapters/install.rs and rebuild the app.
 */
import { generateKeyPairSync } from "node:crypto";
import { mkdirSync, existsSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const keyId = process.argv[2] || "dev-2026";
const outputPath = resolve(process.argv[3] || "signing/adapter-dev-key.json");

if (!/^[a-z0-9-]{2,64}$/.test(keyId)) {
  console.error("keyId must be 2-64 lowercase letters, numbers, or hyphens");
  process.exit(2);
}
if (existsSync(outputPath)) {
  console.error(`Refusing to overwrite existing key: ${outputPath}`);
  process.exit(2);
}

const { publicKey, privateKey } = generateKeyPairSync("ed25519");
const publicJwk = publicKey.export({ format: "jwk" });
const privateJwk = privateKey.export({ format: "jwk" });
const publicKeyBase64 = Buffer.from(publicJwk.x, "base64url").toString("base64");

const payload = {
  keyId,
  algorithm: "ed25519",
  publicKeyBase64,
  privateJwk,
  createdAt: new Date().toISOString(),
};

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(payload, null, 2)}\n`, {
  mode: 0o600,
});

console.log(`keyId: ${keyId}`);
console.log(`publicKeyBase64: ${publicKeyBase64}`);
console.log(`private key: ${outputPath}`);
console.log("Keep the private key private. Add the public key to TRUSTED_ADAPTER_KEYS.");
