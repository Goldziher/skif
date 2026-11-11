//! Minimal ANSI styling that is a no-op when colour is disabled.

use std::fmt;

use skif_core::Severity;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Painter {
    enabled: bool,
}

/// `text` wrapped in the escape codes of a style when colour is on, written straight to the formatter.
pub(crate) struct Painted<T> {
    codes: Option<&'static str>,
    text: T,
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.codes {
            Some(codes) => write!(f, "\x1b[{codes}m{}\x1b[0m", self.text),
            None => write!(f, "{}", self.text),
        }
    }
}

impl Painter {
    pub(crate) const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn paint<T: fmt::Display>(self, codes: &'static str, text: T) -> Painted<T> {
        Painted {
            codes: self.enabled.then_some(codes),
            text,
        }
    }

    pub(crate) fn severity<T: fmt::Display>(self, severity: Severity, text: T) -> Painted<T> {
        let codes = match severity {
            Severity::Error => "1;31",
            Severity::Warning => "1;33",
            _ => "1;36",
        };
        self.paint(codes, text)
    }

    pub(crate) fn bold<T: fmt::Display>(self, text: T) -> Painted<T> {
        self.paint("1", text)
    }

    pub(crate) fn accent<T: fmt::Display>(self, text: T) -> Painted<T> {
        self.paint("1;34", text)
    }

    pub(crate) fn dim<T: fmt::Display>(self, text: T) -> Painted<T> {
        self.paint("2", text)
    }

    pub(crate) fn success<T: fmt::Display>(self, text: T) -> Painted<T> {
        self.paint("1;32", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_only_when_enabled() {
        let on = Painter::new(true);
        let off = Painter::new(false);
        assert_eq!(on.bold("x").to_string(), "\x1b[1mx\x1b[0m");
        assert_eq!(on.severity(Severity::Error, 7).to_string(), "\x1b[1;31m7\x1b[0m");
        assert_eq!(on.severity(Severity::Warning, "w").to_string(), "\x1b[1;33mw\x1b[0m");
        assert_eq!(on.severity(Severity::Info, "i").to_string(), "\x1b[1;36mi\x1b[0m");
        assert_eq!(on.accent("a").to_string(), "\x1b[1;34ma\x1b[0m");
        assert_eq!(on.dim("d").to_string(), "\x1b[2md\x1b[0m");
        assert_eq!(on.success("s").to_string(), "\x1b[1;32ms\x1b[0m");
        assert_eq!(off.bold("x").to_string(), "x");
        assert_eq!(off.severity(Severity::Error, "e").to_string(), "e");
    }
}
