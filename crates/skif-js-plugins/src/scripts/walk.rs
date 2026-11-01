//! Interprets tokenized commands and collects what they reference.

use std::collections::HashSet;
use std::ops::Range;

use super::args::{self, Parsed, Val};
use super::tables::{self, Files, Runner, Wrapper};
use super::tokenize::{Segment, Word, is_assignment_text, tokenize};
use super::{BinaryUse, BinaryVia, DepRef, FileRef, ScriptContext, ScriptInfo, is_ignored_binary};

const MAX_DEPTH: usize = 32;

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
    name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'))
}

fn classify(text: &str) -> Target<'_> {
    let bin_dir = text
        .rfind("/.bin/")
        .map(|i| &text[i + "/.bin/".len()..])
        .or_else(|| text.strip_prefix(".bin/"));
    if let Some(name) = bin_dir {
        return if valid_binary(name) { Target::Bin(name) } else { Target::Skip };
    }
    if text.starts_with('/') || text.starts_with('~') || text == "." || text == ".." {
        return Target::Skip;
    }
    if text.contains('/') {
        return Target::File(text);
    }
    if valid_binary(text) { Target::Bin(text) } else { Target::Skip }
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
}

impl<'c> Walker<'c> {
    pub(super) fn new(ctx: &'c ScriptContext) -> Self {
        let expanded = ctx.current_script.iter().cloned().collect();
        Self {
            ctx,
            info: ScriptInfo::default(),
            expanded,
            depth: 0,
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
        let tokens = tokenize(text);
        let frame = Frame { text, map };
        for segment in &tokens.segments {
            self.segment(segment, &frame, st);
        }
        for range in &tokens.substitutions {
            if let Some(inner) = text.get(range.clone()) {
                self.run_text(inner, &map.shifted(range.start), st);
            }
        }
        self.depth -= 1;
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

    fn bin(&mut self, name: &str, span: Range<usize>, st: St) {
        self.info.binaries.push(BinaryUse {
            name: name.to_owned(),
            span,
            via: st.via,
            filtered: st.filtered,
        });
    }

    fn file(&mut self, path: &str, span: Range<usize>) {
        if path.is_empty() || path.starts_with('-') || path.contains('$') {
            return;
        }
        self.info.files.push(FileRef {
            path: path.to_owned(),
            span,
        });
    }

    fn dep(&mut self, name: &str, span: Range<usize>, ephemeral: bool) {
        self.info.dependencies.push(DepRef {
            name: name.to_owned(),
            span,
            ephemeral,
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

    fn run_script_name(&mut self, name: &str, map: &Map, filtered: bool) {
        if filtered || name.is_empty() {
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
        match name {
            "node" | "nodejs" => self.node_args(args, frame, st, true),
            "npm" => self.npm(args, frame, st),
            "npx" => self.exec_like(args, frame, St { via: BinaryVia::Npx, ..st }, true),
            "pnpx" => self.exec_like(args, frame, St { via: BinaryVia::Dlx, ..st }, false),
            "pnpm" => self.pnpm(args, frame, st),
            "yarn" | "yarnpkg" => self.yarn(args, frame, st),
            "bun" => self.bun(args, frame, st),
            "bunx" => self.exec_like(args, frame, St { via: BinaryVia::Bunx, ..st }, false),
            "corepack" => self.corepack(args, frame, st),
            "concurrently" | "conc" => {
                self.bin(name, span, st);
                self.concurrently(args, frame);
            }
            "npm-run-all" | "npm-run-all2" | "run-p" | "run-s" => {
                self.bin(name, span, st);
                self.run_all(args, frame);
            }
            "turbo" => {
                self.bin(name, span, st);
                self.turbo(args);
            }
            "command" if args.iter().any(|a| a.text.starts_with("-v") || a.text.starts_with("-V")) => {}
            n if tables::PASSTHROUGH.contains(&n) => self.passthrough(args, frame, st),
            n if tables::SHELLS.contains(&n) => self.shell(args, frame, st),
            n if is_ignored_binary(n) => {}
            n => {
                self.bin(n, span, st);
                if let Some(wrapper) = tables::wrapper(n) {
                    self.wrapper(wrapper, args, frame, st);
                } else if let Some(runner) = tables::runner(n) {
                    self.runner(runner, args, frame, st);
                }
            }
        }
    }

    fn passthrough(&mut self, args: &[Word], frame: &Frame, st: St) {
        let mut i = 0;
        while let Some(word) = args.get(i) {
            if is_assignment_text(&word.text) {
                self.assignment(word, frame);
            } else if !word.text.starts_with('-') {
                break;
            }
            i += 1;
        }
        self.command_or_string(&args[i..], frame, st);
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
                i += if !text.contains('=') && wrapper.value_flags.contains(&name) { 2 } else { 1 };
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
            if tables::NODE_MODULE_FLAGS.contains(&flag.name.as_str()) {
                self.module_ref(&value.text, frame.span(&value.span));
            } else if flag.name == "run" {
                let map = Map::Fixed(frame.span(&value.span));
                self.run_script_name(&value.text, &map, st.filtered);
            }
        }
        if files && let Some(&i) = parsed.positionals.first() {
            self.file(&args[i].text, frame.span(&args[i].span));
        }
    }

    /// `npx`, `npm exec`, `pnpm dlx`, `yarn dlx` and `bunx`: run a package binary, possibly fetched
    /// on demand. `args` start after the command name.
    fn exec_like(&mut self, args: &[Word], frame: &Frame, st: St, call: bool) {
        let flags = if call { tables::EXEC_VALUE_FLAGS } else { tables::DLX_VALUE_FLAGS };
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
        let filtered = st.filtered || parsed.has(tables::WORKSPACE_FLAGS);
        match args[ci].text.as_str() {
            "run" | "run-script" | "rum" | "urn" => {
                let inner = args::parse(after, tables::NPM_VALUE_FLAGS, false);
                let filtered = filtered || inner.has(tables::WORKSPACE_FLAGS);
                if let Some(&i) = inner.positionals.first() {
                    self.run_script_word(&after[i], frame, filtered);
                }
            }
            "exec" | "x" => {
                let st = St { filtered, via: BinaryVia::NpmExec };
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
    }

    fn pnpm(&mut self, args: &[Word], frame: &Frame, st: St) {
        let mut args = args;
        let mut recursive = false;
        let mut filter = false;
        let (ci, parsed) = loop {
            let parsed = args::parse(args, tables::PNPM_VALUE_FLAGS, true);
            filter |= parsed.has(&["filter", "F", "filter-prod"]);
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
        let filtered = st.filtered || (filter && !recursive);
        let cmd = args[ci].text.as_str();
        match cmd {
            "run" | "run-script" => {
                let inner = args::parse(after, tables::PNPM_VALUE_FLAGS, false);
                if let Some(&i) = inner.positionals.first() {
                    self.run_script_word(&after[i], frame, filtered);
                }
            }
            "exec" => {
                let inner = args::parse(after, tables::PNPM_VALUE_FLAGS, true);
                let command = command_words(after, &inner);
                self.command_or_string(command, frame, St { filtered, via: BinaryVia::PnpmExec });
            }
            "dlx" => self.exec_like(after, frame, St { filtered, via: BinaryVia::Dlx }, false),
            _ => {
                let name = if matches!(cmd, "t" | "tst") { "test" } else { cmd };
                if self.ctx.scripts.contains_key(name) {
                    let map = Map::Fixed(frame.span(&args[ci].span));
                    self.run_script_name(name, &map, filtered);
                } else if !filtered && !tables::PNPM_COMMANDS.contains(&cmd) && !tables::LIFECYCLE.contains(&cmd) {
                    self.command(&args[ci..], frame, St { filtered, via: BinaryVia::PnpmExec });
                }
            }
        }
    }

    fn yarn(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, tables::YARN_VALUE_FLAGS, true);
        let Some(&ci) = parsed.positionals.first() else {
            return;
        };
        let filtered = st.filtered || parsed.has(&["cwd"]);
        let after = &args[parsed.rest..];
        let cmd = args[ci].text.as_str();
        match cmd {
            "run" => {
                let inner = args::parse(after, &[], false);
                if let Some(&i) = inner.positionals.first()
                    && !filtered
                {
                    if self.ctx.scripts.contains_key(&after[i].text) {
                        self.run_script_word(&after[i], frame, filtered);
                    } else {
                        self.command(&after[i..], frame, St { filtered, via: BinaryVia::YarnRun });
                    }
                }
            }
            "dlx" => self.exec_like(after, frame, St { filtered, via: BinaryVia::Dlx }, false),
            "exec" => self.command_or_string(after, frame, St { filtered, via: BinaryVia::YarnExec }),
            "node" => self.node_args(after, frame, St { filtered, ..st }, true),
            "workspace" => {
                if after.len() > 1 {
                    self.yarn(&after[1..], frame, St { filtered: true, ..st });
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
                } else if !filtered && !tables::YARN_COMMANDS.contains(&cmd) && !tables::LIFECYCLE.contains(&cmd) {
                    self.command(&args[ci..], frame, St { filtered, via: BinaryVia::YarnRun });
                }
            }
        }
    }

    fn bun(&mut self, args: &[Word], frame: &Frame, st: St) {
        let parsed = args::parse(args, tables::BUN_VALUE_FLAGS, true);
        for value in parsed.values(&["preload", "r", "import", "require"]) {
            self.module_ref(&value.text, frame.span(&value.span));
        }
        let mut filtered = st.filtered || parsed.has(&["filter"]);
        let Some(&ci) = parsed.positionals.first() else {
            return;
        };
        let after = &args[parsed.rest..];
        let cmd = args[ci].text.as_str();
        match cmd {
            "x" => self.exec_like(after, frame, St { filtered, via: BinaryVia::Bunx }, false),
            "run" => {
                let inner = args::parse(after, tables::BUN_VALUE_FLAGS, false);
                filtered |= inner.has(&["filter"]);
                if let Some(&i) = inner.positionals.first() {
                    self.bun_target(&after[i..], frame, filtered);
                }
            }
            _ => {
                if self.ctx.scripts.contains_key(cmd) {
                    let map = Map::Fixed(frame.span(&args[ci].span));
                    self.run_script_name(cmd, &map, filtered);
                } else if !tables::BUN_COMMANDS.contains(&cmd) && !tables::LIFECYCLE.contains(&cmd) {
                    self.bun_target(&args[ci..], frame, filtered);
                }
            }
        }
    }

    /// The target of `bun run <target>`: a script, a file or a binary.
    fn bun_target(&mut self, from: &[Word], frame: &Frame, filtered: bool) {
        let Some(word) = from.first() else {
            return;
        };
        if self.ctx.scripts.contains_key(&word.text) {
            self.run_script_word(word, frame, filtered);
        } else if is_path_like(&word.text) {
            self.file(&word.text, frame.span(&word.span));
        } else if !filtered {
            self.command(from, frame, St { filtered, via: BinaryVia::BunRun });
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
            match text.split_once(':') {
                Some((manager, rest)) if matches!(manager, "npm" | "pnpm" | "yarn" | "bun") => {
                    let name = rest.split_whitespace().next().unwrap_or("");
                    self.script_pattern(name, word, frame);
                }
                _ => {
                    let map = frame.child(word);
                    self.run_text(&word.text, &map, St::default());
                }
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

    fn turbo(&mut self, args: &[Word]) {
        let parsed = args::parse(args, tables::TURBO_VALUE_FLAGS, false);
        let mut names: Vec<&str> = parsed.positionals.iter().map(|&i| args[i].text.as_str()).collect();
        match names.first().copied() {
            None => return,
            Some("run" | "watch") => {
                names.remove(0);
            }
            Some(first) if tables::TURBO_SUBCOMMANDS.contains(&first) => return,
            Some(_) => {}
        }
        for name in names {
            if !self.info.turbo_tasks.iter().any(|t| t == name) {
                self.info.turbo_tasks.push(name.to_owned());
            }
        }
    }

    fn runner(&mut self, runner: &Runner, args: &[Word], frame: &Frame, st: St) {
        let value_flags: Vec<&str> = [runner.value_flags, runner.dep_flags, runner.file_flags, runner.exec_flags]
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
                for &i in &parsed.positionals {
                    let word = &rest[i];
                    if matches!(runner.files, Files::All) || tables::has_script_ext(&word.text) {
                        self.file(&word.text, frame.span(&word.span));
                    }
                }
            }
        }
    }

    fn runner_flags(&mut self, runner: &Runner, parsed: &Parsed, frame: &Frame, st: St) {
        for flag in &parsed.flags {
            let Some(Val { text, span }) = &flag.value else {
                continue;
            };
            let name = flag.name.as_str();
            if runner.dep_flags.contains(&name) {
                self.module_ref(text, frame.span(span));
            } else if runner.file_flags.contains(&name) {
                self.file(text, frame.span(span));
            } else if runner.exec_flags.contains(&name) {
                let map = Map::Fixed(frame.span(span));
                self.run_text(text, &map, St { via: BinaryVia::Direct, ..st });
            }
        }
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
