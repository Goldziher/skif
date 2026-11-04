//! TypeScript sources for generated workspaces, so the JS analysis has a module graph to build.

use std::fmt::Write as _;
use std::path::PathBuf;

use skif_bench::generate::Generated;

/// Adds `files_per_package` modules under `src/` to every generated package: `index.ts` (the `main` of the generated
/// manifests) plus a chain `m0.ts ... m{n-2}.ts` in which each module imports its predecessor, so the graph has one
/// long path per package and every file is reachable from the entry.
pub fn add_sources(generated: &mut Generated, files_per_package: usize) {
    if files_per_package == 0 {
        return;
    }
    let directories: Vec<PathBuf> = generated
        .files
        .iter()
        .filter(|(path, _)| path.file_name().is_some_and(|name| name == "package.json"))
        .filter_map(|(path, _)| path.parent().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from))
        .collect();
    let modules = files_per_package - 1;
    for directory in directories {
        let src = directory.join("src");
        for index in 0..modules {
            let mut text = String::new();
            if index > 0 {
                let _ = writeln!(text, "import {{ value{} }} from \"./m{}\";", index - 1, index - 1);
                let _ = writeln!(text, "export const value{index} = value{} + 1;", index - 1);
            } else {
                text.push_str("export const value0 = 0;\n");
            }
            generated.files.push((src.join(format!("m{index}.ts")), text));
        }
        let entry = if modules == 0 {
            "export const total = 0;\n".to_owned()
        } else {
            format!(
                "import {{ value{last} }} from \"./m{last}\";\nexport const total: number = value{last};\n",
                last = modules - 1
            )
        };
        generated.files.push((src.join("index.ts"), entry));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use skif_bench::generate::{self, Options, PackageManager};

    fn generated(packages: usize) -> Generated {
        generate::build(&Options {
            packages,
            seed: 1,
            package_manager: PackageManager::Pnpm,
            catalog: false,
            mismatch_rate: 0.0,
        })
        .unwrap()
    }

    fn sources(generated: &Generated) -> Vec<&(PathBuf, String)> {
        generated
            .files
            .iter()
            .filter(|(path, _)| path.extension().is_some_and(|e| e == "ts"))
            .collect()
    }

    #[test]
    fn every_package_gets_the_requested_number_of_files() {
        let mut workspace = generated(7);
        add_sources(&mut workspace, 12);
        assert_eq!(sources(&workspace).len(), 7 * 12);
        assert!(
            sources(&workspace)
                .iter()
                .any(|(path, _)| path.ends_with("src/index.ts"))
        );
    }

    #[test]
    fn modules_chain_to_their_predecessor() {
        let mut workspace = generated(1);
        add_sources(&mut workspace, 4);
        let text = |name: &str| {
            sources(&workspace)
                .into_iter()
                .find(|(path, _)| path.ends_with(name))
                .map(|(_, text)| text.clone())
                .unwrap()
        };
        assert!(text("src/m1.ts").contains("from \"./m0\""));
        assert!(text("src/index.ts").contains("from \"./m2\""));
    }

    #[test]
    fn zero_files_changes_nothing() {
        let mut workspace = generated(3);
        let before = workspace.files.len();
        add_sources(&mut workspace, 0);
        assert_eq!(workspace.files.len(), before);
    }

    #[test]
    fn is_deterministic() {
        let (mut a, mut b) = (generated(5), generated(5));
        add_sources(&mut a, 6);
        add_sources(&mut b, 6);
        assert_eq!(a.files, b.files);
    }
}
