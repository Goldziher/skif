//! A small POSIX-ish tokenizer: quotes, escapes, separators, redirections and substitutions.

use std::ops::Range;

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
    }
    .run()
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
        if let Some(mut word) = self.cur.take() {
            word.span.end = self.pos(self.i);
            if self.redirect {
                self.redirect = false;
            } else {
                self.words.push(word);
            }
        }
    }

    fn end_segment(&mut self) {
        self.end_word();
        self.redirect = false;
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
                '\n' | ';' | '(' | ')' => {
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
        if self.at(self.i) == Some(c) {
            self.i += 1;
            if c == '<' && self.at(self.i) == Some('<') {
                self.i += 1;
            }
        }
        if matches!(self.at(self.i), Some('&' | '|')) {
            self.i += 1;
        }
        self.redirect = true;
    }
}
