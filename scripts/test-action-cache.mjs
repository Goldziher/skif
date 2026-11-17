// Tests the GitHub expressions of the Action's cache steps (action.yml) by evaluating them: the variables the run
// step hands skif, and when the store is saved. `a && '' || b` always gives `b`, since `''` is falsy; the cases
// below fail on it.
//
// Usage: node --test scripts/test-action-cache.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const action = readFileSync(new URL("../action.yml", import.meta.url), "utf8");

// Evaluates the subset of GitHub's expression syntax the Action uses: context paths, string literals, `==`, `!=`,
// `&&`, `||`, `!`, parentheses, `format()` and `always()`. `&&` and `||` return an operand, as GitHub's do.
const truthy = (value) => value !== "" && value !== false && value !== null && value !== undefined;

function evaluate(expression, context) {
  const tokens = expression.match(/'(?:[^']|'')*'|==|!=|&&|\|\||[()!,]|[A-Za-z_][\w.-]*/gu) ?? [];
  assert.equal(tokens.join("").length, expression.replaceAll(/\s+/gu, "").length, `cannot read: ${expression}`);
  let at = 0;
  const lookup = (path) => path.split(".").reduce((value, key) => value?.[key], context) ?? "";
  const primary = () => {
    const token = tokens[at++];
    if (token === "(") {
      const value = or();
      at++;
      return value;
    }
    if (token === "!") return !truthy(primary());
    if (token.startsWith("'")) return token.slice(1, -1).replaceAll("''", "'");
    if (tokens[at] !== "(") return lookup(token);
    at++;
    const args = [];
    while (tokens[at] !== ")") {
      args.push(or());
      if (tokens[at] === ",") at++;
    }
    at++;
    if (token === "always") return true;
    if (token === "format") return args[0].replaceAll(/\{(\d+)\}/gu, (_, index) => String(args[Number(index) + 1]));
    throw new Error(`unknown function ${token}`);
  };
  const comparison = () => {
    const left = primary();
    if (tokens[at] !== "==" && tokens[at] !== "!=") return left;
    const equal = tokens[at++] === "==";
    return (left === primary()) === equal;
  };
  const and = () => {
    let value = comparison();
    while (tokens[at] === "&&") {
      at++;
      const right = comparison();
      if (truthy(value)) value = right;
    }
    return value;
  };
  function or() {
    let value = and();
    while (tokens[at] === "||") {
      at++;
      const right = and();
      if (!truthy(value)) value = right;
    }
    return value;
  }
  const value = or();
  assert.equal(at, tokens.length, `tokens left over in: ${expression}`);
  return value;
}

// The expression of `NAME: ${{ ... }}` in the env block of the run step.
function variable(name) {
  const found = action.match(new RegExp(`^ +${name}: \\$\\{\\{(.*)\\}\\}$`, "mu"));
  assert.ok(found, `action.yml sets no ${name}`);
  return found[1].trim();
}

// The `if:` of the step named `name`, folded onto one line.
function condition(name) {
  const step = action.split(/^ {4}- name: /mu).find((text) => text.startsWith(`${name}\n`));
  assert.ok(step, `action.yml has no step ${name}`);
  const found = step.match(/^ {6}if: >-\n((?: {8}.*\n)+)/mu);
  assert.ok(found, `the step ${name} has no folded if`);
  return found[1].replaceAll(/\s+/gu, " ").trim();
}

const run = (inputs) => {
  const context = { inputs: { cache: "true", "cache-max-size": "256MiB", ...inputs }, runner: { temp: "/tmp/runner" } };
  return Object.fromEntries(
    ["SKIF_CACHE", "SKIF_CACHE_DIR", "SKIF_CACHE_MAX_SIZE"].map((name) => [name, evaluate(variable(name), context)]),
  );
};

test("the evaluator treats an empty string as false, as GitHub does", () => {
  assert.equal(evaluate("inputs.cache == 'true' && '' || '0'", { inputs: { cache: "true" } }), "0");
  assert.equal(
    evaluate("inputs.a-b != 'x' && format('{0}/{1}', 'p', inputs.a-b) || ''", { inputs: { "a-b": "q" } }),
    "p/q",
  );
});

test("with the cache on, the run is given the store and is not told to turn the cache off", () => {
  assert.deepEqual(run({}), {
    SKIF_CACHE: "",
    SKIF_CACHE_DIR: "/tmp/runner/skif-cache",
    SKIF_CACHE_MAX_SIZE: "256MiB",
  });
});

test("with the cache off, the run is told so and given no store", () => {
  assert.deepEqual(run({ cache: "false" }), { SKIF_CACHE: "0", SKIF_CACHE_DIR: "", SKIF_CACHE_MAX_SIZE: "" });
});

test("no expression of the Action has an empty string between && and ||", () => {
  assert.deepEqual(action.match(/&&\s*''\s*\|\|/gu), null);
});

const saves = ({ event = "push", ref = "refs/heads/main", save = "", hit = "false", cache = "true" }) =>
  evaluate(condition("Save the skif cache"), {
    inputs: { cache, "install-only": "false", "cache-save": save },
    steps: { store: { outputs: { "cache-primary-key": "skif-key", "cache-hit": hit } } },
    github: { event_name: event, ref, event: { repository: { default_branch: "main" } } },
  }) === true;

test("by default only a push or a scheduled run of the default branch saves", () => {
  assert.equal(saves({ event: "push" }), true);
  assert.equal(saves({ event: "schedule" }), true);
  for (const event of ["workflow_dispatch", "issue_comment", "workflow_run", "repository_dispatch", "merge_group"]) {
    assert.equal(saves({ event }), false, event);
  }
  assert.equal(saves({ event: "push", ref: "refs/heads/feature" }), false);
});

test("cache-save decides for every event but a pull request", () => {
  assert.equal(saves({ event: "workflow_dispatch", save: "true" }), true);
  assert.equal(saves({ event: "push", ref: "refs/heads/feature", save: "true" }), true);
  assert.equal(saves({ event: "push", save: "false" }), false);
  for (const event of ["pull_request", "pull_request_target"]) {
    assert.equal(saves({ event, save: "true" }), false, event);
    assert.equal(saves({ event }), false, event);
  }
});

test("nothing is saved with the cache off or after an exact hit", () => {
  assert.equal(saves({ cache: "false" }), false);
  assert.equal(saves({ hit: "true" }), false);
});
