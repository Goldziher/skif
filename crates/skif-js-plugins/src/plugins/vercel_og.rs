//! Vercel OG: the Open Graph image routes of a Next.js app are production entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `vercel-og` plugin.
pub struct VercelOg;

static SPEC: PluginSpec = PluginSpec {
    id: "vercel-og",
    enablers: &[Enabler::Dependency("next"), Enabler::Dependency("@vercel/og")],
    production: &["{src/,}pages/api/og.{jsx,tsx}", "{src/,}app/api/og/route.{jsx,tsx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for VercelOg {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
