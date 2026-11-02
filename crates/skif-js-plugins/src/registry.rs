//! The plugin table of ADR 0011.

use crate::plugin::Plugin;
use crate::plugins::{
    babel, changesets, commitlint, eslint, github_actions, husky, jest, lint_staged, next, playwright, postcss,
    prettier, rollup, rspack, storybook, tailwind, tsup, turbo, typescript, vite, vitest, webpack,
};

static PLUGINS: [&dyn Plugin; 22] = [
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
];

/// Every plugin, in the order ADR 0011 lists them.
#[must_use]
pub fn plugins() -> &'static [&'static dyn Plugin] {
    &PLUGINS
}

/// The plugin with this identifier.
#[must_use]
pub fn plugin(id: &str) -> Option<&'static dyn Plugin> {
    PLUGINS.iter().copied().find(|p| p.spec().id == id)
}
