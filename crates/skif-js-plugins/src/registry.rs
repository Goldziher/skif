//! The plugin table of ADR 0011.

use crate::plugin::Plugin;
use crate::plugins::{
    angular, astro, aws_cdk, babel, browser_extension, changesets, commitlint, cspell, cypress, docusaurus, drizzle,
    esbuild, eslint, expo, expo_router, github_actions, graphql_codegen, husky, jest, karma, lefthook, lint_staged,
    markdownlint, mastra, metro, mocha, next, node, nodemon, nuxt, nx, oxlint, playwright, pm2, pnpm, postcss, prettier,
    prisma, react_native, release_it, rollup, rspack, semantic_release, simple_git_hooks, storybook, stylelint, svelte,
    sveltekit, syncpack, tailwind, tanstack_router, ts_node, tsdown, tsup, tsx, turbo, typescript, vite, vitepress,
    vitest, webpack, wrangler, yarn,
};

static PLUGINS: [&dyn Plugin; 63] = [
    &typescript::Typescript,
    &eslint::Eslint,
    &prettier::Prettier,
    &vite::Vite,
    &vitest::Vitest,
    &jest::Jest,
    &next::Next,
    &storybook::Storybook,
    &playwright::Playwright,
    &tailwind::Tailwind,
    &postcss::Postcss,
    &babel::Babel,
    &webpack::Webpack,
    &rspack::Rspack,
    &rollup::Rollup,
    &tsup::Tsup,
    &turbo::Turbo,
    &husky::Husky,
    &lint_staged::LintStaged,
    &commitlint::Commitlint,
    &changesets::Changesets,
    &github_actions::GithubActions,
    &mastra::Mastra,
    &aws_cdk::AwsCdk,
    &browser_extension::BrowserExtension,
    &tsdown::Tsdown,
    &oxlint::Oxlint,
    &angular::Angular,
    &karma::Karma,
    &esbuild::Esbuild,
    &svelte::Svelte,
    &nuxt::Nuxt,
    &astro::Astro,
    &drizzle::Drizzle,
    &ts_node::TsNode,
    &tsx::Tsx,
    &nx::Nx,
    &cypress::Cypress,
    &mocha::Mocha,
    &sveltekit::Sveltekit,
    &prisma::Prisma,
    &graphql_codegen::GraphqlCodegen,
    &docusaurus::Docusaurus,
    &vitepress::Vitepress,
    &wrangler::Wrangler,
    &expo::Expo,
    &react_native::ReactNative,
    &metro::Metro,
    &release_it::ReleaseIt,
    &semantic_release::SemanticRelease,
    &lefthook::Lefthook,
    &simple_git_hooks::SimpleGitHooks,
    &stylelint::Stylelint,
    &markdownlint::Markdownlint,
    &cspell::Cspell,
    &nodemon::Nodemon,
    &pm2::Pm2,
    &pnpm::Pnpm,
    &yarn::Yarn,
    &syncpack::Syncpack,
    &node::Node,
    &expo_router::ExpoRouter,
    &tanstack_router::TanstackRouter,
];

/// Every plugin, in the order of the table (ADR 0011, ADR 0022, ADR 0025, ADR 0029).
#[must_use]
pub fn plugins() -> &'static [&'static dyn Plugin] {
    &PLUGINS
}

/// The plugin with this identifier.
#[must_use]
pub fn plugin(id: &str) -> Option<&'static dyn Plugin> {
    PLUGINS.iter().copied().find(|p| p.spec().id == id)
}
