---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0001", "0026"]
---

# Speed Benchmarks on a Pinned GitHub-Hosted Runner

## Context and Problem Statement

[ADR 0001](0001-mission-and-scope.md) sets the speed targets and has `mise run bench` measure them on Apple silicon.
[ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md) has `mise run bench:publish` write the summary to
`tools/bench/results` and refresh the published speed tables. Both run on the maintainer's machine: nobody else can
repeat a published number, the summary recorded only the CPU model, the logical CPUs and the load, and the next
publication may come from another machine, toolchain or background load without the tables saying so.

The corpus repositories are already pinned to commits and the comparison tools (sherif, syncpack, knip) to versions
in `tools/bench/corpus.toml`, and hyperfine and Node are locked in `mise.lock`. What is not fixed is the machine, the
Rust toolchain, the package manager shims, and what a reader is told about them.

Where should the published speed numbers be measured, and what does a measurement record about where it ran?

## Decision Drivers

* Anyone can repeat a published measurement: same runner image, toolchain and inputs, started from the repository
* A published number says exactly what produced it: skif commit and compiler, harness commit, tool versions, runner
  image, machine and the run
* Nothing that is timed comes from a cache, and nothing compared is unpinned
* The repository's commits stay signed by the maintainer: a workflow does not commit
* Shared CI machines are noisy; the published figures must stay meaningful anyway

## Considered Options

* A manually dispatched workflow on a pinned GitHub-hosted runner image, imported locally
* Keep measuring on the maintainer's machine, with the machine record added
* A scheduled or per-pull-request benchmark workflow that commits its results
* A self-hosted or dedicated benchmark machine

## Decision Outcome

Chosen option: "A manually dispatched workflow on a pinned GitHub-hosted runner image, imported locally", because it
is the only option anyone can repeat with nothing but the repository, and the machine record makes the remaining
variation visible instead of hiding it.

**Workflow.** `.github/workflows/bench.yml` runs only on `workflow_dispatch`, with the inputs `ref` (the commit, tag
or branch of skif to measure; the dispatched ref by default), `runner` (a choice of pinned image labels,
`ubuntu-24.04` or `ubuntu-24.04-arm`; never a `-latest` label, which moves to a new image without notice), `runs` and
`warmup` (the harness defaults, 15 and 3) and `only` (some repositories of the bench corpus). The harness,
`corpus.toml` and the mise configuration come from the dispatched ref; the measured skif is built from `ref`, so an
older release can be measured by the current harness, and the summary records both commits. One bench run runs at a
time (a concurrency group that queues rather than cancels). The job has `contents: read` and commits nothing.

**What is pinned.** The runner image label. The Rust toolchain: each checkout builds with the `rust-version` of its
own `Cargo.toml`, the MSRV the CI job already builds against, rather than `stable` on the day of the run; a workflow
variable would be one more version surface to keep in step, and the measured ref's own declaration makes a re-run of
an old ref use the toolchain it was released against. Builds are `--locked`. hyperfine, Node and pnpm come from
`mise.lock` (installed with `--locked`), corepack is a pinned version, and each corpus repository's package manager
comes from its own `packageManager` at the pinned commit. The harness refuses a corpus repository whose `rev` is not a
full commit SHA (as before) and, newly, a comparison tool whose version in `corpus.toml` is not exact
(`major.minor.patch`, no range or dist-tag), so an unpinned input fails the run before anything is timed. The Rust
cache holds compiled dependencies only; the corpus, its installs and the generated workspaces are written outside
both target directories, so no timed input comes from a cache.

**What is recorded.** `summary.json` becomes schema 3. It replaces `host`, `cpu` and `cores` with a `machine` record:
the operating system (`PRETTY_NAME` of `/etc/os-release`, `sw_vers` on macOS), the kernel (`uname -sr`), the
architecture, the CPU model (`/proc/cpuinfo`, `lscpu` where Arm CPUs name none there, `sysctl` on macOS), the logical
CPUs, those the process may use and the physical cores, the total memory, the cgroup CPU limit, a hypervisor hint
(`lscpu`, `systemd-detect-virt`), and in a GitHub Actions job the runner facts its environment holds (`ImageOS`,
`ImageVersion`, `RUNNER_OS`, `RUNNER_ARCH`, `RUNNER_ENVIRONMENT`, `RUNNER_NAME`) and the URL of the run. It adds a
`toolchain` record (the `--version` of `rustc`, the one `RUSTUP_TOOLCHAIN` selects, and of `node`, `npm`, `pnpm`,
`yarn` and `bun`), the harness commit beside the measured skif commit, and `corpus_only` for a narrowed run. A fact
that cannot be read is `null`; reading the machine never fails a run. `summary.md` gets the same as its header, and
the published tables a sentence naming the runner image, the machine, the compiler and a link to the run.

**Publishing.** The workflow writes `summary.md` to the job summary and uploads `summary.json`, `summary.md` and the
raw hyperfine exports as the artifact `bench-<commit>-<runner>`. `mise run bench:import -- <run-id>` downloads it with
`gh`, refreshes the tables with `refresh-numbers.mjs --bench` and puts the summary into `tools/bench/results`, for the
maintainer to commit, signed, like any other change. `refresh-numbers.mjs` refuses a partial (`only`) run and an older
schema, so the import then changes nothing.

**Local runs.** `mise run bench:publish` is removed: with two ways to publish, numbers from an unrecorded machine and
numbers from the runner would end up in the same tables. `mise run bench` stays for measuring while developing,
writes to `target/bench-results` and is not published.

### Consequences

* Good, because a published number names the commit, compiler, runner image, CPU model and run it came from, and the
  run can be repeated by dispatching the workflow with the same inputs.
* Good, because an unpinned comparison tool or corpus repository fails the run instead of silently measuring whatever
  the registry or the branch holds that day.
* Good, because the results reach the repository through a signed local commit; the workflow needs no write access.
* Neutral, because the ADR 0001 target checks are still computed and shown, but a run does not fail on them
  (`--no-enforce`): a shared runner is not where a regression gate belongs.
* Bad, because GitHub-hosted runners are shared virtual machines: a neighbour's load moves a run's times by several
  percent, and the same image label may land on different CPU models. The tables publish medians, which one slow
  run does not move, over 15 measured runs after 3 warmup runs; the warmups fill the file cache and the first-run
  costs of the shared machine, and `runs` and `warmup` can be raised for a closer figure. Runs compare only on the same
  image and CPU model, which the record names, and the ratios against knip, sherif and syncpack, which share the
  machine and its noise, are the result rather than the absolute times.
* Bad, because a runner image label is retired by GitHub after some years, and its successor is a new pinned label
  and a new baseline.
* Bad, because the toolchain follows the MSRV, which may be older than the compiler a release is built with; the
  record names the `rustc` used, and the MSRV moves with the releases.

### Confirmation

* `tools/bench/src/provenance.rs`: `reads_the_operating_system_name_from_os_release`,
  `reads_the_total_memory_from_meminfo`, `reads_the_cgroup_cpu_limit`, `reads_the_cpu_topology_of_x86_and_arm`,
  `reads_the_cpu_model_and_hypervisor_from_lscpu`, `reads_the_github_runner_from_the_environment`,
  `a_version_is_the_first_line_of_its_output` and `the_machine_record_never_fails_and_names_the_architecture`.
* `tools/bench/src/run.rs`: `summary_markdown_is_formatted` and
  `the_machine_line_names_unknown_facts_and_leaves_out_missing_ones`.
* `tools/bench/src/corpus.rs`: `rejects_a_comparison_tool_version_that_is_not_exact` and `rejects_branch_pins`.
* `tools/bench/src/main.rs`: `run_narrows_the_corpus_and_names_the_skif_source`.
* `tools/bench/refresh-numbers.test.mjs`: the provenance sentence of a runner and of a local summary, the machine line,
  and the refusal of a partial run and of an older schema.
* `scripts/test-check-scripts.sh`: the `bench-import` cases (usage, a failed download, two artifacts, an incomplete
  artifact, a refused summary, a successful import).
* `actionlint`, `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A manually dispatched workflow on a pinned GitHub-hosted runner image, imported locally

* Good, because anyone with the repository can repeat a measurement on the same image.
* Good, because the maintainer decides when numbers are refreshed, typically once per release.
* Bad, because shared runners are noisier than a quiet dedicated machine.

### Keep measuring on the maintainer's machine, with the machine record added

* Good, because a quiet local machine is less noisy.
* Bad, because no one else can repeat the numbers, and the machine changes with the maintainer's hardware.

### A scheduled or per-pull-request benchmark workflow that commits its results

* Good, because the numbers would never be stale.
* Bad, because the workflow would need write access and its commits would not be the maintainer's signed commits.
* Bad, because a run on every pull request queues behind and competes with CI, and noise would make the committed
  numbers move without a change in skif.

### A self-hosted or dedicated benchmark machine

* Good, because a dedicated machine gives the lowest noise.
* Bad, because it has to be bought, secured and kept running, and others still cannot reproduce on it.

## More Information

* Amends: [ADR 0001](0001-mission-and-scope.md) (the performance targets are published from the `bench` workflow on a
  pinned GitHub-hosted runner, not measured on Apple silicon) and
  [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md) (the "Speed" paragraph: `mise run bench:publish` is
  replaced by the `bench` workflow and `mise run bench:import`)
* The harness and its commands: [tools/bench/README.md](../tools/bench/README.md)
* GitHub-hosted runner images: <https://github.com/actions/runner-images>
* See the [ADR index](index.md).
