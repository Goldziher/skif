// Loads the built npm addon from <directory> and runs `skif --version` through it.
//
// Usage: node scripts/load-addon.mjs <directory holding index.js and the .node file>
//
// A load failure (wrong libc, missing symbol, absent binary) or a non-zero exit fails the job before publish.
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const directory = process.argv[2];
if (!directory) {
  console.error("usage: load-addon.mjs <directory>");
  process.exit(2);
}

const addon = await import(pathToFileURL(join(resolve(directory), "index.js")).href);
if (typeof addon.run !== "function") {
  console.error("the addon does not export run()");
  process.exit(1);
}
const code = addon.run(["--version"]);
if (code !== 0) {
  console.error(`run(["--version"]) exited ${code}`);
  process.exit(1);
}

// The synchronous `lint` throws the structured failure `lintAsync` returns: an Error with `kind` (and `path`).
if (typeof addon.lint === "function") {
  let thrown;
  try {
    addon.lint(join(resolve(directory), "no-such-project"));
  } catch (error) {
    thrown = error;
  }
  if (!(thrown instanceof Error) || typeof thrown.kind !== "string") {
    console.error(`lint() on a missing path must throw an Error with a kind, got ${String(thrown)}`);
    process.exit(1);
  }
}
