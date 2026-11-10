# Migrating to skif and adopting it on an existing repository

The full guide, with what is translated from each tool, is <https://goldziher.github.io/skif/guides/migrating/>.

## Translate the old configuration

`skif migrate` reads the configuration of sherif (the `sherif` key of `package.json`), syncpack (`.syncpackrc*`,
`syncpack.config.*`, the `syncpack` key) and knip (`knip.json(c)`, `knip.{js,ts,...}`, the `knip` key), plus the
flags of the `package.json` scripts that run them, and writes a `skif.toml`. It executes nothing and deletes no file;
the old configuration stays where it is.

```sh
skif migrate --dry-run --format json   # print the skif.toml and the report, write nothing
skif migrate                           # write ./skif.toml for every tool found (--from auto)
skif migrate --from knip               # one tool; --from sherif,syncpack,knip for several
skif migrate --merge                   # add the translation to an existing skif.toml, keeping its comments
```

- `--profile exact` (default) keeps only the checks the old tools ran: every other rule skif enables by default is
  set to `off` with a `# skif migrate: not checked by <tools>` comment, and `[js] enabled = false` when knip is not
  migrated. `--profile full` keeps skif's defaults for the rest.
- Without `--merge` or `--force`, an existing `skif.toml` is not overwritten.
- `--strict` exits `1` when any construct needs a human.

Read the report before committing. Each note has the file, the key path and a letter: `M` translated one to one,
`A` translated with a behaviour difference the note explains, `D` covered by skif's defaults, `H` not translated
because the value is not static or unclear, `X` no equivalent in skif. Resolve every `H` and `X` with the user. The
report also lists the extra checks skif runs that the old tools did not.

Then run `skif` once: a knip pattern copied as written that skif's regular expression engine does not support is
reported there. Replace the scripts and CI jobs that run sherif, syncpack or knip with `skif` once the report is
clean; `skif migrate` does not edit them. knip reports files in `fixtures` and `__fixtures__` directories, skif does
not unless `[js] include-fixtures = true`.

## Adopting skif

A repository rarely starts clean. Record the current findings, fix them over time, and keep CI from accepting new
ones:

```sh
skif baseline write                      # record every error and warning in skif.baseline.toml
skif                                     # clean: skif.toml now extends the baseline
skif baseline prune                      # after fixing some findings, drop the entries that match nothing
skif baseline check --base origin/main   # in CI: exit 1 when the baseline gained entries
```

- `write` adds one `[[ignore]]` entry per finding (rule, package, dependency, or file for findings about a file) and
  adds the baseline to the `extends` of the root's `skif.toml`. Version findings are recorded as `name@specifier`, so
  only that version is silenced.
- Give `write` and `prune` the lint flags the CI run uses (`--strict`, `--production`, `--config ci.toml`), so the
  baseline holds the findings that run reports. With several projects, `-r` gives each its own baseline.
- Edit a `reason` to say why an entry stays; `write` and `prune` keep edited reasons. Do not add entries by hand to
  hide a new finding: fix it or write a targeted `[[ignore]]` with a reason in `skif.toml`.
- `check --base` reads the old file with `git show`; a shallow CI checkout needs `git fetch origin main` first.
- `info` findings, and findings with neither a package nor a dependency, are not recorded; `write` lists them on
  stderr.
