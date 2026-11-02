//! Module specifiers: builtins, package names and `@types` pairing.

const BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Runtimes that are provided by the environment and never listed as dependencies.
pub(crate) const RUNTIME_PACKAGES: &[&str] = &["node", "bun", "deno"];

/// Whether `specifier` names a Node.js builtin, with or without the `node:` prefix.
pub(crate) fn is_builtin(specifier: &str) -> bool {
    if specifier.starts_with("node:") {
        return true;
    }
    let root = specifier.split('/').next().unwrap_or(specifier);
    BUILTINS.contains(&root)
}

/// Whether the specifier carries a URL-like scheme that is not a module on disk (`data:`, `https:`, `virtual:`).
pub(crate) fn has_foreign_scheme(specifier: &str) -> bool {
    let Some((scheme, _)) = specifier.split_once(':') else {
        return false;
    };
    scheme.len() > 1 && scheme.bytes().all(|b| b.is_ascii_lowercase())
}

/// The package a bare specifier points into: `lodash/fp` is `lodash`, `@a/b/c` is `@a/b`.
///
/// Returns `None` for relative, absolute and `#` specifiers and for strings that cannot be a package name.
pub(crate) fn package_name(specifier: &str) -> Option<String> {
    let specifier = specifier.split(['?', '#']).next()?;
    if specifier.is_empty() || specifier.starts_with(['.', '/', '#', '~']) {
        return None;
    }
    let name = if let Some(rest) = specifier.strip_prefix('@') {
        let mut parts = rest.split('/');
        let scope = parts.next().filter(|part| !part.is_empty())?;
        let name = parts.next().filter(|part| !part.is_empty())?;
        format!("@{scope}/{name}")
    } else {
        specifier.split('/').next().filter(|part| !part.is_empty())?.to_owned()
    };
    let valid = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'/'));
    valid.then_some(name)
}

/// The part of a bare specifier after the package name, without a leading slash.
pub(crate) fn subpath<'a>(specifier: &'a str, package: &str) -> Option<&'a str> {
    let rest = specifier.strip_prefix(package)?.strip_prefix('/')?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    (!rest.is_empty()).then_some(rest)
}

/// The `DefinitelyTyped` package for `name`: `react` is `@types/react`, `@a/b` is `@types/a__b`.
/// `None` for packages that already are `DefinitelyTyped` packages.
pub(crate) fn types_package(name: &str) -> Option<String> {
    if name.starts_with("@types/") {
        return None;
    }
    Some(match name.strip_prefix('@').and_then(|rest| rest.split_once('/')) {
        Some((scope, rest)) => format!("@types/{scope}__{rest}"),
        None => format!("@types/{name}"),
    })
}

/// The package a `DefinitelyTyped` package describes: `@types/a__b` is `@a/b`.
pub(crate) fn typed_package(types: &str) -> Option<String> {
    let rest = types.strip_prefix("@types/")?;
    Some(match rest.split_once("__") {
        Some((scope, name)) => format!("@{scope}/{name}"),
        None => rest.to_owned(),
    })
}

/// The package name found in a path below `node_modules`, using the last `node_modules` component.
pub(crate) fn node_modules_package(path: &std::path::Path) -> Option<String> {
    let components: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    let position = components.iter().rposition(|c| c == "node_modules")?;
    let first = components.get(position + 1)?;
    if first.starts_with('@') {
        let second = components.get(position + 2)?;
        Some(format!("{first}/{second}"))
    } else {
        Some(first.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn package_names() {
        let cases = [
            ("lodash", Some("lodash")),
            ("lodash/fp", Some("lodash")),
            ("@scope/pkg", Some("@scope/pkg")),
            ("@scope/pkg/deep/file.js", Some("@scope/pkg")),
            ("@scope", None),
            ("@/alias/x", None),
            ("./local", None),
            ("../up", None),
            ("/abs", None),
            ("#internal", None),
            ("~/x", None),
            ("pkg?raw", Some("pkg")),
            ("", None),
        ];
        for (specifier, expected) in cases {
            assert_eq!(package_name(specifier).as_deref(), expected, "{specifier}");
        }
    }

    #[test]
    fn builtins_and_schemes() {
        assert!(is_builtin("node:fs"));
        assert!(is_builtin("fs"));
        assert!(is_builtin("fs/promises"));
        assert!(!is_builtin("fs-extra"));
        assert!(has_foreign_scheme("https://x.dev/a.js"));
        assert!(has_foreign_scheme("virtual:pwa"));
        assert!(!has_foreign_scheme("lodash"));
    }

    #[test]
    fn types_pairing() {
        assert_eq!(types_package("react").as_deref(), Some("@types/react"));
        assert_eq!(types_package("@a/b").as_deref(), Some("@types/a__b"));
        assert_eq!(types_package("@types/react"), None);
        assert_eq!(typed_package("@types/a__b").as_deref(), Some("@a/b"));
        assert_eq!(typed_package("@types/node").as_deref(), Some("node"));
        assert_eq!(typed_package("react"), None);
    }

    #[test]
    fn subpaths() {
        assert_eq!(subpath("@a/b/c/d", "@a/b"), Some("c/d"));
        assert_eq!(subpath("@a/b", "@a/b"), None);
    }

    #[test]
    fn node_modules_packages() {
        assert_eq!(
            node_modules_package(Path::new("/r/node_modules/.pnpm/x/node_modules/@a/b/index.js")).as_deref(),
            Some("@a/b")
        );
        assert_eq!(node_modules_package(Path::new("/r/src/a.ts")), None);
    }
}
