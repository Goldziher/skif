"use strict";

const path = require("node:path");
const fs = require("node:fs");
const { promisify } = require("node:util");
const glob = require("glob");

const readFile = promisify(fs.readFile);

function Cache(limit) {
  this.limit = limit || 100;
  this.entries = new Map();
}

Cache.prototype.get = function get(key) {
  if (!this.entries.has(key)) return undefined;
  const value = this.entries.get(key);
  this.entries.delete(key);
  this.entries.set(key, value);
  return value;
};

Cache.prototype.set = function set(key, value) {
  if (this.entries.size >= this.limit) {
    this.entries.delete(this.entries.keys().next().value);
  }
  this.entries.set(key, value);
  return this;
};

const cache = new Cache(50);

async function load(pattern, cwd) {
  const files = await new Promise((resolve, reject) => {
    glob(pattern, { cwd }, (error, matches) => (error ? reject(error) : resolve(matches)));
  });
  const out = [];
  for (const file of files) {
    const full = path.resolve(cwd, file);
    let text = cache.get(full);
    if (text === undefined) {
      text = await readFile(full, "utf8");
      cache.set(full, text);
    }
    out.push({ file, size: text.length, lines: text.split(/\r?\n/).length });
  }
  return out.sort((a, b) => b.size - a.size);
}

function summarize(entries) {
  return entries.reduce(
    (acc, { size, lines }) => ({ size: acc.size + size, lines: acc.lines + lines, files: acc.files + 1 }),
    { size: 0, lines: 0, files: 0 },
  );
}

switch (process.env.MODE) {
  case "summary":
    load("**/*.js", process.cwd()).then((entries) => console.log(summarize(entries)));
    break;
  case "list":
    load("**/*.js", process.cwd()).then((entries) => entries.forEach((e) => console.log(e.file)));
    break;
  default:
    console.error("unknown mode");
    process.exitCode = 1;
}

module.exports = { Cache, load, summarize };
module.exports.default = load;
