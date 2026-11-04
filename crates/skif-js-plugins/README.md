# skif-js-plugins

JavaScript and TypeScript tool knowledge for skif.

- `scripts`: parses `package.json` scripts into the binaries, entry files and dependencies they reference.
- `plugin`, `plugins`, `registry`: the declarative plugin table of ADR 0011 (extended by ADR 0022 and ADR 0025). A plugin is a `PluginSpec` row
  (enablers, config, entry, production and project globs, provided binaries) with an optional resolver that reads the
  tool's config statically and reports the dependencies, entries and scripts it references. Configs are never
  executed (ADR 0010).
- `resolve_plugins`: the entry point the module graph calls, given a `PluginContext` for one workspace.

The table holds 60 plugins: `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`, `next`, `storybook`,
`playwright`, `tailwind`, `postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`, `husky`, `lint-staged`,
`commitlint`, `changesets`, `github-actions`, `mastra`, `aws-cdk`, `browser-extension`, `tsdown`, `oxlint`, `angular`,
`karma`, `esbuild`, `svelte`, `nuxt`, `astro`, `drizzle`, `ts-node`, `tsx`, `nx`, `cypress`, `mocha`, `sveltekit`,
`prisma`, `graphql-codegen`, `docusaurus`, `vitepress`, `wrangler`, `expo`, `react-native`, `metro`, `release-it`,
`semantic-release`, `lefthook`, `simple-git-hooks`, `stylelint`, `markdownlint`, `cspell`, `nodemon`, `pm2`, `pnpm`,
`yarn` and `syncpack`.

## Acknowledgements

The behaviour of each plugin (enablers, config file names, entry patterns and which configuration values name
dependencies) follows [knip](https://github.com/webpro-nl/knip) (ISC licence), checked at commit
`da07b6e9887f9a1eb08545da29755b7f9d309bca`. The implementation is independent. `aws-cdk` and `browser-extension` have no knip counterpart at that commit.
