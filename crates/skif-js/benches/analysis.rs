//! The JS provider (module graph plus every JS rule) over a generated TypeScript workspace held in a [`MemoryFs`].
//!
//! Each package has `files` modules chained by imports from an `index.ts` entry, an external dependency, an unused
//! dependency and one unused export per module, so every rule has something to find.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use skif_config::Config;
use skif_core::MemoryFs;
use skif_js::JsProvider;
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/repo";
const FILES_PER_PACKAGE: usize = 20;

fn module(index: usize, files: usize) -> String {
    let mut source = String::new();
    if index + 1 < files {
        let _ = writeln!(source, "import {{ step{} as next }} from \"./m{}\";", index + 1, index + 1);
    }
    source.push_str("import debounce from \"lodash/debounce\";\n\n");
    let _ = writeln!(source, "export interface Options{index} {{\n  id: number;\n  name: string;\n}}\n");
    let _ = writeln!(source, "export const unused{index} = {index};\n");
    let _ = writeln!(source, "export function step{index}(value: number): number {{");
    if index + 1 < files {
        let _ = writeln!(source, "  return debounce(next, {index})(value + 1) ?? value;");
    } else {
        source.push_str("  return value;\n");
    }
    source.push_str("}\n");
    source
}

fn memory_fs(packages: usize) -> MemoryFs {
    let workspaces: Vec<String> = (0..packages).map(|i| format!("\"packages/p{i}\"")).collect();
    let mut fs = MemoryFs::new().with_file(
        format!("{ROOT}/package.json"),
        format!(
            "{{\"name\": \"root\", \"private\": true, \"workspaces\": [{}]}}",
            workspaces.join(", ")
        ),
    );
    for package in 0..packages {
        let dir = format!("{ROOT}/packages/p{package}");
        fs.insert(
            format!("{dir}/package.json"),
            format!(
                "{{\"name\": \"@bench/p{package}\", \"private\": true, \"main\": \"src/index.ts\", \
                 \"dependencies\": {{\"lodash\": \"^4.17.21\", \"left-over\": \"^1.0.0\"}}}}"
            ),
        );
        fs.insert(
            format!("{dir}/src/index.ts"),
            "import { step0 } from \"./m0\";\n\nexport const run = (): number => step0(1);\n",
        );
        for index in 0..FILES_PER_PACKAGE {
            fs.insert(format!("{dir}/src/m{index}.ts"), module(index, FILES_PER_PACKAGE));
        }
    }
    fs
}

fn bench_analysis(c: &mut Criterion) {
    let config = Config::default();
    let mut group = c.benchmark_group("js_analysis");
    for packages in [10, 100] {
        let fs = memory_fs(packages);
        let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &fs).expect("workspace discovers");
        let registry =
            Registry::new(vec![Box::new(JsProvider::new(Arc::new(fs)))]).expect("the JS provider registers");
        group.throughput(Throughput::Elements((packages * (FILES_PER_PACKAGE + 1)) as u64));
        group.sample_size(10);
        group.bench_function(BenchmarkId::new("files", packages * (FILES_PER_PACKAGE + 1)), |b| {
            b.iter(|| {
                run_unsorted(&registry, black_box(&workspace), &config, &RunOptions::default())
                    .expect("the JS run succeeds")
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_analysis);
criterion_main!(benches);
