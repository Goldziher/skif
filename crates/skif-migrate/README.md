# skif-migrate

One-shot translation of sherif, syncpack and knip configuration into a `skif.toml`. It is the library behind `skif migrate`.

- `migrate(&[ToolInput], &Options) -> Result<Migration, MigrateError>`: pure; the caller reads the files and passes their text.
- JavaScript and TypeScript configs (`knip.ts`, `.syncpackrc.js`, ...) are read statically through `skif-js-syntax`; a value that
  is only known at run time becomes a note that needs a human. Nothing is executed.
- The generated text is parsed back with `skif_config::Config::from_toml_str` before it is returned.
- `Migration::merge_into` combines the output with an existing `skif.toml`.
- A `Note` records how a construct was handled, with the file and the key path it came from, classed `Exact`,
  `Approximate`, `Default`, `Human` or `Unsupported`. Every construct not translated one to one has one; `Human` and
  `Unsupported` notes need a human.

See the [migration guide](https://github.com/polylint/skif/blob/main/website/src/content/docs/guides/migrating.mdx) and
[ADR 0003](https://github.com/polylint/skif/blob/main/adrs/0003-configuration-suppressions-baselines-and-migration.md).
