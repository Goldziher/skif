//! Which registry a package is fetched from, and with which token, as npm's configuration says (ADR 0058).
//!
//! Read: `registry`, `@scope:registry` and `//host/path/:_authToken`, from `npm_config_registry` in the environment,
//! the project's `.npmrc` and the user's `.npmrc` (`NPM_CONFIG_USERCONFIG`, else `~/.npmrc`), in that order of
//! precedence. `${VAR}` in a value of the user's `.npmrc` is replaced from the environment. Nothing else in `.npmrc` is
//! read: no `_auth`,
//! username or password, client certificate or `strict-ssl`. A token is only ever sent in the `Authorization` header
//! of a request to the registry it is configured for, and never printed.
//!
//! The project's `.npmrc` comes with the repository, so whoever writes it chooses the hosts and URLs. A `${VAR}` in
//! one of its values would send that CI secret wherever the file says: in a registry URL's path or host name (and so
//! to DNS), or as the token of a host it picks. So the project's file expands `${VAR}` only in the `_authToken` of a
//! host the user already trusts (`registry.npmjs.org`, or a host of the user's `.npmrc` or of `npm_config_registry`),
//! and only when the file does not point that host at plain `http://`. Any other line of it that reads a variable is
//! skipped with a warning naming its key or host, never its value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif::update::is_valid_package_name;

/// The public npm registry.
pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org/";

/// The host of [`DEFAULT_REGISTRY`].
const DEFAULT_HOST: &str = "registry.npmjs.org";

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

/// A registry URL with a trailing slash and without credentials, a query or a fragment in it: the package name is
/// appended to it, so anything after the path would swallow the name.
fn normalized(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.split(['?', '#']).next().unwrap_or(url);
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

/// The `host[:port]` of a registry URL or of a `//host[:port]/path/` nerf dart, lowercased.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("//").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().filter(|host| !host.is_empty())?;
    Some(host.to_ascii_lowercase())
}

impl Npmrc {
    /// Reads the settings of one `.npmrc` text that the user wrote: every `${VAR}` is expanded.
    pub fn parse(text: &str, var: &dyn Fn(&str) -> Option<String>) -> Self {
        Self::parse_with(text, var, None, &mut Vec::new())
    }

    /// Reads the settings of one `.npmrc` text. With `trusted` (the project's file), no `${VAR}` is expanded except
    /// in the `_authToken` of a host `trusted` accepts, and only when the file does not point that host at plain
    /// `http://`; every other line that reads a variable is skipped with a warning in `warnings` that names its key
    /// or host, never the value.
    fn parse_with(
        text: &str,
        var: &dyn Fn(&str) -> Option<String>,
        trusted: Option<&dyn Fn(&str) -> bool>,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut npmrc = Self::default();
        let mut tokens = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            let Some((key, raw)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let raw = unquoted(raw);
            if let Some(dart) = key.strip_suffix(":_authToken").filter(|dart| dart.starts_with("//")) {
                // Tokens wait for the registries, which may come later in the file.
                tokens.push((dart, raw));
                continue;
            }
            if trusted.is_some() && raw.contains("${") {
                warnings.push(format!(
                    "ignoring `{key}` in the project .npmrc: it reads an environment variable, which only the user \
                     .npmrc and the _authToken of a trusted host may do"
                ));
                continue;
            }
            let Some(value) = expand(raw, var) else {
                continue;
            };
            if key == "registry" {
                npmrc.registry = normalized(&value);
            } else if let Some(scope) = key.strip_suffix(":registry").filter(|scope| scope.starts_with('@'))
                && let Some(url) = normalized(&value)
            {
                npmrc.scopes.insert(scope.to_owned(), url);
            }
        }
        // The hosts this file sends to over plain http: a token read from the environment never goes there.
        let plain: std::collections::BTreeSet<String> = npmrc
            .registry
            .iter()
            .chain(npmrc.scopes.values())
            .filter(|url| url.starts_with("http://"))
            .filter_map(|url| host_of(url))
            .collect();
        for (dart, raw) in tokens {
            let mut dart = dart.to_owned();
            if !dart.ends_with('/') {
                dart.push('/');
            }
            let host = host_of(&dart).unwrap_or_default();
            if let Some(trusted) = trusted
                && raw.contains("${")
            {
                if !trusted(&host) {
                    warnings.push(format!(
                        "ignoring the _authToken for {host} in the project .npmrc: it reads an environment variable, \
                         and {host} is not {DEFAULT_HOST} or a host of the user .npmrc or npm_config_registry"
                    ));
                    continue;
                }
                if plain.contains(&host) {
                    warnings.push(format!(
                        "ignoring the _authToken for {host} in the project .npmrc: it reads an environment variable, \
                         and the project .npmrc sets an http:// registry for {host}"
                    ));
                    continue;
                }
            }
            if let Some(value) = expand(raw, var).filter(|value| !value.is_empty()) {
                npmrc.tokens.insert(dart, value);
            }
        }
        npmrc
    }

    /// The hosts this configuration names: its registry, its scoped registries and the hosts of its tokens.
    fn hosts(&self) -> std::collections::BTreeSet<String> {
        self.registry
            .iter()
            .chain(self.scopes.values())
            .chain(self.tokens.keys())
            .filter_map(|url| host_of(url))
            .collect()
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
    /// `root/.npmrc`, then the user's `.npmrc`. An unreadable file counts as empty. The warnings name each project
    /// token that was dropped because it reads an environment variable for an untrusted host.
    pub fn load(root: &Path) -> (Self, Vec<String>) {
        let var = |name: &str| std::env::var(name).ok();
        let text = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
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
        let user = user.map_or_else(Self::default, |path| Self::parse(&text(&path), &var));
        let mut warnings = Vec::new();
        let project = Self::project(&text(&root.join(".npmrc")), &var, &environment, &user, &mut warnings);
        (environment.over(project).over(user), warnings)
    }

    /// Reads the project's `.npmrc`, trusting `${VAR}` tokens only for the default host and the hosts of
    /// `environment` and `user`.
    fn project(
        text: &str,
        var: &dyn Fn(&str) -> Option<String>,
        environment: &Self,
        user: &Self,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut trusted = environment.hosts();
        trusted.extend(user.hosts());
        trusted.insert(DEFAULT_HOST.to_owned());
        Self::parse_with(text, var, Some(&|host| trusted.contains(host)), warnings)
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
/// escaped, as npm does (`@scope%2fname`). `None` when `name` is not a valid package name, so no name can add a path
/// segment, a query or a fragment to the URL.
pub fn package_url(registry: &str, name: &str) -> Option<String> {
    is_valid_package_name(name).then(|| format!("{registry}{}", name.replacen('/', "%2f", 1)))
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
        assert_eq!(
            normalized("https://x.dev/npm?token=1#frag").as_deref(),
            Some("https://x.dev/npm/")
        );
        assert_eq!(normalized("https://x.dev#/a").as_deref(), Some("https://x.dev/"));
        assert_eq!(normalized("https://"), None);
    }

    #[test]
    fn a_project_token_reads_the_environment_only_for_a_trusted_host() {
        let text = "registry=https://evil.example/\n//evil.example/:_authToken=${TOKEN}\n//registry.npmjs.org/:_authToken=${TOKEN}\n//npm.acme.dev/:_authToken=${TOKEN}\n//mirror.dev:8443/npm/:_authToken=${TOKEN}\n//literal.dev/:_authToken=plain\n";
        let mut warnings = Vec::new();
        let none = Npmrc::project(text, &env, &Npmrc::default(), &Npmrc::default(), &mut warnings);
        assert_eq!(none.token_for("https://evil.example/"), None);
        assert_eq!(none.token_for("https://registry.npmjs.org/"), Some("s3cret"));
        assert_eq!(none.token_for("https://npm.acme.dev/"), None);
        assert_eq!(
            none.token_for("https://literal.dev/"),
            Some("plain"),
            "a literal token reads nothing"
        );
        assert_eq!(
            warnings.len(),
            3,
            "evil.example, npm.acme.dev and mirror.dev:8443: {warnings:?}"
        );
        assert!(warnings[0].contains("evil.example"), "{warnings:?}");
        assert!(warnings.iter().all(|warning| !warning.contains("s3cret")));

        let user = Npmrc::parse("@acme:registry=https://npm.acme.dev/\n", &env);
        let environment = Npmrc {
            registry: normalized("https://mirror.dev:8443/npm"),
            ..Npmrc::default()
        };
        let mut warnings = Vec::new();
        let trusted = Npmrc::project(text, &env, &environment, &user, &mut warnings);
        assert_eq!(trusted.token_for("https://npm.acme.dev/"), Some("s3cret"));
        assert_eq!(trusted.token_for("https://mirror.dev:8443/npm/"), Some("s3cret"));
        assert_eq!(trusted.token_for("https://evil.example/"), None);
        assert_eq!(warnings.len(), 1, "{warnings:?}");

        let user_tokens = Npmrc::parse("//evil.example/:_authToken=${TOKEN}\n", &env);
        assert_eq!(
            user_tokens.token_for("https://evil.example/"),
            Some("s3cret"),
            "the user's own file keeps full expansion"
        );
    }

    #[test]
    fn a_project_registry_never_reads_the_environment() {
        let text = "registry=https://evil.example/${TOKEN}\n@acme:registry=https://${TOKEN}.evil.example/\nother-key=${TOKEN}\n";
        let mut warnings = Vec::new();
        let project = Npmrc::project(text, &env, &Npmrc::default(), &Npmrc::default(), &mut warnings);
        assert_eq!(project.registry_for("react"), DEFAULT_REGISTRY);
        assert_eq!(project.registry_for("@acme/ui"), DEFAULT_REGISTRY);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        for (warning, key) in warnings.iter().zip(["registry", "@acme:registry", "other-key"]) {
            assert!(warning.contains(&format!("`{key}`")), "{warning}");
            assert!(!warning.contains("s3cret") && !warning.contains("TOKEN"), "{warning}");
        }

        let user = Npmrc::parse(text, &env);
        assert_eq!(
            user.registry_for("react"),
            "https://evil.example/s3cret/",
            "the user's own file keeps full expansion"
        );
    }

    #[test]
    fn a_project_token_from_the_environment_never_goes_over_http() {
        let text = "registry=http://registry.npmjs.org/\n//registry.npmjs.org/:_authToken=${TOKEN}\n@acme:registry=http://npm.acme.dev/\n//npm.acme.dev/:_authToken=literal\n";
        let user = Npmrc::parse("@acme:registry=https://npm.acme.dev/\n", &env);
        let mut warnings = Vec::new();
        let project = Npmrc::project(text, &env, &Npmrc::default(), &user, &mut warnings);
        assert_eq!(project.token_for("http://registry.npmjs.org/"), None);
        assert_eq!(
            project.token_for("http://npm.acme.dev/"),
            Some("literal"),
            "a literal token reads nothing"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("registry.npmjs.org") && warnings[0].contains("http://"));
        assert!(!warnings[0].contains("s3cret"));

        let mut warnings = Vec::new();
        let https = Npmrc::project(
            "//registry.npmjs.org/:_authToken=${TOKEN}\nregistry=https://registry.npmjs.org/\n",
            &env,
            &Npmrc::default(),
            &Npmrc::default(),
            &mut warnings,
        );
        assert_eq!(https.token_for("https://registry.npmjs.org/"), Some("s3cret"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_scoped_name_escapes_its_slash() {
        assert_eq!(
            package_url(DEFAULT_REGISTRY, "@types/node").as_deref(),
            Some("https://registry.npmjs.org/@types%2fnode")
        );
        assert_eq!(
            package_url(DEFAULT_REGISTRY, "react").as_deref(),
            Some("https://registry.npmjs.org/react")
        );
        assert_eq!(
            package_url(DEFAULT_REGISTRY, "JSONStream").as_deref(),
            Some("https://registry.npmjs.org/JSONStream"),
            "a legacy name with capitals"
        );
        for bad in [
            "a?x",
            "a#b",
            "a/../b",
            "..",
            "@scope/..",
            "@a/b/c",
            "a b",
            "",
            "@scope",
            ".hidden",
            "_x",
            "a%2fb",
        ] {
            assert_eq!(package_url(DEFAULT_REGISTRY, bad), None, "{bad}");
        }
    }
}
