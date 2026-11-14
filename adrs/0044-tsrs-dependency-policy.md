---
status: proposed
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# tsrs Dependency Policy: One Crate, Exact Pins, and the Process-Level Constraints of the Checker

## Context and Problem Statement

Type-aware analysis ([ADR 0043](0043-type-aware-analysis-on-tsrs.md)) depends on tsrs, published as pre-release
crates (`tsrs_*` `7.1.0-beta.tsrs1`). Beyond its API, tsrs makes choices for the whole process it runs in:

* Programs live in leaked arenas: `new_program` returns a `&'static Program`, and only the unsafe
  `free_unshared_program` releases the memory.
* On unix it reserves a fixed 32 GiB range of address space for its pointers and aborts the process when it cannot;
  the `plain-ptrs` feature uses plain pointers instead.
* It reads `TSRS_*` and `RAYON_NUM_THREADS` from the environment and runs a global worker pool with large stacks.
* `tsrs_core` pins `unicode-id-start = "=1.1.2"`, which Cargo unifies with oxc's `unicode-id-start = "1"` for the
  whole workspace.
* It is Apache-2.0 and ships a NOTICE that includes the TypeScript and DefinitelyTyped notices.

How does skif take on this dependency so that hosts and users can trust its behaviour?

## Decision Drivers

* An embedding host must not be forced onto tsrs, nor have its process aborted or its address space claimed
* One version of tsrs at a time, upgraded deliberately with evidence
* License obligations are met in every artefact that contains tsrs
* skif's own rules (no global state, no environment input in the library,
  [ADR 0003](0003-library-api-and-embedding.md)) hold except where tsrs makes an exception unavoidable, and then the
  exception is written down

## Considered Options

* One boundary crate with exact pins, `plain-ptrs` where the reservation is unsafe, unwinding panics, and the
  constraints documented
* Depend on tsrs from `skif-js` directly
* Vendor the parts of tsrs skif needs

## Decision Outcome

Chosen option: "One boundary crate with exact pins, `plain-ptrs` where the reservation is unsafe, unwinding panics,
and the constraints documented", because it confines tsrs and its unsafe code to one crate, keeps hosts safe from
its process-level choices and makes every upgrade a deliberate, measured change.

### Boundary and pins

* Only `skif-js-types` depends on tsrs crates, each at the same exact version (`=`). `skif-js` depends on it behind
  its `type-aware` feature; the facade without default features has no tsrs crate in its tree.
* An upgrade of tsrs is its own change, carrying the parity run, the typed-versus-untyped comparison, the benchmark
  and the MSRV check (tsrs's `rust-version` must not exceed skif's).
* The `unicode-id-start` pin of tsrs applies to the whole workspace lock, oxc included, because Cargo unifies the
  version. oxc then classifies identifiers with the Unicode tables of that release; an upgrade of either side checks
  that the pin still resolves.

### Memory and unsafe code

* Each program is owned by a guard whose `Drop` calls `free_unshared_program`; facts are copied out before it drops.
  This is the only unsafe code in skif (`unsafe_code` is denied in `skif-js-types` and forbidden elsewhere), allowed
  at the one call with the reason beside it.
* A test runs the analysis 50 times on one thread in one process and fails when the arena bytes in use after a run
  differ from those after the first. With several checker threads the arenas tsrs's worker threads keep for
  themselves grow from run to run, and skif cannot give them back; long-lived hosts (`skif lsp`, `skif mcp`,
  `--watch`) therefore run untyped by default until tsrs releases them.

### Address space and panics

* The npm addon is built with `plain-ptrs`: a Node.js process holds V8's reservations and other addons, so a fixed
  reservation could collide and abort the host. The CLI uses `plain-ptrs` too unless the measured cost is above 10%.
* A panic inside tsrs is caught at the boundary and the affected packages fall back to the untyped analysis. The CLI's
  release profile therefore unwinds (`panic = "unwind"`), as the addon's profile already does.

### Environment and threads

* skif passes the bundled lib path and the checker count explicitly and never relies on `TSRS_*` variables; it does
  not set or clear them either. Variables a user sets reach tsrs, an exception to
  [ADR 0003](0003-library-api-and-embedding.md) recorded here.
* The worker pool of tsrs is global to the process and cannot be handed in. skif limits the checkers per program from
  its own `threads` setting; a single-threaded run uses tsrs's single-threaded mode.

### License

* `ATTRIBUTIONS.md` gets a "tsrs (and TypeScript)" section with the full NOTICE text, and the file ships in every
  binary archive, crate and npm package, as [ADR 0033](0033-attribution-and-vendoring.md) requires.

### Consequences

* Good, because hosts that do not want tsrs never build it, and the addon cannot abort Node.js through a reservation.
* Good, because upgrades are visible and measured.
* Bad, because the CLI binary grows by the checker, the emitter crates tsrs's compiler pulls in and the embedded
  `lib.*.d.ts` files; skif asks upstream for a compiler without the emitter.
* Bad, because unwinding panics make the CLI binary slightly larger and slower to unwind than aborting.

### Confirmation

* `scripts/check-pins.sh` fails unless every tsrs crate is pinned with `=` to one version.
* `scripts/check-dependency-tree.sh` fails when a crate other than `skif-js-types` depends on tsrs, or when `skif`
  without default features does.
* `repeated_analyses_free_their_arenas` (50 single-threaded runs, arena bytes in use stay at the first run's).
* `a_panicking_checker_falls_back_untyped`.
* The release workflow's archive and npm tarball checks fail when `ATTRIBUTIONS.md` lacks the tsrs section.

## Pros and Cons of the Options

### One boundary crate with exact pins

* Good, because the constraints are handled in one place and stay out of the rest of skif.
* Bad, because the boundary copies facts into owned types.

### Depend on tsrs from `skif-js`

* Good, because no extra crate.
* Bad, because tsrs types and unsafe code would spread through the analysis, and the optional dependency would be
  harder to keep optional.

### Vendor tsrs

* Good, because skif would control the code.
* Bad, because it is a full compiler that moves fast; skif would carry it alone.
