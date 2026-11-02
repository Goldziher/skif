#!/usr/bin/env node
import { run } from "../index.js";

// Set the exit code instead of calling process.exit so the runtime drains stdout and stderr itself.
process.exitCode = run(process.argv.slice(2));
