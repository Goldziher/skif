# skif-migrate

One-shot translation of sherif, syncpack and knip configuration into a `skif.toml`. It is the library behind `skif migrate`.

- `migrate(&[ToolInput], &Options) -> Migration`: pure; the caller reads the files and passes their text.
- JavaScript and TypeScript configs (`knip.ts`, `.syncpackrc.js`, ...) are read statically through `skif-js-syntax`; a value that
  is only known at run time becomes a note that needs a human. Nothing is executed.
- The generated text is parsed back with `skif_config::Config::from_toml_str` before it is returned.
- `Migration::merge_into` combines the output with an existing `skif.toml`.
- Every construct that was not copied one to one is a `Note` (`Exact`, `Approximate`, `Default`, `Human`, `Unsupported`) with the file and the
  key path it came from.

See the [migration guide](https://github.com/Goldziher/skif/blob/main/website/src/content/docs/guides/migrating.mdx) and
[ADR 0039](https://github.com/Goldziher/skif/blob/main/adrs/0039-skif-migrate-a-one-shot-translator-from-sherif-syncpack-and-knip.md).
