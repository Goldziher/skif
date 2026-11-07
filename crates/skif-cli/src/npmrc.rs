//! Which registry a package is fetched from, and with which token, as npm's configuration says (ADR 0050).
//!
//! Read: `registry`, `@scope:registry` and `//host/path/:_authToken`, from `npm_config_registry` in the environment,
//! the project's `.npmrc` and the user's `.npmrc` (`NPM_CONFIG_USERCONFIG`, else `~/.npmrc`), in that order of
//! precedence. `${VAR}` in a value is replaced from the environment. Nothing else in `.npmrc` is read: no `_auth`,
//! username or password, client certificate or `strict-ssl`. A token is only ever sent in the `Authorization` header
//! of a request to the registry it is configured for, and never printed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The public npm registry.
pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org/";

/// JSR's npm-compatible registry, for `@jsr/*` packages unless `@jsr:registry` says otherwise.
const JSR_REGISTRY: &str = "https://npm.jsr.io/";

/// The registry settings of one or more `.npmrc` files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Npmrc {
    registry: Option<String>,
    scopes: BTreeMap<String, String>,
    /// Tokens by "nerf dart": the registry URL without its scheme, as `//host/path/`.
    tokens: BTreeMap<String, String>,
}

/// A registry URL with a trailing slash and without credentials in it.
fn normalized(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") || rest.is_empty() {
        return None;
    }
    // Credentials in the URL are never used or shown; a token belongs in `_authToken`.
    let rest = match rest.split_once('/') {
        Some((authority, path)) => format!("{}/{path}", authority.rsplit('@').next().unwrap_or(authority)),
        None => rest.rsplit('@').next().unwrap_or(rest).to_owned(),
    };
    let mut url = format!("{scheme}://{rest}");
    if !url.ends_with('/') {
        url.push('/');
    }
    Some(url)
}

/// `${NAME}` replaced by the variable's value; `None` when a variable is unset, so a half-expanded token is never
/// sent.
fn expand(value: &str, var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let end = rest[start..].find('}')? + start;
        out.push_str(&rest[..start]);
        out.push_str(&var(&rest[start + 2..end])?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn unquoted(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|inner| inner.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

impl Npmrc {
    /// Reads the settings of one `.npmrc` text.
    pub fn parse(text: &str, var: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut npmrc = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let Some(value) = expand(unquoted(value), var) else {
                continue;
            };
            if key == "registry" {
                npmrc.registry = normalized(&value);
            } else if let Some(scope) = key.strip_suffix(":registry").filter(|scope| scope.starts_with('@')) {
                if let Some(url) = normalized(&value) {
                    npmrc.scopes.insert(scope.to_owned(), url);
                }
            } else if let Some(dart) = key.strip_suffix(":_authToken").filter(|dart| dart.starts_with("//")) {
                let mut dart = dart.to_owned();
                if !dart.ends_with('/') {
                    dart.push('/');
                }
                if !value.is_empty() {
                    npmrc.tokens.insert(dart, value);
                }
            }
        }
        npmrc
    }

    /// Fills in what `self` does not set from `lower`, a source of lower precedence.
    #[must_use]
    pub fn over(mut self, lower: Self) -> Self {
        if self.registry.is_none() {
            self.registry = lower.registry;
        }
        for (scope, url) in lower.scopes {
            self.scopes.entry(scope).or_insert(url);
        }
        for (dart, token) in lower.tokens {
            self.tokens.entry(dart).or_insert(token);
        }
        self
    }

    /// The settings that apply in the workspace at `root`: the environment's `npm_config_registry`, then
    /// `root/.npmrc`, then the user's `.npmrc`. An unreadable file counts as empty.
    pub fn load(root: &Path) -> Self {
        let var = |name: &str| std::env::var(name).ok();
        let read = |path: &Path| {
            std::fs::read_to_string(path).map_or_else(|_| Self::default(), |text| Self::parse(&text, &var))
        };
        let environment = Self {
            registry: ["npm_config_registry", "NPM_CONFIG_REGISTRY"]
                .iter()
                .find_map(|name| var(name))
                .and_then(|url| normalized(&url)),
            ..Self::default()
        };
        let user = var("NPM_CONFIG_USERCONFIG")
            .or_else(|| var("npm_config_userconfig"))
            .map(PathBuf::from)
            .or_else(|| {
                var("HOME")
                    .or_else(|| var("USERPROFILE"))
                    .map(|home| Path::new(&home).join(".npmrc"))
            });
        let project = read(&root.join(".npmrc"));
        let user = user.map_or_else(Self::default, |path| read(&path));
        environment.over(project).over(user)
    }

    /// The registry `name` is fetched from, with a trailing slash.
    pub fn registry_for(&self, name: &str) -> String {
        let scope = name
            .split_once('/')
            .map(|(scope, _)| scope)
            .filter(|scope| scope.starts_with('@'));
        if let Some(url) = scope.and_then(|scope| self.scopes.get(scope)) {
            return url.clone();
        }
        if scope == Some("@jsr") {
            return JSR_REGISTRY.to_owned();
        }
        self.registry.clone().unwrap_or_else(|| DEFAULT_REGISTRY.to_owned())
    }

    /// The token configured for `registry`: the one for the longest `//host/path/` that prefixes it.
    pub fn token_for(&self, registry: &str) -> Option<&str> {
        let (_, rest) = registry.split_once("://")?;
        let mut dart = format!("//{rest}");
        loop {
            if let Some(token) = self.tokens.get(&dart) {
                return Some(token);
            }
            let trimmed = dart.trim_end_matches('/');
            let cut = trimmed.rfind('/')?;
            if cut < 2 {
                return None;
            }
            dart = format!("{}/", &trimmed[..cut]);
        }
    }
}

/// The URL of the registry document of `name` under `registry` (which ends in `/`): the `/` of a scoped name is
/// escaped, as npm does.
pub fn package_url(registry: &str, name: &str) -> String {
    format!("{registry}{}", name.replacen('/', "%2f", 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        (name == "TOKEN").then(|| "s3cret".to_owned())
    }

    #[test]
    fn scoped_registries_tokens_and_the_default_are_read() {
        let npmrc = Npmrc::parse(
            "# comment\n; also a comment\nregistry = https://mirror.example.com/npm\n@acme:registry=\"https://npm.acme.dev/\"\n//npm.acme.dev/:_authToken=${TOKEN}\n//other.dev/:_authToken=${MISSING}\nstrict-ssl=false\n",
            &env,
        );
        assert_eq!(npmrc.registry_for("react"), "https://mirror.example.com/npm/");
        assert_eq!(npmrc.registry_for("@acme/ui"), "https://npm.acme.dev/");
        assert_eq!(npmrc.registry_for("@jsr/std__path"), "https://npm.jsr.io/");
        assert_eq!(npmrc.token_for("https://npm.acme.dev/"), Some("s3cret"));
        assert_eq!(npmrc.token_for("https://npm.acme.dev/sub/path/"), Some("s3cret"));
        assert_eq!(
            npmrc.token_for("https://other.dev/"),
            None,
            "an unset variable sends no token"
        );
        assert_eq!(npmrc.token_for("https://mirror.example.com/npm/"), None);
        assert_eq!(Npmrc::default().registry_for("react"), DEFAULT_REGISTRY);
    }

    #[test]
    fn a_higher_source_wins_key_by_key() {
        let project = Npmrc::parse("@a:registry=https://a.dev\n", &env);
        let user = Npmrc::parse("registry=https://user.dev\n@a:registry=https://ignored.dev\n", &env);
        let merged = project.over(user);
        assert_eq!(merged.registry_for("@a/x"), "https://a.dev/");
        assert_eq!(merged.registry_for("x"), "https://user.dev/");
    }

    #[test]
    fn credentials_in_a_registry_url_are_dropped() {
        let npmrc = Npmrc::parse("registry=https://user:pass@private.dev/npm/\n", &env);
        assert_eq!(npmrc.registry_for("x"), "https://private.dev/npm/");
        assert_eq!(normalized("ftp://x.dev"), None);
        assert_eq!(normalized("https://"), None);
    }

    #[test]
    fn a_scoped_name_escapes_its_slash() {
        assert_eq!(
            package_url(DEFAULT_REGISTRY, "@types/node"),
            "https://registry.npmjs.org/@types%2fnode"
        );
        assert_eq!(
            package_url(DEFAULT_REGISTRY, "react"),
            "https://registry.npmjs.org/react"
        );
    }
}
