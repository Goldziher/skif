//! TypeScript declaration files, told apart from source files by their whole suffix.

/// The suffixes of TypeScript declaration files: `.d.ts`, `.d.mts` and `.d.cts`.
pub const DECLARATION_SUFFIXES: [&str; 3] = [".d.ts", ".d.mts", ".d.cts"];

/// Whether `path`, a path or a file name, names a TypeScript declaration file (`.d.ts`, `.d.mts`, `.d.cts`).
#[must_use]
pub fn is_declaration_file(path: &str) -> bool {
    DECLARATION_SUFFIXES.iter().any(|suffix| path.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_files_are_told_by_their_whole_suffix() {
        let cases = [
            ("typings/legacy.d.ts", true),
            ("typings/legacy.d.mts", true),
            ("typings/legacy.d.cts", true),
            ("legacy.d.ts", true),
            ("src/index.ts", false),
            ("src/index.mts", false),
            ("src/index.cts", false),
            ("src/build.d.tsx", false),
            ("src/d.ts", false),
        ];
        for (path, expected) in cases {
            assert_eq!(is_declaration_file(path), expected, "{path}");
        }
    }
}
