---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Source Command Scans Read the Files the Module Graph Reaches

## Context and Problem Statement

The `execa`, `zx` and `nano-spawn` plugins read the commands that source files run (`execa("tsx", ["build.ts"])`) as
scripts ([ADR 0017](0017-scripts-binaries-and-working-directories.md)). They searched every own source file of a
package, whether or not anything imports or runs it. A library a workspace root lists enables its plugin in every
member, which inherits it, so each member read all of its files, test fixtures included: on a large monorepo whose
member sources are mostly fixtures, reading them took most of the run. A fixture's `execa("x")` also ran `x` for the
package, although no code of the package runs it, while [ADR 0014](0014-files-the-js-analysis-reads-walk-git-index-fixtures.md)
reads no fixture nothing reaches, and the `child_process` calls of ADR 0017 count only in reachable files. Which source
files do these plugins read?

## Decision Drivers

* A command counts where code that runs is written, as an import does
* No file is read only to be searched: the scans take the texts the module graph read
* A file a command runs is an entry wherever the command is found, so what it reaches is used
* The output does not depend on the order the files are found in

## Considered Options

* Search the files the module graph reaches, as knip does, and build the graph again while the commands found add
  entries
* Search a member only when it lists the library itself, and every own file of the package that does
* Leave out the files that match test-fixture globs

## Decision Outcome

Chosen option: "Search the files the module graph reaches", because a command in a file nothing runs is no command the
package runs, and the graph has read those files already.

### Passes

* A plugin whose findings all come from its package's own sources says so (`Plugin::scans_reached_sources`). The
  resolvers run plugins in passes (`PluginPass`): before the graph, such a plugin gives what its `[js.plugins]` globs
  give and searches no file; after it, it searches only the files a `PackageSources::reached` listing holds.
* The analysis keeps, while building the graph, the texts of the modules of a package that has such a plugin, and hands
  each package the reached files below its directory (`InputSource::reached_inputs`). The plugins keep their other rules:
  only the JavaScript and TypeScript files a listing of the package's sources would hold, without the files of a member
  below it, which are that member's.
* What they find is added to what the plugins gave before the graph, and the settings of every package are derived
  again, so a command in another directory credits the package there as before.

### Reaching more files

* A command can run a file (`execa("node", ["scripts/next.mjs"])`), which becomes an entry. When what the scans find
  changes anything but the packages and binaries named, the entries and the graph are built again, taking the texts the
  previous graph kept, and the new graph's files are searched in turn.
* Each graph reaches every file the previous one did, since entries are only added, so the search ends once a graph
  reaches no new file. It is bounded at 16 searches; a longer chain of scripts running one another leaves the files of
  its last links unsearched.

### Consequences

* Good, because a fixture or any other file nothing imports or runs is not read, and its calls run nothing.
* Good, because a member that inherits the library searches only its reached files, whatever its fixture tree holds.
* Bad, because a command in a file nothing reaches no longer credits the package that provides its binary, so the
  package can become an `unused-dependencies` finding, and the file it runs is no entry; the file holding the command
  is reported by `unused-files` (unless it is a fixture) as before.
* Bad, because a chain of scripts that run one another builds the graph once per link.
* Neutral, because the commands a reached file runs come after the other plugins' findings of its package instead of in
  plugin order.

### Confirmation

* `crates/skif-js/tests/reached_sources.rs`: `execa_calls_are_read_from_the_files_the_graph_reaches`.
* `crates/skif-js-plugins/tests/plugins.rs`: `the_source_scans_of_a_split_pass_search_only_the_reached_files`.
* `crates/skif-js-plugins/src/sources.rs`: `a_reached_listing_searches_only_the_files_it_holds`.
* The `fixture_tree` group of `crates/skif-js/benches/analysis.rs`.
