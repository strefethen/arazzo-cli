#!/usr/bin/env node
"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

// Selects the arazzo-debug-adapter binary for the requested Rust target
// triple (--target <triple> or ARAZZO_DAP_TARGET), copies it into bin/, and
// records + verifies its SHA-256 in bin/SHA256SUMS.txt. With no target it
// uses the host build in target/release/.
function parseTarget() {
  const argv = process.argv.slice(2);
  const flagIndex = argv.indexOf("--target");
  if (flagIndex !== -1) {
    const value = argv[flagIndex + 1];
    if (!value) {
      console.error("ERROR: --target requires a Rust target triple.");
      process.exit(1);
    }
    return value;
  }
  return process.env.ARAZZO_DAP_TARGET || undefined;
}

const target = parseTarget();
// The .exe suffix follows the *target* platform, not the host.
const targetIsWindows = target
  ? target.includes("windows")
  : os.platform() === "win32";
const binaryName = `arazzo-debug-adapter${targetIsWindows ? ".exe" : ""}`;

const repoRoot = path.join(__dirname, "..", "..");
const releaseDir = target
  ? path.join(repoRoot, "target", target, "release")
  : path.join(repoRoot, "target", "release");
const src = path.join(releaseDir, binaryName);
const destDir = path.join(__dirname, "..", "bin");
const dest = path.join(destDir, binaryName);
const sumsFile = path.join(destDir, "SHA256SUMS.txt");

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function recordedHash() {
  if (!fs.existsSync(sumsFile)) {
    return undefined;
  }
  for (const line of fs.readFileSync(sumsFile, "utf8").split("\n")) {
    const match = line.match(/^([0-9a-f]{64})[ *]+(.+)$/);
    if (match && match[2] === binaryName) {
      return match[1];
    }
  }
  return undefined;
}

if (!fs.existsSync(src)) {
  // No build output for this target. A previously staged bin/ binary is only
  // acceptable if it verifies against the recorded checksum.
  if (fs.existsSync(dest)) {
    const recorded = recordedHash();
    if (!recorded) {
      console.error(
        `ERROR: ${dest} is staged but ${path.basename(sumsFile)} has no entry for ${binaryName}.\n` +
          `Rebuild with \`cargo build --release -p arazzo-debug-adapter${target ? ` --target ${target}` : ""}\` and rerun.`
      );
      process.exit(1);
    }
    const actual = sha256(dest);
    if (actual !== recorded) {
      console.error(
        `ERROR: SHA-256 mismatch for staged ${binaryName}.\n` +
          `  recorded: ${recorded}\n` +
          `  actual:   ${actual}\n` +
          `Rebuild with \`cargo build --release -p arazzo-debug-adapter${target ? ` --target ${target}` : ""}\` and rerun.`
      );
      process.exit(1);
    }
    console.log(
      `Verified pre-staged ${path.relative(process.cwd(), dest)} (sha256 ${actual})`
    );
    process.exit(0);
  }
  console.error(
    `ERROR: ${src} not found.\n` +
      `Run \`cargo build --release -p arazzo-debug-adapter${target ? ` --target ${target}` : ""}\` first.`
  );
  process.exit(1);
}

const srcHash = sha256(src);
fs.mkdirSync(destDir, { recursive: true });
fs.copyFileSync(src, dest);
if (!targetIsWindows) {
  fs.chmodSync(dest, 0o755);
}

const destHash = sha256(dest);
if (destHash !== srcHash) {
  console.error(
    `ERROR: SHA-256 mismatch after copy.\n` +
      `  source: ${srcHash}\n` +
      `  copied: ${destHash}`
  );
  process.exit(1);
}
fs.writeFileSync(sumsFile, `${srcHash}  ${binaryName}\n`);

console.log(
  `Copied ${binaryName}${target ? ` (${target})` : ""} -> ` +
    `${path.relative(process.cwd(), dest)} (sha256 ${srcHash})`
);
