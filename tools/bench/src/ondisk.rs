//! A monorepo shaped like a large private one, to be written to disk and linted recursively.
//!
//! Unlike [`generate`](crate::generate), which makes one pnpm workspace, this mixes what `skif -r .` meets in a real
//! repository: several pnpm workspaces, standalone npm packages, projects nested below workspaces without being
//! members of them, and many directories that are not JavaScript at all (Python and Go trees). The cost of such a run
//! is directory walks and file system calls, not parsing. [`build`] is pure and deterministic.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

/// The shape of the generated repository.
#[derive(Debug, Clone)]
pub struct OnDiskOptions {
    /// pnpm workspaces under `ws/`.
    pub workspaces: usize,
    /// Member packages of each workspace.
    pub members: usize,
    /// Standalone npm packages under `svc/`.
    pub standalone: usize,
    /// Projects below each of the first workspaces that are not members of it.
    pub nested_per_workspace: usize,
    /// How many of the workspaces get nested projects.
    pub workspaces_with_nested: usize,
    /// Source directories per package, each holding `files_per_directory` modules.
    pub source_directories: usize,
    /// Modules per source directory.
    pub files_per_directory: usize,
    /// Python applications under `py/`, each with `python_packages` packages of `python_modules` directories.
    pub python_apps: usize,
    pub python_packages: usize,
    pub python_modules: usize,
    /// Go services under `go/`.
    pub go_services: usize,
}

impl Default for OnDiskOptions {
    /// About 10k directories, 70 project roots and 5k JavaScript files.
    fn default() -> Self {
        Self {
            workspaces: 10,
            members: 4,
            standalone: 40,
            nested_per_workspace: 2,
            workspaces_with_nested: 10,
            source_directories: 5,
            files_per_directory: 9,
            python_apps: 20,
            python_packages: 20,
            python_modules: 20,
            go_services: 30,
        }
    }
}

/// The generated files and what they add up to.
#[derive(Debug, Clone)]
pub struct OnDisk {
    /// Every file as `(path relative to the repository root, contents)`.
    pub files: Vec<(PathBuf, String)>,
    /// Project roots `skif -r` finds: workspaces, standalone packages and nested projects.
    pub projects: usize,
    /// JavaScript and TypeScript files.
    pub js_files: usize,
    /// Distinct directories holding the files.
    pub directories: usize,
}

struct Builder {
    options: OnDiskOptions,
    files: Vec<(PathBuf, String)>,
    js_files: usize,
}

impl Builder {
    fn file(&mut self, path: impl Into<PathBuf>, contents: impl Into<String>) {
        self.files.push((path.into(), contents.into()));
    }

    fn source(&mut self, path: String, contents: String) {
        self.js_files += 1;
        self.file(path, contents);
    }

    /// A package at `dir` (no trailing slash) with sources that import each other, `lodash` and `sibling`.
    fn package(&mut self, dir: &str, name: &str, sibling: Option<&str>) {
        let mut dependencies = String::from(r#""lodash": "^4.17.21""#);
        if let Some(sibling) = sibling {
            let _ = write!(dependencies, r#", "{sibling}": "workspace:*""#);
        }
        self.file(
            format!("{dir}/package.json"),
            format!(
                "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"main\": \"src/index.ts\",\n  \
                 \"dependencies\": {{ {dependencies} }}\n}}\n"
            ),
        );
        self.file(
            format!("{dir}/tsconfig.json"),
            "{ \"compilerOptions\": { \"strict\": true, \"module\": \"esnext\" } }\n",
        );
        self.source(
            format!("{dir}/src/util.ts"),
            "export function helper(value: number): number {\n  return value + 1;\n}\n".to_owned(),
        );
        let mut index = String::new();
        if let Some(sibling) = sibling {
            let _ = writeln!(index, "import {{ entry as upstream }} from '{sibling}';");
        }
        for directory in 0..self.options.source_directories {
            let _ = writeln!(index, "import * as d{directory} from './d{directory}/index';");
        }
        let _ = writeln!(index, "export const entry = [");
        if sibling.is_some() {
            let _ = writeln!(index, "  upstream,");
        }
        for directory in 0..self.options.source_directories {
            let _ = writeln!(index, "  d{directory},");
        }
        index.push_str("];\n");
        self.source(format!("{dir}/src/index.ts"), index);
        for directory in 0..self.options.source_directories {
            self.directory_sources(dir, directory);
        }
        // Installed dependencies are walked past, never analysed.
        self.file(
            format!("{dir}/node_modules/lodash/package.json"),
            "{ \"name\": \"lodash\", \"version\": \"4.17.21\", \"main\": \"index.js\" }\n",
        );
        self.file(format!("{dir}/node_modules/lodash/index.js"), "module.exports = {};\n");
    }

    fn directory_sources(&mut self, dir: &str, directory: usize) {
        let files = self.options.files_per_directory;
        let mut index = String::new();
        for file in 0..files {
            let _ = writeln!(index, "export * as f{file} from './f{file}';");
        }
        self.source(format!("{dir}/src/d{directory}/index.ts"), index);
        for file in 0..files {
            self.source(
                format!("{dir}/src/d{directory}/f{file}.ts"),
                format!(
                    "import {{ debounce }} from 'lodash';\nimport {{ helper }} from '../util';\n\n\
                     export const value{file} = helper({file});\nexport const wrapped{file} = debounce(() => value{file}, 10);\n"
                ),
            );
        }
    }

    fn workspace(&mut self, index: usize) {
        let dir = format!("ws/ws{index}");
        self.file(
            format!("{dir}/package.json"),
            format!(
                "{{\n  \"name\": \"ws{index}\",\n  \"private\": true,\n  \"packageManager\": \"pnpm@12.15.0\"\n}}\n"
            ),
        );
        self.file(format!("{dir}/pnpm-workspace.yaml"), "packages:\n  - packages/*\n");
        self.file(format!("{dir}/.gitignore"), "dist\ncoverage\n");
        for member in 0..self.options.members {
            let sibling = (member > 0).then(|| format!("@ws{index}/pkg{}", member - 1));
            self.package(
                &format!("{dir}/packages/pkg{member}"),
                &format!("@ws{index}/pkg{member}"),
                sibling.as_deref(),
            );
        }
        if index < self.options.workspaces_with_nested {
            for nested in 0..self.options.nested_per_workspace {
                self.package(&format!("{dir}/tools/nested{nested}"), &format!("ws{index}-tool{nested}"), None);
            }
        }
    }

    fn other_trees(&mut self) {
        let OnDiskOptions {
            python_apps,
            python_packages,
            python_modules,
            go_services,
            ..
        } = self.options;
        for app in 0..python_apps {
            self.file(format!("py/app{app}/pyproject.toml"), "[project]\nname = \"app\"\n");
            for package in 0..python_packages {
                for module in 0..python_modules {
                    self.file(
                        format!("py/app{app}/pkg{package}/mod{module}/impl.py"),
                        "def run() -> int:\n    return 1\n",
                    );
                }
            }
        }
        for service in 0..go_services {
            self.file(format!("go/svc{service}/go.mod"), "module example.com/svc\n\ngo 1.22\n");
            self.file(format!("go/svc{service}/cmd/main.go"), "package main\n\nfunc main() {}\n");
            self.file(format!("go/svc{service}/internal/store/store.go"), "package store\n");
        }
    }
}

/// Builds the repository described by `options`.
#[must_use]
pub fn build(options: &OnDiskOptions) -> OnDisk {
    let mut builder = Builder {
        options: options.clone(),
        files: Vec::new(),
        js_files: 0,
    };
    builder.file("package.json", "{ \"name\": \"repo\", \"private\": true }\n");
    for index in 0..options.workspaces {
        builder.workspace(index);
    }
    for index in 0..options.standalone {
        builder.package(&format!("svc/s{index}"), &format!("svc-{index}"), None);
    }
    builder.other_trees();
    let directories: BTreeSet<PathBuf> = builder
        .files
        .iter()
        .flat_map(|(path, _)| path.ancestors().skip(1))
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(PathBuf::from)
        .collect();
    let nested = options.workspaces.min(options.workspaces_with_nested) * options.nested_per_workspace;
    OnDisk {
        projects: options.workspaces + options.standalone + nested,
        js_files: builder.js_files,
        directories: directories.len(),
        files: builder.files,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_shape_matches_the_large_monorepo() {
        let repo = build(&OnDiskOptions::default());
        assert_eq!(repo.projects, 70);
        assert!((4_500..6_500).contains(&repo.js_files), "{}", repo.js_files);
        assert!(repo.directories >= 8_000, "{}", repo.directories);
        assert!(repo.files.iter().all(|(path, _)| path.is_relative()));
    }

    #[test]
    fn output_is_deterministic_and_paths_are_unique() {
        let first = build(&OnDiskOptions::default());
        let second = build(&OnDiskOptions::default());
        assert_eq!(first.files, second.files);
        let unique: BTreeSet<&PathBuf> = first.files.iter().map(|(path, _)| path).collect();
        assert_eq!(unique.len(), first.files.len());
    }

    #[test]
    fn counts_follow_the_options() {
        let repo = build(&OnDiskOptions {
            workspaces: 1,
            members: 2,
            standalone: 1,
            nested_per_workspace: 1,
            workspaces_with_nested: 1,
            source_directories: 1,
            files_per_directory: 2,
            python_apps: 0,
            python_packages: 0,
            python_modules: 0,
            go_services: 0,
        });
        // workspace (2 members + 1 nested) and one standalone = 4 packages with util, index, d0/index and two modules.
        assert_eq!(repo.projects, 3);
        assert_eq!(repo.js_files, 4 * 5);
    }
}
