//! Minimal ANSI styling that is a no-op when colour is disabled.

use skif_core::Severity;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Painter {
    enabled: bool,
}

impl Painter {
    pub(crate) const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn paint(self, codes: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{codes}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub(crate) fn severity(self, severity: Severity, text: &str) -> String {
        let codes = match severity {
            Severity::Error => "1;31",
            Severity::Warning => "1;33",
            Severity::Info => "1;36",
        };
        self.paint(codes, text)
    }

    pub(crate) fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub(crate) fn accent(self, text: &str) -> String {
        self.paint("1;34", text)
    }

    pub(crate) fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    pub(crate) fn success(self, text: &str) -> String {
        self.paint("1;32", text)
    }
}
