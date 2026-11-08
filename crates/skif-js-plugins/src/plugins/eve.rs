//! eve: the agent, subagent and extension files of an eve agent are production entries; its evals are entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `eve` plugin.
pub struct Eve;

// knip writes the subagent roots as `+(subagents/*/|extensions/*/subagents/*/)`; one level is matched.
static SPEC: PluginSpec = PluginSpec {
    id: "eve",
    enablers: &[Enabler::Dependency("eve")],
    entry: &[
        "{evals,agents/*/evals}/evals.config.ts",
        "{evals,agents/*/evals}/**/*.eval.ts",
    ],
    production: &[
        "{,agent/,agents/*/,agents/*/agent/}{agent,instructions,instrumentation,memory,sandbox}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{instructions,instrumentation,memory}/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}sandbox/sandbox.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}sandbox/workspace/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{channels,connections,hooks,skills,tools,schedules}/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}subagents/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}subagents/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}{agent,instructions,memory,sandbox}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}{instructions,memory}/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}sandbox/sandbox.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}sandbox/workspace/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}{connections,hooks,skills,tools}/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}extensions/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}extensions/*/{extension,instructions}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}extensions/*/instructions/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}extensions/*/{channels,connections,hooks,skills,tools,schedules}/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}extensions/*/subagents/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}extensions/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}extensions/*/{extension,instructions}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}extensions/*/instructions/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}extensions/*/{channels,connections,hooks,skills,tools,schedules}/**/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "{,agent/,agents/*/,agents/*/agent/}{subagents/*/,extensions/*/subagents/*/}extensions/*/subagents/*.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for Eve {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
