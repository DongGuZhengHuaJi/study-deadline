#!/usr/bin/env node
/**
 * Build a signed adapter catalog.
 *
 * Usage:
 *   node scripts/adapter-catalog.mjs <entries.json> <private-key.json> <catalog.json>
 *
 * entries.json is a JSON array of:
 * { id, name, version, apiVersion, sequence, url, sha256 }
 *
 * sha256 is the base64 SHA-256 of the .sdapkg file. The exact entries JSON
 * bytes are covered by the signature, so do not reformat the file after
 * signing.
 */
import { createHash, createPrivateKey, sign } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const [entriesArg, keyFileArg, outputArg] = process.argv.slice(2);
if (!entriesArg || !keyFileArg || !outputArg) {
  console.error(
    "Usage: node scripts/adapter-catalog.mjs <entries.json> <private-key.json> <catalog.json>",
  );
  process.exit(2);
}
const entriesRaw = readFileSync(resolve(entriesArg), "utf8").trim();
const key = JSON.parse(readFileSync(resolve(keyFileArg), "utf8"));
if (!key.keyId || !key.privateJwk) {
  console.error("private key file must contain keyId and privateJwk");
  process.exit(1);
}
let entries;
try {
  entries = JSON.parse(entriesRaw);
} catch (error) {
  console.error(`Invalid entries JSON: ${error.message}`);
  process.exit(1);
}
if (!Array.isArray(entries) || entries.length === 0) {
  console.error("entries must be a non-empty array");
  process.exit(1);
}
for (const entry of entries) {
  if (!entry.id || !entry.version || !entry.sequence || !entry.url || !entry.sha256) {
    console.error("each entry needs id, version, sequence, url, sha256");
    process.exit(1);
  }
}
const generatedAt = new Date().toISOString();
const sha256Base64 = (value) => createHash("sha256").update(value).digest("base64");
const payload = [
  "study-deadline-adapter-catalog/1",
  key.keyId,
  sha256Base64(Buffer.from(generatedAt, "utf8")),
  sha256Base64(Buffer.from(entriesRaw, "utf8")),
].join("\n");
const privateKey = createPrivateKey({ key: key.privateJwk, format: "jwk" });
const signature = sign(null, Buffer.from(payload, "utf8"), privateKey).toString("base64");
const catalog = {
  format: "study-deadline-adapter-catalog/1",
  keyId: key.keyId,
  generatedAt,
  adaptersJson: entriesRaw,
  signature,
};
writeFileSync(resolve(outputArg), `${JSON.stringify(catalog, null, 2)}\n`);
console.log(`Wrote ${resolve(outputArg)} with ${entries.length} adapter entries`);
