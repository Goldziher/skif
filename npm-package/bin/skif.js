#!/usr/bin/env node
import { resolve } from "node:path";
import { run } from "../index.js";

// Under `bazel run` the binary starts in the runfiles tree; enter the directory the user ran Bazel from, as the native
// binary does (ADR 0112). It is done here, not in the addon, so that Node's own current directory stays in step.
function enterBazelRunDirectory() {
  const workspace = process.env.BUILD_WORKSPACE_DIRECTORY;
  if (!workspace) return true;
  const directory = resolve(workspace, process.env.BUILD_WORKING_DIRECTORY || "");
  try {
    process.chdir(directory);
    return true;
  } catch (error) {
    process.stderr.write(`error: cannot enter the \`bazel run\` directory ${directory}: ${error.message}\n`);
    return false;
  }
}

// Set the exit code instead of calling process.exit so the runtime drains stdout and stderr itself.
process.exitCode = enterBazelRunDirectory() ? run(process.argv.slice(2)) : 2;
