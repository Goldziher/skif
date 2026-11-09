//! A small POSIX-ish tokenizer: quotes, escapes, separators, redirections, substitutions, here-documents, `case`
//! patterns and function definitions.

use std::ops::Range;

use super::tables::RESERVED_WORDS;

/// One shell word with quotes and escapes resolved.
#[derive(Debug, Clone)]
pub(super) struct Word {
    pub text: String,
    /// Byte range of the raw word (quotes included) in the tokenized text.
    pub span: Range<usize>,
    /// The word contains `$`, a backtick or a command substitution, so its value is not static.
    pub dynamic: bool,
}

/// A simple command: leading `NAME=value` assignments followed by the command words.
#[derive(Debug, Default)]
pub(super) struct Segment {
    pub assignments: Vec<Word>,
    pub words: Vec<Word>,
}

#[derive(Debug, Default)]
pub(super) struct Tokens {
    pub segments: Vec<Segment>,
    /// Byte ranges of the bodies of `$( )` and backtick substitutions.
    pub substitutions: Vec<Range<usize>>,
    /// Names of the functions the text defines (`name() { … }`, `function name { … }`): calling one runs no binary.
    pub functions: Vec<String>,
}

pub(super) fn is_assignment_text(text: &str) -> bool {
    let Some((name, _)) = text.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn tokenize(src: &str) -> Tokens {
    Lexer {
        src,
        chars: src.char_indices().collect(),
        i: 0,
        tokens: Tokens::default(),
        words: Vec::new(),
        cur: None,
        redirect: false,
        heredoc_target: false,
        heredocs: Vec::new(),
        case_depth: 0,
        case_pattern: false,
    }
    .run()
}

/// The words after the leading reserved words (`then`, `do`, `{`, …) of a command.
fn after_reserved(words: &[Word]) -> &[Word] {
    let lead = words
        .iter()
        .take_while(|word| !word.dynamic && RESERVED_WORDS.contains(&word.text.as_str()))
        .count();
    &words[lead..]
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    i: usize,
    tokens: Tokens,
    words: Vec<Word>,
    cur: Option<Word>,
    /// The next completed word is a redirection target and is discarded.
    redirect: bool,
    /// The redirection target pending in `redirect` is the delimiter of a here-document (`<<EOF`, `<<-'EOF'`).
    heredoc_target: bool,
    /// Delimiters of the here-documents whose bodies start after the current line, in order.
    heredocs: Vec<String>,
    /// How many `case … in` constructs are open.
    case_depth: usize,
    /// The next token starts the pattern list of a `case` item (`a|b)`), or the `esac` that closes the construct.
    case_pattern: bool,
}

impl Lexer<'_> {
    fn pos(&self, i: usize) -> usize {
        self.chars.get(i).map_or(self.src.len(), |c| c.0)
    }

    fn at(&self, i: usize) -> Option<char> {
        self.chars.get(i).map(|c| c.1)
    }

    fn word(&mut self) -> &mut Word {
        let start = self.pos(self.i);
        self.cur.get_or_insert_with(|| Word {
            text: String::new(),
            span: start..start,
            dynamic: false,
        })
    }

    fn push(&mut self, c: char) {
        self.word().text.push(c);
    }

    fn raw(&mut self, from: usize, to: usize) {
        let src = self.src;
        let slice = &src[self.pos(from)..self.pos(to)];
        let word = self.word();
        word.text.push_str(slice);
        word.dynamic = true;
    }

    fn end_word(&mut self) {
        let Some(mut word) = self.cur.take() else {
            return;
        };
        word.span.end = self.pos(self.i);
        if self.redirect {
            self.redirect = false;
            if std::mem::take(&mut self.heredoc_target) {
                self.heredocs.push(word.text);
            }
            return;
        }
        if word.text == "esac" && !word.dynamic && after_reserved(&self.words).is_empty() {
            // The reserved word that closes a `case`; it runs nothing.
            self.case_depth = self.case_depth.saturating_sub(1);
            return;
        }
        self.words.push(word);
        if self.is_case_header() {
            // `case WORD in`: what follows is the pattern list of the first item, not a command.
            self.end_segment();
            self.case_depth += 1;
            self.case_pattern = true;
        }
    }

    /// The words so far are `case WORD in`, after any leading reserved words.
    fn is_case_header(&self) -> bool {
        matches!(
            after_reserved(&self.words),
            [case, _, keyword] if case.text == "case" && !case.dynamic && keyword.text == "in" && !keyword.dynamic
        )
    }

    fn end_segment(&mut self) {
        self.end_word();
        self.redirect = false;
        self.heredoc_target = false;
        if let [keyword, name, ..] = after_reserved(&self.words)
            && keyword.text == "function"
            && !keyword.dynamic
            && !name.dynamic
        {
            self.tokens.functions.push(name.text.clone());
        }
        let mut words = std::mem::take(&mut self.words);
        let split = words.iter().take_while(|w| is_assignment_text(&w.text)).count();
        let rest = words.split_off(split);
        if !(words.is_empty() && rest.is_empty()) {
            self.tokens.segments.push(Segment {
                assignments: words,
                words: rest,
            });
        }
    }

    fn run(mut self) -> Tokens {
        let n = self.chars.len();
        while self.i < n {
            if std::mem::take(&mut self.case_pattern) {
                self.skip_case_pattern();
                continue;
            }
            let c = self.chars[self.i].1;
            match c {
                '\\' => self.backslash(),
                ' ' | '\t' | '\r' => {
                    self.end_word();
                    self.i += 1;
                }
                '(' if self.cur.is_none() && self.at(self.i + 1) == Some('(') => {
                    // `(( n++ ))` is an arithmetic command, not a subshell running `n++`.
                    self.end_segment();
                    self.i = (self.find_close(self.i + 1) + 1).min(n);
                }
                '(' => {
                    self.end_word();
                    if let Some((name, close)) = self.function_definition() {
                        self.tokens.functions.push(name);
                        self.words.clear();
                        self.i = (close + 1).min(n);
                    } else {
                        self.end_segment();
                        self.i += 1;
                    }
                }
                '\n' => {
                    self.end_segment();
                    self.i += 1;
                    self.skip_heredoc_bodies();
                }
                ';' => {
                    self.end_segment();
                    self.i += 1;
                    // `;;`, `;&` and `;;&` end a `case` item; the next item starts with its patterns.
                    if self.case_depth > 0 && matches!(self.at(self.i), Some(';' | '&')) {
                        self.i += 1;
                        if self.at(self.i) == Some('&') {
                            self.i += 1;
                        }
                        self.case_pattern = true;
                    }
                }
                ')' => {
                    self.end_segment();
                    self.i += 1;
                }
                '#' if self.cur.is_none() => {
                    while self.i < n && self.chars[self.i].1 != '\n' {
                        self.i += 1;
                    }
                }
                '\'' => self.single(),
                '"' => self.double(),
                '`' => self.backtick(),
                '$' => self.dollar(),
                '&' => match self.at(self.i + 1) {
                    Some('&') => {
                        self.end_segment();
                        self.i += 2;
                    }
                    Some('>') => {
                        self.end_word();
                        self.i += 1;
                        self.redirect_op();
                    }
                    _ => {
                        self.end_segment();
                        self.i += 1;
                    }
                },
                '|' => {
                    self.end_segment();
                    self.i += if matches!(self.at(self.i + 1), Some('|' | '&')) {
                        2
                    } else {
                        1
                    };
                }
                '<' | '>' => self.redirect_op(),
                _ => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
        self.end_segment();
        self.tokens
    }

    /// At a `(` with the words of the command read: `name ()` or `function name ()` defines the function `name`.
    /// Returns the name and the index of the `)`.
    fn function_definition(&self) -> Option<(String, usize)> {
        let mut close = self.i + 1;
        while matches!(self.at(close), Some(' ' | '\t')) {
            close += 1;
        }
        if self.at(close) != Some(')') {
            return None;
        }
        let words = match after_reserved(&self.words) {
            [keyword, rest @ ..] if keyword.text == "function" && !keyword.dynamic => rest,
            words => words,
        };
        match words {
            [name] if !name.dynamic && !name.text.is_empty() => Some((name.text.clone(), close)),
            _ => None,
        }
    }

    /// After the end of a line that opened here-documents: skips their bodies, each up to the line that holds only
    /// its delimiter (leading tabs of `<<-` and any indentation allowed). The body is input text, not commands.
    fn skip_heredoc_bodies(&mut self) {
        let src = self.src;
        let n = self.chars.len();
        for delimiter in std::mem::take(&mut self.heredocs) {
            while self.i < n {
                let start = self.pos(self.i);
                while self.i < n && self.chars[self.i].1 != '\n' {
                    self.i += 1;
                }
                let line = &src[start..self.pos(self.i)];
                self.i = (self.i + 1).min(n);
                if line.trim() == delimiter {
                    break;
                }
            }
        }
    }

    /// Skips the pattern list of a `case` item up to and including its `)`, or the `esac` that closes the construct.
    /// A line that holds no `)` is read as commands after all.
    fn skip_case_pattern(&mut self) {
        let n = self.chars.len();
        loop {
            while self.at(self.i).is_some_and(char::is_whitespace) {
                self.i += 1;
            }
            if self.at(self.i) != Some('#') {
                break;
            }
            while self.i < n && self.chars[self.i].1 != '\n' {
                self.i += 1;
            }
        }
        let start = self.i;
        let rest = &self.src[self.pos(start)..];
        if rest.starts_with("esac")
            && self
                .at(start + 4)
                .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | ')'))
        {
            self.i = start + 4;
            self.case_depth = self.case_depth.saturating_sub(1);
            return;
        }
        while let Some(c) = self.at(self.i) {
            match c {
                ')' => {
                    self.i += 1;
                    return;
                }
                '\n' => break,
                '\\' => self.i += 2,
                '\'' | '"' => {
                    self.i += 1;
                    while self.at(self.i).is_some_and(|q| q != c) {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                _ => self.i += 1,
            }
        }
        self.i = start;
    }

    fn backslash(&mut self) {
        match self.at(self.i + 1) {
            Some('\n') => self.i += 2,
            Some('\r') if self.at(self.i + 2) == Some('\n') => self.i += 3,
            Some(c) => {
                self.push(c);
                self.i += 2;
            }
            None => self.i += 1,
        }
    }

    fn single(&mut self) {
        self.word();
        self.i += 1;
        while let Some(c) = self.at(self.i) {
            self.i += 1;
            if c == '\'' {
                break;
            }
            self.push(c);
        }
    }

    fn double(&mut self) {
        self.word();
        self.i += 1;
        while let Some(c) = self.at(self.i) {
            match c {
                '"' => {
                    self.i += 1;
                    break;
                }
                '\\' => match self.at(self.i + 1) {
                    Some('\n') => self.i += 2,
                    Some(e @ ('$' | '`' | '"' | '\\')) => {
                        self.push(e);
                        self.i += 2;
                    }
                    Some(e) => {
                        self.push('\\');
                        self.push(e);
                        self.i += 2;
                    }
                    None => self.i += 1,
                },
                '$' => self.dollar(),
                '`' => self.backtick(),
                _ => {
                    self.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn find_close(&self, from: usize) -> usize {
        let n = self.chars.len();
        let mut depth = 1_usize;
        let mut j = from;
        while j < n {
            match self.chars[j].1 {
                '\\' => j += 1,
                '\'' => {
                    j += 1;
                    while j < n && self.chars[j].1 != '\'' {
                        j += 1;
                    }
                }
                '"' => {
                    j += 1;
                    while j < n && self.chars[j].1 != '"' {
                        if self.chars[j].1 == '\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                }
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return j;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        n
    }

    fn dollar(&mut self) {
        let n = self.chars.len();
        match self.at(self.i + 1) {
            Some('(') => {
                let close = self.find_close(self.i + 2);
                let inner = self.pos(self.i + 2)..self.pos(close);
                // `$(( n + 1 ))` is arithmetic expansion: its body is an expression, not a command.
                let arithmetic = self.at(self.i + 2) == Some('(');
                if inner.start < inner.end && !arithmetic {
                    self.tokens.substitutions.push(inner);
                }
                let end = (close + 1).min(n);
                self.raw(self.i, end);
                self.i = end;
            }
            Some('{') => {
                let mut j = self.i + 2;
                while j < n && self.chars[j].1 != '}' {
                    j += 1;
                }
                let end = (j + 1).min(n);
                self.raw(self.i, end);
                self.i = end;
            }
            Some(c) if c.is_alphanumeric() || matches!(c, '_' | '@' | '*' | '#' | '?' | '!' | '$') => {
                self.word().dynamic = true;
                self.push('$');
                self.i += 1;
            }
            _ => {
                self.push('$');
                self.i += 1;
            }
        }
    }

    fn backtick(&mut self) {
        let n = self.chars.len();
        let mut j = self.i + 1;
        while j < n {
            match self.chars[j].1 {
                '\\' => j += 2,
                '`' => break,
                _ => j += 1,
            }
        }
        let close = j.min(n);
        let inner = self.pos(self.i + 1)..self.pos(close);
        if inner.start < inner.end {
            self.tokens.substitutions.push(inner);
        }
        let end = (close + 1).min(n);
        self.raw(self.i, end);
        self.i = end;
    }

    /// Consumes a redirection operator; the following word is its target and is dropped.
    fn redirect_op(&mut self) {
        let fd_prefix = self
            .cur
            .as_ref()
            .is_some_and(|w| !w.text.is_empty() && w.text.chars().all(|c| c.is_ascii_digit()));
        if fd_prefix {
            self.cur = None;
        } else {
            self.end_word();
        }
        let c = self.chars[self.i].1;
        self.i += 1;
        let mut heredoc = false;
        if self.at(self.i) == Some(c) {
            self.i += 1;
            if c == '<' && self.at(self.i) == Some('<') {
                // `<<<` is a here-string: its word is the input.
                self.i += 1;
            } else if c == '<' {
                heredoc = true;
                if self.at(self.i) == Some('-') {
                    self.i += 1;
                }
            }
        }
        if matches!(self.at(self.i), Some('&' | '|')) {
            self.i += 1;
        }
        self.redirect = true;
        self.heredoc_target = heredoc;
    }
}
