# skif-js-plugins

JavaScript and TypeScript tool knowledge for skif.

- `scripts`: parses `package.json` scripts into the binaries, entry files and dependencies they reference.
- `plugin`, `plugins`, `registry`: the declarative plugin table of ADR 0011 (extended by ADR 0022, ADR 0025, ADR 0029, ADR 0036, ADR 0049, ADR 0050 and ADR 0051). A plugin is a `PluginSpec` row
  (enablers, config, entry, production and project globs, provided binaries) with an optional resolver that reads the
  tool's config statically and reports the dependencies, entries and scripts it references. Configs are never
  executed (ADR 0010).
- `resolve_plugins`: the entry point the module graph calls, given a `PluginContext` for one workspace.

The table holds 190 plugins: `typescript`, `eslint`, `prettier`, `vite`, `vitest`, `jest`, `next`, `storybook`, `playwright`, `tailwind`,
`postcss`, `babel`, `webpack`, `rspack`, `rollup`, `tsup`, `turbo`, `husky`, `lint-staged`, `commitlint`,
`changesets`, `github-actions`, `mastra`, `aws-cdk`, `browser-extension`, `tsdown`, `oxlint`, `angular`, `karma`,
`esbuild`, `svelte`, `nuxt`, `astro`, `drizzle`, `ts-node`, `tsx`, `nx`, `cypress`, `mocha`, `sveltekit`, `prisma`,
`graphql-codegen`, `docusaurus`, `vitepress`, `wrangler`, `expo`, `react-native`, `metro`, `release-it`,
`semantic-release`, `lefthook`, `simple-git-hooks`, `stylelint`, `markdownlint`, `cspell`, `nodemon`, `pm2`, `pnpm`,
`yarn`, `syncpack`, `node`, `expo-router`, `tanstack-router`, `ava`, `biome`, `c8`, `dotenv`, `electron-vite`,
`gatsby`, `i18next-parser`, `jasmine`, `knex`, `lingui`, `msw`, `nest`, `netlify`, `nitro`, `nyc`, `oclif`,
`panda-css`, `plop`, `preconstruct`, `react-router`, `remark`, `remix`, `rolldown`, `rsbuild`, `rslib`, `sanity`,
`sentry`, `size-limit`, `stryker`, `svgo`, `svgr`, `swc`, `tsd`, `typedoc`, `unbuild`, `unocss`, `vercel`, `vike`, `tap`, `oxfmt`, `bun`, `webdriver-io`,
`pre-commit`, `parcel`, `next-intl`, `dependency-cruiser`, `commitizen`, `electron-forge`, `serverless-framework`, `github-action`, `vue`, `unplugin-auto-import`, `unplugin-vue-components`, `unplugin-vue-router`, `mdx`, `next-mdx`, `starlight`, `pino`, `execa`, `zx`, `nano-spawn`, `azure-pipelines`, `sapper`, `tauri`, `capacitor`, `convex`, `payload`, `wxt`, `hardhat`, `temporal`, `react-email`, `openapi-ts`, `danger`, `changelogen`, `purgecss`, `fumadocs`, `nuxtjs-i18n`, `playwright-ct`, `sst`, `qwik`, `quasar`, `stencil`, `eleventy`, `cucumber`, `relay`, `orval`, `vite-plugin-pwa`, `xo`, `textlint`, `rstest`, `sveltejs-package`, `taskfile`, `mise`, `moonrepo`, `wireit`, `travis`, `buf`, `nano-staged`, `astro-db`, `astro-og-canvas`, `bumpp`, `changelogithub`, `create-typescript-app`, `eve`, `expressive-code`, `kysely-codegen`, `lockfile-lint`, `lost-pixel`, `node-modules-inspector`, `vercel-og`, `vite-pwa-assets-generator`, `astro-markdoc`, `borp`, `ladle`, `linthtml`, `lunaria`, `marko`, `mcp`, `mdxlint`, `npm-package-json-lint`, `osls`, `playwright-test`, `raycast`, `react-cosmos`, `tsrs`, `varlock` and `yorkie`.

## Acknowledgements

The behaviour of each plugin (enablers, config file names, entry patterns and which configuration values name
dependencies) follows [knip](https://github.com/webpro-nl/knip) (ISC licence), checked at commit
`da07b6e9887f9a1eb08545da29755b7f9d309bca`. The implementation is independent. `aws-cdk`, `browser-extension`, `azure-pipelines` and `sapper` have no knip counterpart at that commit.
