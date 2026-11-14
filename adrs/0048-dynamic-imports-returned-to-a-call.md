---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Dynamic Imports Returned to a Call: Lazy-Component Loaders Read `default`, Other Calls the Whole Module

## Context and Problem Statement

A function that returns an `import()` is the loader of a lazy component: `lazy(() => import("./Page.js"))`. skif
counted the `import()` as unbound and so as a use of every export of the module
([ADR 0018](0018-export-namespace-and-member-rules.md)), and never reported a named export that a page module also
has. React's `lazy` and the loaders built on it render the default export only. knip counts every `import()` an arrow
function returns to a call as a use of `default`, whatever the call. Which exports does such an `import()` use, for
the loaders skif knows and for any other call?

## Decision Drivers

* A use skif cannot see must not be reported as unused: a call that receives the loader may read any export
* The loaders whose contract is the default export are few and documented
* `skif explain-usage` says why an export counts as used

## Considered Options

* Credit `default` for every call, as knip does
* Credit `default` for the known lazy-component loaders, and keep a use of the whole module for any other call
* Keep a use of the whole module for every call

## Decision Outcome

Chosen option: "Credit `default` for the known lazy-component loaders", because their contract is the default export,
while another call may read a named export: TanStack Router's `lazyRouteComponent(() => import("./x"), "Named")` and a
project's own route table read the export they are told to, so knip's rule reports exports that are used.

* A function passed as the first argument of a known loader that returns `import("x")` (an arrow function with that
  body, one `return import("x")`, also awaited) uses the default export of `x` only. The known loaders are `lazy` of
  `react` and `preact/compat` (also as `React.lazy` on a default or namespace import), the default export of
  `next/dynamic`, and the default export of `@loadable/component` when its options cannot pick another export (no
  `resolveComponent`, no spread, an object literal or none).
* `import("x").then((m) => m.Name)` inside the loader reads `Name`, as any `.then` callback on an `import()` does.
* A function returning `import("x")` passed to any other call keeps the namespace of `x` used as a whole. The parser
  records the call as written (`wrapLazy`, `routes.define`) on the import's binding, and `explain-usage` describes the
  use as "the namespace, used as a whole, returned from a callback passed to wrapLazy".

### Consequences

* Good, because the named exports of a lazily loaded page are reported, as knip reports them.
* Good, because a loader that reads another export never makes skif report a used export.
* Bad, because skif reports nothing for a project's own wrapper around `lazy` where knip does; the explanation names
  the wrapper, and a future setting may opt into knip's rule.
* Neutral, because the parse output changes, so the cache schema is raised.
