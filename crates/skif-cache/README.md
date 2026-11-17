# skif-cache

The disk cache store of [skif](https://github.com/polylint/skif), a dependency and package-manager linter for
JavaScript and TypeScript with first-class monorepo support. `DiskStore` implements `skif_core::CacheStore` over one
cache location holding a store per base directory (the worktrees of one git repository share a store, two
repositories never do): it keeps, per namespace, what the base's last complete run used, caps the whole location in
size, writes atomically behind a try-lock and treats anything it cannot read as empty.

It is the one skif library crate that reads and writes files outside the `FileSystem` trait, scoped to its own cache
directory, and it reads no environment: the host passes the location and the platform cache directory
(`default_location`). The `skif` facade re-exports it as `skif::cache` behind the `cache-disk` feature.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D @polylint/skif`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
