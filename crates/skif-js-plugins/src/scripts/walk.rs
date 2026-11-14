//! Interprets tokenized commands and collects what they reference.

use skif_core::HashSet;
use std::ops::Range;

use super::args::{self, Parsed, Val};
use super::tables::{self, Files, Runner, Wrapper};
use super::tokenize::{EmptyCommand, Segment, Word, is_assignment_text, tokenize};
use crate::support::{below_node_modules, join_rel};

use super::{
    BinaryUse, BinaryVia, DepRef, FileRef, FileTree, ScriptContext, ScriptInfo, is_ignored_binary, is_shell_builtin,
};

const MAX_DEPTH: usize = 32;

/// The flags of `bun` and `bun run` that run the command in other workspace packages.
const BUN_WORKSPACE_FLAGS: &[&str] = &["filter", "F", "workspaces"];

/// The flags of `bun` and `bun run` that name the workspace packages the command runs in.
const BUN_FILTER_FLAGS: &[&str] = &["filter", "F"];

/// Maps byte ranges of the text being walked back to the originally parsed script.
#[derive(Clone, Debug)]
enum Map {
    /// The text is a verbatim slice of the script starting at this offset.
    Offset(usize),
    /// The text is derived (unescaped, or another script); every range maps to this one.
    Fixed(Range<usize>),
}

impl Map {
    fn apply(&self, range: &Range<usize>) -> Range<usize> {
        match self {
            Self::Offset(by) => range.start + by..range.end + by,
            Self::Fixed(fixed) => fixed.clone(),
        }
    }

    fn shifted(&self, by: usize) -> Self {
        match self {
            Self::Offset(offset) => Self::Offset(offset + by),
            Self::Fixed(fixed) => Self::Fixed(fixed.clone()),
        }
    }
}

struct Frame<'a> {
    text: &'a str,
    map: &'a Map,
}

impl Frame<'_> {
    fn span(&self, range: &Range<usize>) -> Range<usize> {
        self.map.apply(range)
    }

    /// The map for the text of `word`, exact when the word is a verbatim or simply quoted slice.
    fn child(&self, word: &Word) -> Map {
        let raw = self.text.get(word.span.clone()).unwrap_or("");
        if raw == word.text {
            return self.map.shifted(word.span.start);
        }
        let quoted = raw.len() >= 2
            && (raw.starts_with('"') || raw.starts_with('\''))
            && raw.chars().next() == raw.chars().last()
            && raw.get(1..raw.len() - 1) == Some(word.text.as_str());
        if quoted {
            self.map.shifted(word.span.start + 1)
        } else {
            Map::Fixed(self.span(&word.span))
        }
    }
}

#[derive(Clone, Copy, Default)]
struct St {
    /// The command targets another workspace.
    filtered: bool,
    via: BinaryVia,
}

enum Target<'a> {
    Skip,
    File(&'a str),
    Bin(&'a str),
}

fn valid_binary(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if !matches!(first, '-' | '$' | '.' | '=') => {}
        _ => return false,
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'))
}

/// Whether `dir`, a `cd` target or a directory flag's value, is a static relative path the parser can follow.
fn is_static_relative(dir: &str, dynamic: bool) -> bool {
    !dynamic && !dir.is_empty() && dir != "-" && !dir.starts_with(['/', '~']) && !dir.contains(['$', '`', '\\'])
}

fn classify(text: &str) -> Target<'_> {
    let bin_dir = text
        .rfind("/.bin/")
        .map(|i| &text[i + "/.bin/".len()..])
        .or_else(|| text.strip_prefix(".bin/"));
    if let Some(name) = bin_dir {
        return if valid_binary(name) {
            Target::Bin(name)
        } else {
            Target::Skip
        };
    }
    if text.starts_with('/') || text.starts_with('~') || text == "." || text == ".." {
        return Target::Skip;
    }
    if text.contains('/') {
        return Target::File(text);
    }
    if valid_binary(text) {
        Target::Bin(text)
    } else {
        Target::Skip
    }
}

/// The package a path below a `node_modules` directory belongs to (`node_modules/@scope/pkg/cli.js`).
fn installed_package(path: &str) -> Option<&str> {
    package_name(below_node_modules(path)?)
}

/// The package name of a module specifier, without version or subpath. `None` for paths, URLs
/// and dynamic specifiers.
fn package_name(spec: &str) -> Option<&str> {
    if spec.is_empty() || spec.contains([':', '$', '\\']) || spec.starts_with(['.', '/', '~']) {
        return None;
    }
    let (scoped, rest) = spec.strip_prefix('@').map_or((false, spec), |r| (true, r));
    let mut segments = rest.splitn(if scoped { 3 } else { 2 }, '/');
    let first = segments.next()?;
    let end = if scoped {
        let second = segments.next()?;
        1 + first.len() + 1 + second.len()
    } else {
        first.len()
    };
    let name = &spec[..end];
    let skip = usize::from(scoped);
    let name = name[skip..].find('@').map_or(name, |i| &name[..i + skip]);
    (name.len() > skip).then_some(name)
}

/// Whether the first argument of `npx`-like commands names a package instead of a binary.
fn is_package_spec(text: &str) -> bool {
    if text.starts_with('@') {
        text.contains('/')
    } else {
        text.contains('@') && !text.contains('/')
    }
}

fn is_path_like(text: &str) -> bool {
    text.contains('/') || tables::has_script_ext(text)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => {
                if p.get(1) == Some(&'*') {
                    (0..=n.len()).any(|k| go(&p[2..], &n[k..]))
                } else {
                    let mut k = 0;
                    loop {
                        if go(&p[1..], &n[k..]) {
                            return true;
                        }
                        if k < n.len() && n[k] != ':' {
                            k += 1;
                        } else {
                            return false;
                        }
                    }
                }
            }
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    go(&pattern, &name)
}

pub(super) struct Walker<'c> {
    ctx: &'c ScriptContext,
    info: ScriptInfo,
    expanded: HashSet<String>,
    depth: usize,
    /// The workspaces the command being walked is aimed at (`yarn workspace web`, `pnpm --filter web`, `npm -w web`).
    workspaces: Vec<String>,
    /// The functions the script defines; a command calling one runs no binary.
    functions: HashSet<String>,
    /// The directory the commands run in after a `cd` or `pushd`, relative to where the script starts; `None` there
    /// and after a change to a directory that is not a static relative path.
    cwd: Option<String>,
    /// The commands run in a directory the parser cannot place: after a `cd` or `pushd` to one that is not a static
    /// relative path (`cd /tmp`, `cd $DIR`, `cd -`, `cd ~`, a bare `cd`), or to any directory below such a one.
    unknown_dir: bool,
    /// The directories `pushd` left, with whether each was unknown, for `popd`.
    pushed: Vec<(Option<String>, bool)>,
}

/// Where `segment` starts in the text it was tokenized from; `None` for a segment without words.
fn segment_start(segment: &Segment) -> Option<usize> {
    segment
        .assignments
        .first()
        .or_else(|| segment.words.first())
        .map(|word| word.span.start)
}

/// The directory of the last of `commands`, each with where it starts in the text, that starts at or before `offset`:
/// the command that holds a substitution at `offset`. `None` before the first command.
fn dir_at(commands: &[(usize, DirState)], offset: usize) -> Option<&DirState> {
    let held_by = commands.partition_point(|(start, _)| *start <= offset);
    commands.get(held_by.checked_sub(1)?).map(|(_, dir)| dir)
}

/// The directory state of a [`Walker`] that a shell of its own (a subshell, another script, `sh -c`, a substitution)
/// restores when it ends.
#[derive(Clone)]
struct DirState {
    cwd: Option<String>,
    unknown_dir: bool,
    /// The directories `pushd` had left.
    pushed: Vec<(Option<String>, bool)>,
}

impl<'c> Walker<'c> {
    pub(super) fn new(ctx: &'c ScriptContext) -> Self {
        let expanded = ctx.current_script.iter().cloned().collect();
        Self {
            ctx,
            info: ScriptInfo::default(),
            expanded,
            depth: 0,
            workspaces: Vec::new(),
            functions: HashSet::default(),
            cwd: None,
            unknown_dir: false,
            pushed: Vec::new(),
        }
    }

    pub(super) fn run(mut self, script: &str) -> ScriptInfo {
        self.run_text(script, &Map::Offset(0), St::default());
        self.info.dedupe();
        self.info
    }

    fn run_text(&mut self, text: &str, map: &Map, st: St) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        self.depth += 1;
        // A `cd` changes the directory of the commands after it in the same text; the text runs in a shell of its own
        // (another script, `sh -c`, a substitution), so the change ends with it.
        let saved = self.dir_state();
        let mut tokens = tokenize(text);
        self.functions.extend(std::mem::take(&mut tokens.functions));
        let frame = Frame { text, map };
        // So does a subshell `( … )`: the state each open one restores, by its index in `tokens.subshells`, which lists
        // them in the order they close.
        let mut opened: Vec<Option<DirState>> = vec![None; tokens.subshells.len()];
        let mut closed = 0;
        // A substitution runs in the directory of the command that holds it, before that command: where each command
        // starts in the text, a command of redirections alone included, with the directory it runs in.
        let mut commands: Vec<(usize, DirState)> =
            Vec::with_capacity(tokens.segments.len() + tokens.empty_commands.len());
        let mut empty = tokens.empty_commands.iter().peekable();
        for (index, segment) in tokens.segments.iter().enumerate() {
            for (subshell, state) in tokens.subshells.iter().zip(&mut opened) {
                if subshell.start == index && subshell.end > index {
                    *state = Some(self.dir_state());
                }
            }
            self.empty_commands_at(&mut empty, (index, closed), &mut commands);
            if let Some(start) = segment_start(segment) {
                commands.push((start, self.dir_state()));
            }
            self.segment(segment, &frame, st);
            while tokens
                .subshells
                .get(closed)
                .is_some_and(|subshell| subshell.end <= index + 1)
            {
                self.empty_commands_at(&mut empty, (index + 1, closed), &mut commands);
                if let Some(state) = opened[closed].take() {
                    self.restore_dir(state);
                }
                closed += 1;
            }
        }
        self.empty_commands_at(&mut empty, (usize::MAX, usize::MAX), &mut commands);
        for range in &tokens.substitutions {
            if let Some(inner) = text.get(range.clone()) {
                let dir = dir_at(&commands, range.start).unwrap_or(&saved);
                self.restore_dir(dir.clone());
                self.run_text(inner, &map.shifted(range.start), st);
            }
        }
        self.restore_dir(saved);
        self.depth -= 1;
    }

    /// The directory the commands run in, to restore when a shell of their own ends.
    fn dir_state(&self) -> DirState {
        DirState {
            cwd: self.cwd.clone(),
            unknown_dir: self.unknown_dir,
            pushed: self.pushed.clone(),
        }
    }

    fn restore_dir(&mut self, state: DirState) {
        self.cwd = state.cwd;
        self.unknown_dir = state.unknown_dir;
        self.pushed = state.pushed;
    }

    /// Records in `commands` the current directory for each of the `empty` commands that come no later than `at`:
    /// after its count of segments and of closed subshells.
    fn empty_commands_at<'t>(
        &self,
        empty: &mut std::iter::Peekable<impl Iterator<Item = &'t EmptyCommand>>,
        at: (usize, usize),
        commands: &mut Vec<(usize, DirState)>,
    ) {
        while let Some(command) = empty.next_if(|command| (command.segments_before, command.subshells_closed) <= at) {
            commands.push((command.start, self.dir_state()));
        }
    }

    fn segment(&mut self, segment: &Segment, frame: &Frame, st: St) {
        for assignment in &segment.assignments {
            self.assignment(assignment, frame);
        }
        let mut words = segment.words.as_slice();
        while let Some(first) = words.first() {
            if tables::RESERVED_WORDS.contains(&first.text.as_str()) && !first.dynamic {
                words = &words[1..];
            } else {
                break;
            }
        }
        if words
            .first()
            .is_some_and(|w| tables::SKIPPED_COMPOUNDS.contains(&w.text.as_str()))
        {
            return;
        }
        self.command(words, frame, st);
    }

    /// `NODE_OPTIONS` may load modules; other assignments are irrelevant.
    fn assignment(&mut self, word: &Word, frame: &Frame) {
        let Some(value) = word.text.strip_prefix("NODE_OPTIONS=") else {
            return;
        };
        let map = Map::Fixed(frame.span(&word.span));
        let inner = Frame { text: value, map: &map };
        for segment in &tokenize(value).segments {
            self.node_args(&segment.words, &inner, St::default(), false);
        }
    }

    /// A binary the command runs; `bin_path` when the command spells it as a `node_modules/.bin` path.
    fn bin(&mut self, name: &str, span: Range<usize>, st: St, bin_path: bool) {
        self.info.binaries.push(BinaryUse {
            name: name.to_owned(),
            span,
            via: st.via,
            bin_path,
            filtered: st.filtered,
            workspaces: if st.filtered {
                self.workspaces.clone()
            } else {
                Vec::new()
            },
            dir: self.cwd.clone(),
        });
    }

    /// A system binary assumed to be installed globally, kept apart from [`Self::bin`] so that nothing takes it for a
    /// dependency's binary unless the caller asks for it.
    fn global_bin(&mut self, name: &str, span: Range<usize>, st: St) {
        self.info.global_binaries.push(BinaryUse {
            name: name.to_owned(),
            span,
            via: st.via,
            bin_path: false,
            filtered: st.filtered,
            workspaces: Vec::new(),
            dir: None,
        });
    }

    fn file(&mut self, path: &str, span: Range<usize>) {
        self.file_ref(path, span, None);
    }

    /// A file, or with `tree` a path a test runner reads, where a directory means the files the runner reads in it.
    fn file_ref(&mut self, path: &str, span: Range<usize>, tree: Option<FileTree>) {
        if path.is_empty() || path.starts_with('-') || path.contains('$') {
            return;
        }
        if let Some(name) = installed_package(path) {
            self.dep(name, span, false);
            return;
        }
        let path = match &self.cwd {
            Some(cwd) if !path.starts_with('/') => join_rel(cwd, path),
            _ => path.to_owned(),
        };
        self.info.files.push(FileRef { path, span, tree });
    }

    fn package_manager(&mut self, name: &str) {
        if !self.info.package_managers.iter().any(|known| known == name) {
            self.info.package_managers.push(name.to_owned());
        }
    }

    /// Packages a tool loads when it is given a flag or subcommand (`vitest --ui`, `webpack serve`).
    fn flag_dependencies(&mut self, tool: &str, args: &[Word], frame: &Frame) {
        for (name, flag, packages) in tables::FLAG_DEPENDENCIES {
            if *name != tool {
                continue;
            }
            let Some(word) = args.iter().find(|word| {
                word.text == *flag
                    || word
                        .text
                        .strip_prefix(*flag)
                        .is_some_and(|rest| rest.starts_with(['.', '=']))
            }) else {
                continue;
            };
            for package in *packages {
                self.dep(package, frame.span(&word.span), false);
            }
        }
        if tool == "eslint" {
            self.eslint_formatter(args, frame);
        }
    }

    /// `eslint --format <name>` loads `eslint-formatter-<name>` unless the name is built in or a path.
    fn eslint_formatter(&mut self, args: &[Word], frame: &Frame) {
        let inline = args
            .iter()
            .find_map(|word| word.text.strip_prefix("--format=").map(|value| (word, value)));
        let separate = || {
            args.iter()
                .position(|word| word.text == "--format" || word.text == "-f")
                .and_then(|index| args.get(index + 1))
                .map(|word| (word, word.text.as_str()))
        };
        let Some((word, value)) = inline.or_else(separate) else {
            return;
        };
        if tables::ESLINT_FORMATTERS.contains(&value) || value.starts_with('.') || value.contains('$') {
            return;
        }
        let name = match value.split_once('/') {
            Some((scope, _)) if !scope.starts_with('@') => return,
            Some((scope, rest)) if rest.starts_with(tables::ESLINT_FORMATTER_PREFIX) => format!("{scope}/{rest}"),
            Some((scope, rest)) => format!("{scope}/{}{rest}", tables::ESLINT_FORMATTER_PREFIX),
            None if value.starts_with(tables::ESLINT_FORMATTER_PREFIX) => value.to_owned(),
            None => format!("{}{value}", tables::ESLINT_FORMATTER_PREFIX),
        };
        self.dep(&name, frame.span(&word.span), false);
    }

    fn dep(&mut self, name: &str, span: Range<usize>, ephemeral: bool) {
        self.info.dependencies.push(DepRef {
            name: name.to_owned(),
            span,
            ephemeral,
            dir: self.cwd.clone(),
        });
    }

    /// A `--require`-style value: a local file or a package.
    fn module_ref(&mut self, spec: &str, span: Range<usize>) {
        if spec.is_empty() || spec.contains('$') || spec.starts_with("node:") {
            return;
        }
        if spec.starts_with(['.', '/']) {
            self.file(spec, span);
        } else if let Some(path) = spec.strip_prefix("file:") {
            self.file(path, span);
        } else if let Some(name) = package_name(spec) {
            self.dep(name, span, false);
        }
    }

    /// The package's script `name`, run by a package manager or a task runner. In another directory, placed or not, it
    /// is the script of the package there, as with a filter: that package judges its own scripts.
    fn run_script_name(&mut self, name: &str, map: &Map, filtered: bool) {
        if filtered || self.unknown_dir || self.cwd.is_some() || name.is_empty() {
            return;
        }
        if !self.info.run_scripts.iter().any(|s| s == name) {
            self.info.run_scripts.push(name.to_owned());
        }
        let ctx = self.ctx;
        let Some(body) = ctx.scripts.get(name) else {
            return;
        };
        if self.expanded.insert(name.to_owned()) {
            self.run_text(body, map, St::default());
        }
    }

    fn run_script_word(&mut self, word: &Word, frame: &Frame, filtered: bool) {
        let map = Map::Fixed(frame.span(&word.span));
        self.run_script_name(&word.text, &map, filtered);
    }

    fn script_pattern(&mut self, pattern: &str, word: &Word, frame: &Frame) {
        if pattern.is_empty() || pattern.starts_with('!') {
            return;
        }
        let map = Map::Fixed(frame.span(&word.span));
        if pattern.contains('*') {
            let ctx = self.ctx;
            for name in ctx.scripts.keys().filter(|name| glob_match(pattern, name)) {
                self.run_script_name(name, &map, false);
            }
        } else {
            self.run_script_name(pattern, &map, false);
        }
    }

    fn command_or_string(&mut self, words: &[Word], frame: &Frame, st: St) {
        match words {
            [only] if only.text.contains(char::is_whitespace) => {
                let map = frame.child(only);
                self.run_text(&only.text, &map, st);
            }
            _ => self.command(words, frame, st),
        }
    }

    fn command(&mut self, words: &[Word], frame: &Frame, st: St) {
        let Some(first) = words.first() else {
            return;
        };
        if first.dynamic {
            return;
        }
        let args = &words[1..];
        let span = frame.span(&first.span);
        let name = match classify(&first.text) {
            Target::Skip => return,
            Target::File(path) => {
                self.file(path, span);
                return;
            }
            Target::Bin(name) => name,
        };
        if self.functions.contains(name) {
            return;
        }
        let bin_path = name != first.text;
        match name {
            "node" | "nodejs" => self.node_args(args, frame, st, true),
            "npm" => {
                self.package_manager("npm");
                self.npm(args, frame, st);
            }
            "npx" => self.exec_like(
                args,
                frame,
                St {
                    via: BinaryVia::Npx,
                    ..st
                },
                true,
            ),
            "pnpx" => self.exec_like(
                args,
                frame,
                St {
                    via: BinaryVia::Dlx,
                    ..st
                },
                false,
            ),
            "pnpm" => {
                self.package_manager("pnpm");
                self.pnpm(args, frame, st);
            }
            "yarn" | "yarnpkg" => {
                self.package_manager("yarn");
                self.yarn(args, frame, st);
            }
            "bun" => self.bun(args, frame, st),
            "bunx" => self.exec_like(
                args,
                frame,
                St {
                    via: BinaryVia::Bunx,
                    ..st
                },
                false,
            ),
            "corepack" => self.corepack(args, frame, st),
            "concurrently" | "conc" => {
                self.bin(name, span, st, bin_path);
                self.concurrently(args, frame);
            }
            "npm-run-all" | "npm-run-all2" | "run-p" | "run-s" => {
                self.bin(name, span, st, bin_path);
                self.run_all(args, frame);
            }
            // Its arguments are task names and filters, which name scripts of the workspaces, not commands.
            "turbo" => self.bin(name, span, st, bin_path),
            "command"
                if args
                    .iter()
                    .any(|a| a.text.starts_with("-v") || a.text.starts_with("-V")) => {}
            "cd" | "pushd" => {
                if name == "pushd" {
                    self.pushed.push((self.cwd.clone(), self.unknown_dir));
                }
                self.change_dir(args);
            }
            "popd" => (self.cwd, self.unknown_dir) = self.pushed.pop().unwrap_or_default(),
            n if tables::passthrough(n).is_some() => self.passthrough(n, args, frame, st),
            n if tables::SHELLS.contains(&n) => self.shell(args, frame, st),
            n if is_shell_builtin(n) => {}
            n if is_ignored_binary(n) => self.global_bin(n, span, st),
            n => {
                self.bin(n, span, st, bin_path);
                self.flag_dependencies(n, args, frame);
                if let Some(wrapper) = tables::wrapper(n) {
                    self.wrapper(wrapper, args, frame, st);
                } else if let Some(runner) = tables::runner(n) {
                    self.runner(runner, args, frame, st);
                } else if let Some(value_flags) = tables::member_exec(n) {
                    self.member_exec(value_flags, args, frame, st);
                }
            }
        }
    }

    /// `cd <dir>`: the commands after it run in `dir`. A directory that is not a static relative path (`cd`, `cd -`,
    /// `cd ~`, `cd /tmp`, `cd $DIR`) is not followed: a binary run by name there is judged where the script starts,
    /// but a package manager run there works on the package of that directory, as with a filter.
    fn change_dir(&mut self, args: &[Word]) {
        let target = args.iter().find(|word| word.text == "-" || !word.text.starts_with('-'));
        let (dir, dynamic) = target.map_or(("", false), |word| (word.text.as_str(), word.dynamic));
        self.move_to(dir, dynamic);
    }

    /// Moves the commands to `dir`: a static relative path is placed from the current directory, any other directory,
    /// and every directory below one the parser cannot place, is unknown.
    fn move_to(&mut self, dir: &str, dynamic: bool) {
        let placed = !self.unknown_dir && is_static_relative(dir, dynamic);
        self.cwd = if placed { self.relative_dir(dir, dynamic) } else { None };
        self.unknown_dir = !placed;
    }

    /// `dir` as a directory relative to where the script starts, from the current directory; `None` for the start
    /// itself and for a directory that is not a static relative path.
    fn relative_dir(&self, dir: &str, dynamic: bool) -> Option<String> {
        if !is_static_relative(dir, dynamic) {
            return None;
        }
        let joined = join_rel(self.cwd.as_deref().unwrap_or_default(), dir);
        (!joined.is_empty()).then_some(joined)
    }

    /// Moves the commands to `dir`, the value of a flag such as `npm --prefix` or `pnpm -C`, as `cd` does, and returns
    /// the directory state to restore once they are walked.
    fn enter_dir(&mut self, dir: Option<&Val>) -> DirState {
        let saved = self.dir_state();
        if let Some(dir) = dir {
            self.move_to(&dir.text, dir.dynamic);
        }
        saved
    }

    fn passthrough(&mut self, name: &str, args: &[Word], frame: &Frame, st: St) {
        let (value_flags, mut positionals) = tables::passthrough(name).unwrap_or((&[], 0));
        let mut i = 0;
        while let Some(word) = args.get(i) {
            let text = word.text.as_str();
            if is_assignment_text(text) {
                self.assignment(word, frame);
            } else if let Some(flag) = text.strip_prefix('-').filter(|flag| !flag.is_empty()) {
                if value_flags.contains(&flag.trim_start_matches('-')) {
                    i += 1;
                }
            } else if positionals > 0 {
                positionals -= 1;
            } else {
                break;
            }
            i += 1;
        }
        self.command_or_string(args.get(i..).unwrap_or_default(), frame, st);
    }

    /// `sh -c "script"` runs the script; `sh file.sh` runs the file.
    fn shell(&mut self, args: &[Word], frame: &Frame, st: St) {
        for (i, word) in args.iter().enumerate() {
            let text = word.text.as_str();
            let is_command_flag = text.len() > 1
                && text.starts_with('-')
                && !text.starts_with("--")
                && text[1..].chars().all(|c| c.is_ascii_alphabetic())
                && text.contains('c');
            if is_command_flag {
                if let Some(script) = args.get(i + 1) {
                    let map = frame.child(script);
                    self.run_text(&script.text, &map, st);
                }
                return;
            }
            if !text.starts_with('-') {
                self.file(text, frame.span(&word.span));
                return;
            }
        }
    }

    fn wrapper(&mut self, wrapper: &Wrapper, args: &[Word], frame: &Frame, st: St) {
        let rest = match (wrapper.subcommand, args.first()) {
            (Some(sub), Some(first)) if first.text == sub => &args[1..],
            _ => args,
        };
        let mut i = 0;
        let mut command = None;
        while let Some(word) = rest.get(i) {
            let text = word.text.as_str();
            if text == "--" {
                command = Some(i + 1);
                break;
            }
            if wrapper.assignments && is_assignment_text(text) {
                self.assignment(word, frame);
                i += 1;
            } else if let Some(flag) = text.strip_prefix('-').filter(|f| !f.is_empty()) {
                let name = flag.trim_start_matches('-');
                i += if !text.contains('=') && wrapper.value_flags.contains(&name) {
                    2
                } else {
                    1
                };
            } else {
                command = Some(i);
                break;
            }
        }
        if let Some(start) = command.filter(|&start| start < rest.len()) {
            self.command_or_string(&rest[start..], frame, st);
        }
    }

    fn node_args(&mut self, args: &[Word], frame: &Frame, st: St, files: bool) {
        let parsed = args::parse(args, tables::NODE_VALUE_FLAGS, true);
        for flag in &parsed.flags {
            let Some(value) = &flag.value else {
                continue;
            };
            if tables::NODE_MODULE_FLAGS.contains(&flag.name.as_str())
                || (flag.name == "test-reporter" && !tables::NODE_BUILTIN_REPORTERS.contains(&value.text.as_str()))
            {
                self.module_ref(&value.text, frame.span(&value.span));
            } else if flag.name == "run" {
                let map = Map::Fixed(frame.span(&value.span));
                self.run_script_name(&value.text, &map, st.filtered);
            }
        }
        if parsed.has(&["test"]) {
            self.info.node_test = true;
        }
        if files && let Some(&i) = parsed.positionals.first() {
            self.file(&args[i].text, frame.span(&args[i].span));
        }
    }

    /// `npx`, `npm exec`, `pnpm dlx`, `yarn dlx` and `bunx`: run a package binary, possibly fetched
    /// on demand. `args` start after the command name.
    fn exec_like(&mut self, args: &[Word], frame: &Frame, st: St, call: bool) {
        let flags = if call {
            tables::EXEC_VALUE_FLAGS
        } else {
            tables::DLX_VALUE_FLAGS
        };
        let parsed = args::parse(args, flags, true);
        let mut explicit = false;
        let mut called = false;
        for flag in &parsed.flags {
            let Some(value) = &flag.value else {
                continue;
            };
            match flag.name.as_str() {
                "package" | "p" => {
                    explicit = true;
                    if let Some(name) = package_name(&value.text) {
                        self.dep(name, frame.span(&value.span), true);
                    }
                }
                "call" | "c" if call => {
                    called = true;
                    let map = Map::Fixed(frame.span(&value.span));
                    self.run_text(&value.text, &map, St::default());
                }
                _ => {}
            }
        }
        let command = command_words(args, &parsed);
        if called {
            return;
        }
        let Some(first) = command.first() else {
            return;
        };
        if !explicit && is_package_spec(&first.text) {
            if let Some(name) = package_name(&first.text) {
                self.dep(name, frame.span(&first.span), true);
            }
        } else {
            self.command(command, frame, st);
        }
    }

    fn npm(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, tables::NPM_VALUE_FLAGS, true);
        let Some(&ci) = parsed.positionals.first() else {
            return;
        };
        let after = &args[parsed.rest..];
        let filtered = st.filtered || self.unknown_dir || parsed.has(tables::WORKSPACE_FLAGS);
        let trailing = args::parse(after, tables::NPM_VALUE_FLAGS, false);
        let names = parsed
            .values(&["workspace", "w"])
            .chain(trailing.values(&["workspace", "w"]))
            .map(|value| value.text.clone())
            .collect();
        let saved = std::mem::replace(&mut self.workspaces, names);
        let prefix = parsed.values(&["prefix"]).chain(trailing.values(&["prefix"])).next();
        let cwd = self.enter_dir(prefix);
        match args[ci].text.as_str() {
            "run" | "run-script" | "rum" | "urn" => {
                let inner = args::parse(after, tables::NPM_VALUE_FLAGS, false);
                let filtered = filtered || inner.has(tables::WORKSPACE_FLAGS);
                if let Some(&i) = inner.positionals.first() {
                    self.run_script_word(&after[i], frame, filtered);
                }
            }
            "exec" | "x" => {
                let st = St {
                    filtered: filtered || trailing.has(tables::WORKSPACE_FLAGS),
                    via: BinaryVia::NpmExec,
                };
                self.exec_like(after, frame, st, true);
            }
            "start" | "stop" | "restart" | "test" => {
                let map = Map::Fixed(frame.span(&args[ci].span));
                self.run_script_name(&args[ci].text, &map, filtered);
            }
            "t" | "tst" => {
                let map = Map::Fixed(frame.span(&args[ci].span));
                self.run_script_name("test", &map, filtered);
            }
            _ => {}
        }
        self.restore_dir(cwd);
        self.workspaces = saved;
    }

    fn pnpm(&mut self, args: &[Word], frame: &Frame, st: St) {
        let mut args = args;
        let mut recursive = false;
        let mut filter = false;
        let mut names: Vec<String> = Vec::new();
        let (ci, parsed) = loop {
            let parsed = args::parse(args, tables::PNPM_VALUE_FLAGS, true);
            // `--dir`, `-C` and `--prefix` run the command in another directory, like a filter.
            filter |= parsed.has(&["filter", "F", "filter-prod", "dir", "C", "prefix"]);
            names.extend(
                parsed
                    .values(&["filter", "F", "filter-prod"])
                    .map(|value| value.text.clone()),
            );
            recursive |= parsed.has(&["recursive", "r"]);
            let Some(&ci) = parsed.positionals.first() else {
                return;
            };
            if matches!(args[ci].text.as_str(), "recursive" | "multi" | "m") {
                recursive = true;
                args = &args[parsed.rest..];
                continue;
            }
            break (ci, parsed);
        };
        let after = &args[parsed.rest..];
        let filtered = st.filtered || self.unknown_dir || filter;
        let cmd = args[ci].text.as_str();
        let trailing = args::parse(after, tables::PNPM_VALUE_FLAGS, false);
        names.extend(
            trailing
                .values(&["filter", "F", "filter-prod"])
                .map(|value| value.text.clone()),
        );
        let targeted = !names.is_empty();
        let saved = std::mem::replace(&mut self.workspaces, names);
        let dir = parsed.values(&["dir", "C", "prefix"]).next();
        let cwd = self.enter_dir(dir);
        match cmd {
            "run" | "run-script" => {
                let inner = args::parse(after, tables::PNPM_VALUE_FLAGS, false);
                let selected = filtered || inner.has(&["filter", "F", "filter-prod"]);
                let recursive = recursive || inner.has(&["recursive", "r"]);
                if let Some(&i) = inner.positionals.first() {
                    // A recursive run of a script this package doesn't define runs it in other workspaces only.
                    let elsewhere = recursive && !self.ctx.scripts.contains_key(after[i].text.as_str());
                    self.run_script_word(&after[i], frame, selected || elsewhere);
                }
            }
            "exec" => {
                let inner = args::parse(after, tables::PNPM_VALUE_FLAGS, true);
                let command = command_words(after, &inner);
                self.command_or_string(
                    command,
                    frame,
                    St {
                        filtered,
                        via: BinaryVia::PnpmExec,
                    },
                );
            }
            "dlx" => self.exec_like(
                after,
                frame,
                St {
                    filtered,
                    via: BinaryVia::Dlx,
                },
                false,
            ),
            _ => {
                let name = if matches!(cmd, "t" | "tst") { "test" } else { cmd };
                if self.ctx.scripts.contains_key(name) {
                    let map = Map::Fixed(frame.span(&args[ci].span));
                    self.run_script_name(name, &map, filtered);
                } else if (!filtered || targeted)
                    && !recursive
                    && !tables::PNPM_COMMANDS.contains(&cmd)
                    && !tables::LIFECYCLE.contains(&cmd)
                {
                    self.command(
                        &args[ci..],
                        frame,
                        St {
                            filtered,
                            via: BinaryVia::PnpmExec,
                        },
                    );
                }
            }
        }
        self.restore_dir(cwd);
        self.workspaces = saved;
    }

    fn yarn(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, tables::YARN_VALUE_FLAGS, true);
        let Some(&ci) = parsed.positionals.first() else {
            return;
        };
        let filtered = st.filtered || self.unknown_dir || parsed.has(&["cwd"]);
        let targeted = !self.workspaces.is_empty();
        let after = &args[parsed.rest..];
        let cmd = args[ci].text.as_str();
        let cwd = self.enter_dir(parsed.values(&["cwd"]).next());
        match cmd {
            "run" => {
                let inner = args::parse(after, &[], false);
                if let Some(&i) = inner.positionals.first()
                    && (!filtered || (targeted && !tables::LIFECYCLE.contains(&after[i].text.as_str())))
                {
                    if !filtered && self.ctx.scripts.contains_key(&after[i].text) {
                        self.run_script_word(&after[i], frame, filtered);
                    } else {
                        self.command(
                            &after[i..],
                            frame,
                            St {
                                filtered,
                                via: BinaryVia::YarnRun,
                            },
                        );
                    }
                }
            }
            "dlx" => self.exec_like(
                after,
                frame,
                St {
                    filtered,
                    via: BinaryVia::Dlx,
                },
                false,
            ),
            "exec" => self.command_or_string(
                after,
                frame,
                St {
                    filtered,
                    via: BinaryVia::YarnExec,
                },
            ),
            "node" => self.node_args(after, frame, St { filtered, ..st }, true),
            "workspace" => {
                if let Some(name) = after.first()
                    && after.len() > 1
                {
                    let saved = std::mem::replace(&mut self.workspaces, vec![name.text.clone()]);
                    self.yarn(&after[1..], frame, St { filtered: true, ..st });
                    self.workspaces = saved;
                }
            }
            "workspaces" => {
                if after.first().is_some_and(|w| w.text == "foreach") {
                    let rest = &after[1..];
                    let inner = args::parse(rest, tables::YARN_FOREACH_VALUE_FLAGS, true);
                    if let Some(&i) = inner.positionals.first() {
                        self.yarn(&rest[i..], frame, St { filtered: true, ..st });
                    }
                }
            }
            _ => {
                if !filtered && self.ctx.scripts.contains_key(cmd) {
                    let map = Map::Fixed(frame.span(&args[ci].span));
                    self.run_script_name(cmd, &map, filtered);
                } else if (!filtered || targeted)
                    && !tables::YARN_COMMANDS.contains(&cmd)
                    && !tables::LIFECYCLE.contains(&cmd)
                {
                    self.command(
                        &args[ci..],
                        frame,
                        St {
                            filtered,
                            via: BinaryVia::YarnRun,
                        },
                    );
                }
            }
        }
        self.restore_dir(cwd);
    }

    /// `bun`: `--filter` (`-F`) and `--workspaces` run the command in other workspace packages; a binary the command
    /// runs belongs to the packages `--filter` names.
    fn bun(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, tables::BUN_VALUE_FLAGS, true);
        for value in parsed.values(&["preload", "r", "import", "require"]) {
            self.module_ref(&value.text, frame.span(&value.span));
        }
        // `--cwd <dir>` runs the command in another directory: its scripts and binaries are that package's.
        let mut cwd = parsed.values(&["cwd"]).next().map(|value| value.text.clone());
        let mut filtered = st.filtered || self.unknown_dir || parsed.has(BUN_WORKSPACE_FLAGS) || cwd.is_some();
        let Some(&ci) = parsed.positionals.first() else {
            return;
        };
        let after = &args[parsed.rest..];
        let cmd = args[ci].text.as_str();
        let inner = (cmd == "run").then(|| args::parse(after, tables::BUN_VALUE_FLAGS, false));
        let names = parsed
            .values(BUN_FILTER_FLAGS)
            .chain(inner.iter().flat_map(|inner| inner.values(BUN_FILTER_FLAGS)))
            .map(|value| value.text.clone())
            .collect();
        let saved = std::mem::replace(&mut self.workspaces, names);
        match cmd {
            "x" => self.exec_like(
                after,
                frame,
                St {
                    filtered,
                    via: BinaryVia::Bunx,
                },
                false,
            ),
            "run" => {
                if let Some(inner) = &inner {
                    filtered |= inner.has(BUN_WORKSPACE_FLAGS);
                    if let Some(value) = inner.values(&["cwd"]).next() {
                        cwd = Some(value.text.clone());
                        filtered = true;
                    }
                    if let Some(&i) = inner.positionals.first() {
                        self.bun_target(&after[i..], frame, filtered, cwd.as_deref());
                    }
                }
            }
            _ => {
                if self.ctx.scripts.contains_key(cmd) {
                    let map = Map::Fixed(frame.span(&args[ci].span));
                    self.run_script_name(cmd, &map, filtered);
                } else if is_path_like(cmd) {
                    // `bun check` may name a script of the workspace root, which this package's scripts cannot
                    // tell, so only a file is certain.
                    self.file_in(cwd.as_deref(), cmd, frame.span(&args[ci].span));
                }
            }
        }
        self.workspaces = saved;
    }

    /// A file a command run in `cwd` names, as a path relative to the package directory. Nothing is recorded when
    /// `cwd` is not a static relative directory.
    fn file_in(&mut self, cwd: Option<&str>, path: &str, span: Range<usize>) {
        match cwd {
            None => self.file(path, span),
            Some(_) if path.starts_with('/') => self.file(path, span),
            Some(dir) if !dir.starts_with('/') && !dir.contains('$') => self.file(&join_rel(dir, path), span),
            Some(_) => {}
        }
    }

    /// The target of `bun run <target>`: a script, a file or a binary. With `cwd`, it runs in that directory. Run in
    /// other packages (`--filter`, `--workspaces`), Bun runs only their scripts, so no binary is recorded.
    fn bun_target(&mut self, from: &[Word], frame: &Frame, filtered: bool, cwd: Option<&str>) {
        let Some(word) = from.first() else {
            return;
        };
        if self.ctx.scripts.contains_key(&word.text) {
            self.run_script_word(word, frame, filtered);
        } else if is_path_like(&word.text) {
            self.file_in(cwd, &word.text, frame.span(&word.span));
        } else if !filtered {
            self.command(
                from,
                frame,
                St {
                    filtered,
                    via: BinaryVia::BunRun,
                },
            );
        }
    }

    fn corepack(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, &[], true);
        if let Some(&i) = parsed.positionals.first()
            && matches!(args[i].text.as_str(), "npm" | "pnpm" | "yarn" | "bun" | "npx" | "pnpx")
        {
            self.command(&args[i..], frame, st);
        }
    }

    fn concurrently(&mut self, args: &[Word], frame: &Frame) {
        let parsed = args::parse(args, tables::CONCURRENTLY_VALUE_FLAGS, false);
        for value in parsed.values(&["teardown"]) {
            let map = Map::Fixed(frame.span(&value.span));
            self.run_text(&value.text, &map, St::default());
        }
        for &i in &parsed.positionals {
            let word = &args[i];
            let text = word.text.trim();
            if let Some(("npm" | "pnpm" | "yarn" | "bun", rest)) = text.split_once(':') {
                let name = rest.split_whitespace().next().unwrap_or("");
                self.script_pattern(name, word, frame);
            } else {
                let map = frame.child(word);
                self.run_text(&word.text, &map, St::default());
            }
        }
    }

    fn run_all(&mut self, args: &[Word], frame: &Frame) {
        let parsed = args::parse(args, tables::RUN_ALL_VALUE_FLAGS, false);
        for &i in &parsed.positionals {
            let word = &args[i];
            let name = word.text.split_whitespace().next().unwrap_or("");
            self.script_pattern(name, word, frame);
        }
    }

    /// `lerna exec [flags] [--] <cmd>` and `nx exec [flags] -- <cmd>` run `<cmd>` in each member with the root's
    /// binaries on the `PATH`, so it is walked as a command of this package, as `pnpm -r exec` is.
    fn member_exec(&mut self, value_flags: &[&str], args: &[Word], frame: &Frame, st: St) {
        let Some(("exec", rest)) = args.split_first().map(|(first, rest)| (first.text.as_str(), rest)) else {
            return;
        };
        let parsed = args::parse(rest, value_flags, true);
        let command = command_words(rest, &parsed);
        self.command_or_string(
            command,
            frame,
            St {
                via: BinaryVia::Direct,
                ..st
            },
        );
    }

    fn runner(&mut self, runner: &Runner, args: &[Word], frame: &Frame, st: St) {
        let value_flags: Vec<&str> = [
            runner.value_flags,
            runner.dep_flags,
            runner.file_flags,
            runner.exec_flags,
        ]
        .concat();
        let stop = matches!(runner.files, Files::First);
        let mut rest = args;
        let mut parsed = args::parse(rest, &value_flags, stop);
        self.runner_flags(runner, &parsed, frame, st);
        if stop
            && let Some(&i) = parsed.positionals.first()
            && runner.subcommands.contains(&rest[i].text.as_str())
        {
            rest = &rest[parsed.rest..];
            parsed = args::parse(rest, &value_flags, true);
            self.runner_flags(runner, &parsed, frame, st);
        }
        match runner.files {
            Files::First => {
                if let Some(&i) = parsed.positionals.first() {
                    self.file(&rest[i].text, frame.span(&rest[i].span));
                }
            }
            Files::ScriptExt | Files::All => {
                let tree = matches!(runner.files, Files::All).then(|| test_tree(&parsed));
                for &i in &parsed.positionals {
                    let word = &rest[i];
                    if let Some(tree) = &tree {
                        self.file_ref(&word.text, frame.span(&word.span), Some(tree.clone()));
                    } else if tables::has_script_ext(&word.text) {
                        self.file(&word.text, frame.span(&word.span));
                    }
                }
            }
            Files::None => {}
        }
    }

    fn runner_flags(&mut self, runner: &Runner, parsed: &Parsed, frame: &Frame, st: St) {
        for flag in &parsed.flags {
            let Some(Val { text, span, .. }) = &flag.value else {
                continue;
            };
            let name = flag.name.as_str();
            if runner.dep_flags.contains(&name) {
                self.module_ref(text, frame.span(span));
            } else if runner.file_flags.contains(&name) {
                self.file(text, frame.span(span));
            } else if runner.exec_flags.contains(&name) {
                let map = Map::Fixed(frame.span(span));
                self.run_text(
                    text,
                    &map,
                    St {
                        via: BinaryVia::Direct,
                        ..st
                    },
                );
            }
        }
    }
}

/// How mocha reads a directory argument: only with `--recursive` below its direct children, and only files with an
/// `--extension` value (comma-separated or repeated), `js`, `cjs` and `mjs` by default.
fn test_tree(parsed: &Parsed) -> FileTree {
    let mut extensions: Vec<String> = parsed
        .values(&["extension"])
        .flat_map(|value| value.text.split(','))
        .map(|extension| extension.trim().trim_start_matches('.').to_owned())
        .filter(|extension| !extension.is_empty())
        .collect();
    if extensions.is_empty() {
        extensions = tables::TEST_TREE_EXTENSIONS
            .iter()
            .map(|&extension| extension.to_owned())
            .collect();
    }
    FileTree {
        recursive: parsed.has(&["recursive"]),
        extensions,
    }
}

/// The words of the command after `exec`: from the first positional, or after `--`.
fn command_words<'a>(words: &'a [Word], parsed: &Parsed) -> &'a [Word] {
    if let Some(&i) = parsed.positionals.first() {
        &words[i..]
    } else if let Some(d) = parsed.dashdash {
        &words[d + 1..]
    } else {
        &[]
    }
}
