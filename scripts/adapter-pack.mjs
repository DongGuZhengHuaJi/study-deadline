#!/usr/bin/env node
/**
 * Build a signed .sdapkg package from an adapter directory.
 *
 * Usage:
 *   node scripts/adapter-pack.mjs <adapter-dir> <private-key.json> [output.sdapkg]
 *
 * The package is a JSON envelope. The signature covers the exact manifest.json
 * bytes and index.js bytes; it does not rely on ZIP canonicalization.
 */
import { createHash, createPrivateKey, sign } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [adapterDirArg, keyFileArg, outputArg] = process.argv.slice(2);
if (!adapterDirArg || !keyFileArg) {
  console.error(
    "Usage: node scripts/adapter-pack.mjs <adapter-dir> <private-key.json> [output.sdapkg]",
  );
  process.exit(2);
}

const adapterDir = resolve(adapterDirArg);
const keyFile = resolve(keyFileArg);
const manifestRaw = readFileSync(join(adapterDir, "manifest.json"), "utf8");
let manifest;
try {
  manifest = JSON.parse(manifestRaw);
} catch (error) {
  console.error(`Invalid manifest.json: ${error.message}`);
  process.exit(1);
}
if (!manifest.id || !manifest.entry) {
  console.error("manifest must contain id and entry");
  process.exit(1);
}
if (manifest.entry.includes("/") || manifest.entry.includes("\\") || manifest.entry.includes("..")) {
  console.error("manifest.entry must be a single file inside the adapter directory");
  process.exit(1);
}

const source = readFileSync(join(adapterDir, manifest.entry), "utf8");
const key = JSON.parse(readFileSync(keyFile, "utf8"));
if (!key.keyId || !key.privateJwk) {
  console.error("private key file must contain keyId and privateJwk");
  process.exit(1);
}
if (manifest.keyId && manifest.keyId !== key.keyId) {
  console.error(`manifest.keyId ${manifest.keyId} does not match key ${key.keyId}`);
  process.exit(1);
}
if (manifest.sequence === undefined) {
  console.error("manifest.sequence is required for signed packages");
  process.exit(1);
}

const sha256Base64 = (value) => createHash("sha256").update(value).digest("base64");
const payload = [
  "study-deadline-adapter/1",
  key.keyId,
  sha256Base64(Buffer.from(manifestRaw, "utf8")),
  sha256Base64(Buffer.from(source, "utf8")),
].join("\n");
const privateKey = createPrivateKey({ key: key.privateJwk, format: "jwk" });
const signature = sign(null, Buffer.from(payload, "utf8"), privateKey).toString("base64");

const envelope = {
  format: "study-deadline-adapter/1",
  keyId: key.keyId,
  manifestJson: manifestRaw,
  source,
  signature,
};
const output = resolve(
  outputArg || join(adapterDir, `${manifest.id}-${manifest.version}.sdapkg`),
);
writeFileSync(output, `${JSON.stringify(envelope)}\n`);
console.log(`Wrote ${output}`);
console.log(`adapter=${manifest.id} version=${manifest.version} key=${key.keyId}`);
