//! Bazel as the JavaScript analysis sees it: the repository root, the output trees below it and the outputs a `BUILD`
//! file declares. Starlark is scanned as text and never evaluated: string literals, and the keyword arguments of the
//! calls at the top level of a file.

use std::path::{Component, Path, PathBuf};

use skif_core::FileSystem;

/// The files that mark the root of a Bazel repository.
pub const ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// The files that declare a Bazel package, in the order Bazel prefers them.
pub const BUILD_FILES: [&str; 2] = ["BUILD.bazel", "BUILD"];

/// The Bazel repository root that holds `dir`: the nearest directory at or above it with one of [`ROOT_FILES`],
/// looked for up to the git repository root (a directory holding `.git`), so that a stray file higher up is not
/// taken for it. `None` outside a Bazel repository, and when `dir` lies in an output tree of the repository found (a
/// test's runfiles below `bazel-out`), where every file would be output.
#[must_use]
pub fn repository_root(fs: &dyn FileSystem, dir: &Path) -> Option<PathBuf> {
    for candidate in dir.ancestors() {
        if ROOT_FILES.iter().any(|name| fs.is_file(&candidate.join(name))) {
            return (!is_output_path(candidate, dir)).then(|| candidate.to_path_buf());
        }
        let git = candidate.join(".git");
        if fs.is_dir(&git) || fs.is_file(&git) {
            return None;
        }
    }
    None
}

/// Whether `path` lies in an output tree of the Bazel repository at `root`: below one of the `bazel-*` convenience
/// links at its top (`bazel-bin`, `bazel-out`, `bazel-testlogs`, `bazel-<workspace>`), whose contents depend on what
/// was built last.
#[must_use]
pub fn is_output_path(root: &Path, path: &Path) -> bool {
    let first = path.strip_prefix(root).ok().and_then(|rest| rest.components().next());
    matches!(first, Some(Component::Normal(name)) if name.to_str().is_some_and(|name| name.starts_with("bazel-")))
}

/// The `BUILD.bazel` or `BUILD` file of the directory `dir`, when it has one.
#[must_use]
pub fn build_file(fs: &dyn FileSystem, dir: &Path) -> Option<PathBuf> {
    BUILD_FILES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| fs.is_file(path))
}

/// The files and directories the targets of a `BUILD` file declare as outputs, relative to its directory: the
/// `outs`, `out` and `stdout` of a rule (`js_run_binary`, `genrule`, `copy_file`) and its `out_dirs` and `out_dir`.
/// Only literal paths count; a computed one (`glob`, `select`, a variable) declares nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredOutputs {
    files: Vec<String>,
    dirs: Vec<String>,
}

impl DeclaredOutputs {
    /// The outputs declared in the `BUILD` file text `source`.
    #[must_use]
    pub fn read(source: &str) -> Self {
        let mut outputs = Self::default();
        for call in calls(source) {
            for (attribute, values) in &call.attributes {
                let list = match attribute.as_str() {
                    "outs" | "out" | "stdout" => &mut outputs.files,
                    "out_dirs" | "out_dir" => &mut outputs.dirs,
                    _ => continue,
                };
                list.extend(
                    values
                        .iter()
                        .filter_map(|value| package_path(value))
                        .map(|path| path.trim_end_matches('/').to_owned()),
                );
            }
        }
        outputs
    }

    /// Whether `path`, `/`-separated and relative to the `BUILD` file's directory, is a declared output: a declared
    /// file, also with another extension (an import of `gen/schema.js` for an output `gen/schema.ts`, or with no
    /// extension at all), or a file below a declared directory.
    #[must_use]
    pub fn declares(&self, path: &str) -> bool {
        let wanted = stem(path);
        self.files
            .iter()
            .any(|file| file == path || stem(file) == path || stem(file) == wanted)
            || self.dirs.iter().any(|dir| {
                path.strip_prefix(dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
    }

    /// Whether the file declares no output.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty()
    }
}

/// `path` without the extension of its last segment; a declaration file loses `.d.ts` (`.d.mts`, `.d.cts`) whole.
fn stem(path: &str) -> &str {
    for declaration in [".d.ts", ".d.mts", ".d.cts"] {
        if let Some(stem) = path.strip_suffix(declaration) {
            return stem;
        }
    }
    let name_start = path.rfind('/').map_or(0, |at| at + 1);
    match path[name_start..].rfind('.') {
        Some(dot) if dot > 0 => &path[..name_start + dot],
        _ => path,
    }
}

/// The path a literal names in its own package: a relative file (`src/a.ts`) or a target of the package written as a
/// label (`:src/a.ts`), without a leading `./`. A label of another package or repository, an absolute path, one that
/// climbs out with `..` and a template (`{name}.js`, `$(location)`) name none.
pub(crate) fn package_path(literal: &str) -> Option<&str> {
    let path = literal.strip_prefix(':').unwrap_or(literal);
    let path = path.strip_prefix("./").unwrap_or(path);
    let plain = !path.is_empty()
        && !path.starts_with(['/', '@'])
        && !path.contains([':', '$', '{', '}'])
        && !path.split('/').any(|segment| segment == "..");
    plain.then_some(path)
}

/// A call at the top level of a Starlark file, such as a rule or macro of a `BUILD` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Call {
    /// The name called (`js_binary`).
    pub(crate) rule: String,
    /// Each keyword argument with the string literals of its value, in order. A literal inside a nested call
    /// (`glob([...])`, `select({...})`) is left out: it is no plain value of the argument.
    pub(crate) attributes: Vec<(String, Vec<String>)>,
}

impl Call {
    /// The literals of the keyword argument `name`, empty when the call has none.
    pub(crate) fn values(&self, name: &str) -> impl Iterator<Item = &str> {
        self.attributes
            .iter()
            .filter(move |(attribute, _)| attribute == name)
            .flat_map(|(_, values)| values.iter().map(String::as_str))
    }
}

/// The contents of every string literal of Starlark `source`, without those of `#` comments, in order.
pub(crate) fn string_literals(source: &str) -> Vec<String> {
    tokens(source)
        .into_iter()
        .filter_map(|token| match token {
            Token::Text(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// The calls at the top level of Starlark `source`; a method call (`"x".format(...)`) is none.
pub(crate) fn calls(source: &str) -> Vec<Call> {
    let tokens = tokens(source);
    let mut found = Vec::new();
    let mut depth = 0_usize;
    let mut at = 0;
    while at < tokens.len() {
        match &tokens[at] {
            Token::Name(name)
                if depth == 0
                    && matches!(tokens.get(at + 1), Some(Token::Open('(')))
                    && !matches!(
                        at.checked_sub(1).and_then(|before| tokens.get(before)),
                        Some(Token::Dot)
                    ) =>
            {
                let (call, next) = read_call(name, &tokens, at + 2);
                found.push(call);
                at = next;
                continue;
            }
            Token::Open(_) => depth += 1,
            Token::Close => depth = depth.saturating_sub(1),
            _ => {}
        }
        at += 1;
    }
    found
}

/// The call of `rule` whose arguments start at token `start`, just after its `(`, and the position after its `)`.
fn read_call(rule: &str, tokens: &[Token], start: usize) -> (Call, usize) {
    let mut call = Call {
        rule: rule.to_owned(),
        attributes: Vec::new(),
    };
    // For each open bracket, whether it opens the arguments of a call; the first is the call's own.
    let mut open: Vec<bool> = vec![true];
    let mut attribute: Option<usize> = None;
    let mut at = start;
    while at < tokens.len() {
        match &tokens[at] {
            Token::Open(bracket) => {
                let after_name = matches!(
                    at.checked_sub(1).and_then(|before| tokens.get(before)),
                    Some(Token::Name(_))
                );
                open.push(*bracket == '(' && after_name);
            }
            Token::Close => {
                open.pop();
                if open.is_empty() {
                    return (call, at + 1);
                }
            }
            Token::Comma if open.len() == 1 => attribute = None,
            Token::Name(name) if open.len() == 1 && matches!(tokens.get(at + 1), Some(Token::Assign)) => {
                call.attributes.push((name.clone(), Vec::new()));
                attribute = Some(call.attributes.len() - 1);
                at += 2;
                continue;
            }
            Token::Text(text) => {
                if let Some(index) = attribute
                    && !open[1..].contains(&true)
                {
                    call.attributes[index].1.push(text.clone());
                }
            }
            _ => {}
        }
        at += 1;
    }
    (call, at)
}

/// A token of Starlark source, as far as the scan needs to tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// An identifier or keyword.
    Name(String),
    /// The contents of a string literal, its escapes resolved.
    Text(String),
    /// `(`, `[` or `{`.
    Open(char),
    /// `)`, `]` or `}`.
    Close,
    Comma,
    /// `=` on its own, not part of `==`, `!=`, `<=`, `>=` or an augmented assignment.
    Assign,
    Dot,
    /// Anything else: numbers and operators.
    Other,
}

/// The tokens of Starlark `source`, without whitespace and `#` comments.
fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        match c {
            '#' => {
                while chars.get(at).is_some_and(|&c| c != '\n') {
                    at += 1;
                }
            }
            '"' | '\'' => {
                let (text, next) = string(&chars, at, false);
                tokens.push(Token::Text(text));
                at = next;
            }
            '(' | '[' | '{' => {
                tokens.push(Token::Open(c));
                at += 1;
            }
            ')' | ']' | '}' => {
                tokens.push(Token::Close);
                at += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                at += 1;
            }
            '.' => {
                tokens.push(Token::Dot);
                at += 1;
            }
            '=' => {
                let compound = chars.get(at + 1) == Some(&'=')
                    || at
                        .checked_sub(1)
                        .and_then(|before| chars.get(before))
                        .is_some_and(|before| "!<>=+-*/%&|^".contains(*before));
                if chars.get(at + 1) == Some(&'=') {
                    at += 1;
                }
                tokens.push(if compound { Token::Other } else { Token::Assign });
                at += 1;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = at;
                while chars.get(at).is_some_and(|&c| c.is_alphanumeric() || c == '_') {
                    at += 1;
                }
                let word: String = chars[start..at].iter().collect();
                let prefix = word.len() <= 2 && word.chars().all(|c| matches!(c, 'r' | 'R' | 'b' | 'B'));
                if prefix && matches!(chars.get(at), Some('"' | '\'')) {
                    let (text, next) = string(&chars, at, word.contains(['r', 'R']));
                    tokens.push(Token::Text(text));
                    at = next;
                } else if c.is_ascii_digit() {
                    tokens.push(Token::Other);
                } else {
                    tokens.push(Token::Name(word));
                }
            }
            c if c.is_whitespace() || c == '\\' => at += 1,
            _ => {
                tokens.push(Token::Other);
                at += 1;
            }
        }
    }
    tokens
}

/// The string literal whose opening quote is at `start`, and the position after it. A single-quoted literal ends at
/// the end of its line when it is not closed.
fn string(chars: &[char], start: usize, raw: bool) -> (String, usize) {
    let quote = chars[start];
    let triple = chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote);
    let mut at = start + if triple { 3 } else { 1 };
    let mut text = String::new();
    while let Some(&c) = chars.get(at) {
        if c == quote && (!triple || (chars.get(at + 1) == Some(&quote) && chars.get(at + 2) == Some(&quote))) {
            return (text, at + if triple { 3 } else { 1 });
        }
        if c == '\n' && !triple {
            return (text, at);
        }
        if c == '\\'
            && let Some(&next) = chars.get(at + 1)
        {
            match next {
                '\\' | '"' | '\'' if !raw => text.push(next),
                'n' if !raw => text.push('\n'),
                't' if !raw => text.push('\t'),
                '\n' if !raw => {}
                _ => {
                    text.push(c);
                    text.push(next);
                }
            }
            at += 2;
            continue;
        }
        text.push(c);
        at += 1;
    }
    (text, at)
}

#[cfg(test)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn literals_skip_comments_and_read_every_quote_style() {
        let source = "# \"commented\"\nload('@npm//:defs.bzl', \"x\") # 'trailing'\ny = r\"a\\b\" + \"\"\"multi\nline\"\"\" + 'it\\'s'\n";
        assert_eq!(
            string_literals(source),
            ["@npm//:defs.bzl", "x", "a\\b", "multi\nline", "it's"]
        );
    }

    #[test]
    fn calls_keep_the_literal_values_of_their_keyword_arguments() {
        let source = r#"
load("@aspect_rules_js//js:defs.bzl", "js_binary")

js_binary(
    name = "codegen",
    entry_point = "tools/codegen.mjs",
    data = [":node_modules/esbuild", "templates/a.hbs"] + glob(["extra/*.json"]),
    env = select({"//conditions:default": {"MODE": "dev"}}),
    visibility = ["//visibility:public"] if True else [],
)

files = ["a.ts"]
NAME = "x".format(1)
"#;
        let found = calls(source);
        let rules: Vec<&str> = found.iter().map(|call| call.rule.as_str()).collect();
        assert_eq!(rules, ["load", "js_binary"]);
        let binary = &found[1];
        assert_eq!(binary.values("entry_point").collect::<Vec<_>>(), ["tools/codegen.mjs"]);
        assert_eq!(
            binary.values("data").collect::<Vec<_>>(),
            [":node_modules/esbuild", "templates/a.hbs"]
        );
        assert_eq!(binary.values("env").count(), 0);
        assert_eq!(binary.values("visibility").collect::<Vec<_>>(), ["//visibility:public"]);
        assert!(found[0].attributes.is_empty());
    }

    #[test]
    fn comparisons_and_augmented_assignments_are_no_keyword_arguments() {
        let found = calls("rule(name = \"a\", srcs = [\"x.ts\"] if a == \"b\" else [], deps += [\"c\"])");
        let names: Vec<&str> = found[0].attributes.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["name", "srcs"]);
        assert_eq!(found[0].values("srcs").collect::<Vec<_>>(), ["x.ts", "b"]);
    }

    #[test]
    fn declared_outputs_match_files_by_stem_and_files_below_directories() {
        let outputs = DeclaredOutputs::read(
            r#"
js_run_binary(name = "gen", outs = ["src/gen/schema.ts", ":src/gen/types.d.ts"], out_dirs = ["src/api/"])
genrule(name = "g", outs = glob(["no/*.js"]), cmd = "x")
copy_file(name = "c", src = "a.js", out = "copied.js")
"#,
        );
        for (path, declared) in [
            ("src/gen/schema.ts", true),
            ("src/gen/schema.js", true),
            ("src/gen/schema", true),
            ("src/gen/types.js", true),
            ("src/api/client/index.js", true),
            ("copied.js", true),
            ("src/api", false),
            ("src/gen/other.js", false),
            ("no/x.js", false),
            ("a.js", false),
        ] {
            assert_eq!(outputs.declares(path), declared, "{path}");
        }
        assert!(DeclaredOutputs::read("js_library(name = \"x\", srcs = [\"a.js\"])").is_empty());
    }

    #[test]
    fn package_paths_are_files_of_the_package() {
        for (literal, path) in [
            ("src/a.ts", Some("src/a.ts")),
            (":src/a.ts", Some("src/a.ts")),
            ("./a.ts", Some("a.ts")),
            ("//pkg:a.ts", None),
            ("@npm//:a", None),
            ("../a.ts", None),
            ("/abs/a.ts", None),
            ("{name}.js", None),
            ("$(location :x)", None),
            ("", None),
        ] {
            assert_eq!(package_path(literal), path, "{literal}");
        }
    }

    #[test]
    fn the_repository_root_is_the_nearest_directory_with_a_root_file_inside_git() {
        let fs = MemoryFs::new()
            .with_file("/ws/MODULE.bazel", "module(name = \"mono\")\n")
            .with_file("/ws/packages/app/package.json", "{}");
        assert_eq!(
            repository_root(&fs, Path::new("/ws/packages/app")),
            Some(PathBuf::from("/ws"))
        );
        let nested = fs.clone().with_file("/ws/e2e/WORKSPACE", "");
        assert_eq!(
            repository_root(&nested, Path::new("/ws/e2e")),
            Some(PathBuf::from("/ws/e2e"))
        );
        let fenced = MemoryFs::new()
            .with_file("/MODULE.bazel", "")
            .with_file("/ws/.git/HEAD", "ref: refs/heads/main\n");
        assert_eq!(repository_root(&fenced, Path::new("/ws/app")), None);
        let runfiles = "/ws/bazel-out/k8-fastbuild/bin/lint.runfiles/_main";
        assert_eq!(repository_root(&fs, Path::new(runfiles)), None);
    }

    #[test]
    fn output_paths_lie_below_a_bazel_link_at_the_root() {
        let root = Path::new("/ws");
        for (path, output) in [
            ("/ws/bazel-bin/packages/app/gen.js", true),
            ("/ws/bazel-out/k8-fastbuild/bin/x.js", true),
            ("/ws/bazel-mono/packages/app/x.js", true),
            ("/ws/packages/app/bazel-bin/x.js", false),
            ("/ws/packages/bazel-tools/x.js", false),
            ("/other/bazel-bin/x.js", false),
        ] {
            assert_eq!(is_output_path(root, Path::new(path)), output, "{path}");
        }
    }
}
