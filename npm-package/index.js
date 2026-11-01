"use strict";

const fs = require("node:fs");
const path = require("node:path");

const PLATFORM_PACKAGES = [
  { pkg: "skif-darwin-arm64", platform: "darwin", arch: "arm64", libc: null },
  { pkg: "skif-darwin-x64", platform: "darwin", arch: "x64", libc: null },
  { pkg: "skif-linux-arm64-gnu", platform: "linux", arch: "arm64", libc: "glibc" },
  { pkg: "skif-linux-arm64-musl", platform: "linux", arch: "arm64", libc: "musl" },
  { pkg: "skif-linux-x64-gnu", platform: "linux", arch: "x64", libc: "glibc" },
  { pkg: "skif-linux-x64-musl", platform: "linux", arch: "x64", libc: "musl" },
  { pkg: "skif-win32-arm64", platform: "win32", arch: "arm64", libc: null },
  { pkg: "skif-win32-x64", platform: "win32", arch: "x64", libc: null },
];

const BINARY_NAME = process.platform === "win32" ? "skif.exe" : "skif";

function detectLibc() {
  if (process.platform !== "linux") return null;
  try {
    const report = typeof process.report?.getReport === "function" ? process.report.getReport() : null;
    if (report?.header?.glibcVersionRuntime) return "glibc";
    if (report) return "musl";
  } catch {
    // fall through to the glibc default
  }
  return "glibc";
}

function candidatePackages() {
  const libc = detectLibc();
  return PLATFORM_PACKAGES.filter(
    (entry) =>
      entry.platform === process.platform &&
      entry.arch === process.arch &&
      (entry.libc === null || entry.libc === libc),
  ).map((entry) => entry.pkg);
}

function resolveBinaryPath() {
  const override = process.env.SKIF_BINARY;
  if (override) return override;

  for (const pkg of candidatePackages()) {
    try {
      const resolved = require.resolve(`${pkg}/bin/${BINARY_NAME}`);
      if (fs.existsSync(resolved)) return resolved;
    } catch {
      // not installed, try the next candidate
    }
  }

  for (const pkg of candidatePackages()) {
    try {
      const candidate = path.join(path.dirname(require.resolve(`${pkg}/package.json`)), "bin", BINARY_NAME);
      if (fs.existsSync(candidate)) return candidate;
    } catch {
      // not installed, try the next candidate
    }
  }

  return null;
}

function unsupportedPlatformMessage() {
  const supported = PLATFORM_PACKAGES.map(
    (entry) => `  ${entry.platform}/${entry.arch}${entry.libc ? ` (${entry.libc})` : ""}`,
  ).join("\n");
  const libc = detectLibc();
  const host = `${process.platform}/${process.arch}${libc ? ` (${libc})` : ""}`;
  const expected = candidatePackages();

  if (expected.length === 0) {
    return (
      `skif: no prebuilt binary is published for ${host}.\n\n` +
      `Supported platforms:\n${supported}\n\n` +
      `Install from source instead: cargo install skif-cli\n` +
      `Or point SKIF_BINARY at a skif executable.`
    );
  }
  return (
    `skif: the platform package for ${host} is not installed.\n\n` +
    `Expected: ${expected.join(", ")}\n\n` +
    `This usually means optional dependencies were disabled for the install\n` +
    `(for example --no-optional or --omit=optional). Reinstall with them enabled,\n` +
    `or point SKIF_BINARY at a skif executable.`
  );
}

module.exports = { resolveBinaryPath, unsupportedPlatformMessage, PLATFORM_PACKAGES };
