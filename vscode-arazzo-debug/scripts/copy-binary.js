#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const ext = os.platform() === "win32" ? ".exe" : "";
const binaryName = `arazzo-debug-adapter${ext}`;
const src = path.join(__dirname, "..", "..", "target", "release", binaryName);
const destDir = path.join(__dirname, "..", "bin");
const dest = path.join(destDir, binaryName);

if (!fs.existsSync(src)) {
  console.error(
    `ERROR: ${src} not found.\n` +
      `Run \`cargo build --release -p arazzo-debug-adapter\` first.`
  );
  process.exit(1);
}

fs.mkdirSync(destDir, { recursive: true });
fs.copyFileSync(src, dest);

if (os.platform() !== "win32") {
  fs.chmodSync(dest, 0o755);
}

console.log(`Copied ${binaryName} -> ${path.relative(process.cwd(), dest)}`);
