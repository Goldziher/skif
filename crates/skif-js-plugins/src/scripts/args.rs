//! Minimal command-line argument parsing driven by a list of value-taking flags.

use std::ops::Range;

use super::tokenize::Word;

#[derive(Debug, Clone)]
pub(super) struct Val {
    pub text: String,
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub(super) struct Flag {
    /// The flag name without leading dashes; short groups such as `-ws` keep all letters.
    pub name: String,
    pub value: Option<Val>,
}

#[derive(Debug, Default)]
pub(super) struct Parsed {
    pub flags: Vec<Flag>,
    /// Indexes into the parsed words.
    pub positionals: Vec<usize>,
    pub dashdash: Option<usize>,
    /// Index of the first word that was not consumed (after the stopping positional or `--`).
    pub rest: usize,
}

impl Parsed {
    pub fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|f| names.contains(&f.name.as_str()))
    }

    pub fn values<'a>(&'a self, names: &'a [&str]) -> impl Iterator<Item = &'a Val> {
        self.flags
            .iter()
            .filter(move |f| names.contains(&f.name.as_str()))
            .filter_map(|f| f.value.as_ref())
    }
}

fn val(word: &Word) -> Val {
    Val {
        text: word.text.clone(),
        span: word.span.clone(),
    }
}

/// Splits `words` into flags and positionals. `value_flags` lists the flags (without dashes, long
/// and short spellings) that consume a value. Parsing ends at `--`, and with `stop_at_positional`
/// at the first positional, whose index is then the only entry of `positionals`.
pub(super) fn parse(words: &[Word], value_flags: &[&str], stop_at_positional: bool) -> Parsed {
    let mut out = Parsed {
        rest: words.len(),
        ..Parsed::default()
    };
    let mut i = 0;
    while i < words.len() {
        let word = &words[i];
        let text = word.text.as_str();
        if text == "--" {
            out.dashdash = Some(i);
            out.rest = i + 1;
            break;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let value = match inline {
                Some(value) => Some(Val {
                    text: value.to_owned(),
                    span: word.span.clone(),
                }),
                None if value_flags.contains(&name) => {
                    i += 1;
                    words.get(i).map(val)
                }
                None => None,
            };
            out.flags.push(Flag {
                name: name.to_owned(),
                value,
            });
            i += 1;
            continue;
        }
        let short = text.strip_prefix('-').filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()));
        if let Some(short) = short {
            let first = short.chars().next().map_or(0, char::len_utf8);
            let (head, tail) = short.split_at(first);
            if tail.is_empty() {
                let value = if value_flags.contains(&head) {
                    i += 1;
                    words.get(i).map(val)
                } else {
                    None
                };
                out.flags.push(Flag {
                    name: head.to_owned(),
                    value,
                });
            } else if value_flags.contains(&head) {
                out.flags.push(Flag {
                    name: head.to_owned(),
                    value: Some(Val {
                        text: tail.trim_start_matches('=').to_owned(),
                        span: word.span.clone(),
                    }),
                });
            } else {
                out.flags.push(Flag {
                    name: short.to_owned(),
                    value: None,
                });
            }
            i += 1;
            continue;
        }
        out.positionals.push(i);
        if stop_at_positional {
            out.rest = i + 1;
            break;
        }
        i += 1;
    }
    out
}
