#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const { resolveBinaryPath, unsupportedPlatformMessage } = require("../index.js");

const binary = resolveBinaryPath();

if (binary === null) {
  console.error(unsupportedPlatformMessage());
  process.exit(2);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`skif: failed to run ${binary}: ${result.error.message}`);
  process.exit(2);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(result.status ?? 2);
