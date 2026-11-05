// Vendored from syncpack crates/syncpack-specifier/src/parser.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
/// Skip consecutive ASCII digits from `pos`, returning position after last digit.
/// Returns None if no digits at `pos`.
fn skip_digits(bytes: &[u8], pos: usize) -> Option<usize> {
    let start = pos;
    let mut i = pos;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == start { None } else { Some(i) }
}

/// Check if bytes from `pos` match a tag suffix: -[a-z0-9._-]+$
fn has_tag_suffix(bytes: &[u8], pos: usize) -> bool {
    pos < bytes.len()
        && bytes[pos] == b'-'
        && bytes.len() > pos + 1
        && bytes[pos + 1..]
            .iter()
            .all(|&b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
}

/// Check if bytes from `pos` match digits.digits.digits (optionally with -tag)
fn is_semver_triple(bytes: &[u8], pos: usize) -> bool {
    let Some(p1) = skip_digits(bytes, pos) else {
        return false;
    };
    if p1 >= bytes.len() || bytes[p1] != b'.' {
        return false;
    }
    let Some(p2) = skip_digits(bytes, p1 + 1) else {
        return false;
    };
    if p2 >= bytes.len() || bytes[p2] != b'.' {
        return false;
    }
    let Some(p3) = skip_digits(bytes, p2 + 1) else {
        return false;
    };
    p3 == bytes.len() || has_tag_suffix(bytes, p3)
}

/// Check if bytes from `pos` match digits.digits$
fn is_minor_version(bytes: &[u8], pos: usize) -> bool {
    let Some(p1) = skip_digits(bytes, pos) else {
        return false;
    };
    if p1 >= bytes.len() || bytes[p1] != b'.' {
        return false;
    }
    let Some(p2) = skip_digits(bytes, p1 + 1) else {
        return false;
    };
    p2 == bytes.len()
}

/// Check if bytes from `pos` match digits$
fn is_major_version(bytes: &[u8], pos: usize) -> bool {
    let Some(p1) = skip_digits(bytes, pos) else {
        return false;
    };
    p1 == bytes.len()
}

/// Get the byte offset after a range prefix (^, ~, >, >=, <, <=).
/// Returns None if no range prefix found.
fn range_prefix_len(bytes: &[u8]) -> Option<usize> {
    match bytes.first() {
        Some(b'^' | b'~') => Some(1),
        Some(b'>' | b'<') => {
            if bytes.get(1) == Some(&b'=') {
                Some(2)
            } else {
                Some(1)
            }
        }
        _ => None,
    }
}

/// Whether `str` is any of the plain forms: exact, `latest`, major, minor, or a range over one of them.
#[must_use]
pub fn is_simple_semver(str: &str) -> bool {
    is_exact(str)
        || is_latest(str)
        || is_major(str)
        || is_minor(str)
        || is_range(str)
        || is_range_major(str)
        || is_range_minor(str)
}

/// "1.2.3", "1.2.3-beta.1", "=1.2.3", "=1.2.3-beta.1"
#[must_use]
pub fn is_exact(s: &str) -> bool {
    let bytes = s.as_bytes();
    let start = usize::from(bytes.first() == Some(&b'='));
    is_semver_triple(bytes, start)
}

/// `*`, `latest` or `x`.
#[must_use]
pub fn is_latest(str: &str) -> bool {
    str == "*" || str == "latest" || str == "x"
}

/// "1"
#[must_use]
pub fn is_major(s: &str) -> bool {
    is_major_version(s.as_bytes(), 0)
}

/// "1.2"
#[must_use]
pub fn is_minor(s: &str) -> bool {
    is_minor_version(s.as_bytes(), 0)
}

/// "^1.2.3", "~1.2.3", ">1.2.3", ">=1.2.3", etc. (with optional -tag)
#[must_use]
pub fn is_range(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(start) = range_prefix_len(bytes) else {
        return false;
    };
    is_semver_triple(bytes, start)
}

/// "^1", "~1", ">1", ">=1", etc.
#[must_use]
pub fn is_range_major(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(start) = range_prefix_len(bytes) else {
        return false;
    };
    is_major_version(bytes, start)
}

/// "^1.2", "~1.2", ">1.2", ">=1.2", etc.
#[must_use]
pub fn is_range_minor(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(start) = range_prefix_len(bytes) else {
        return false;
    };
    is_minor_version(bytes, start)
}

/// "1.x", "1.2.*", "1.X.x": a partial version whose trailing components are
/// wildcards. Only meaningful as a part of a complex range; on its own it is
/// classified as unsupported.
#[must_use]
pub fn is_x_range(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() > 3 || parts[0].is_empty() || !parts[0].bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let is_wildcard = |p: &str| matches!(p, "x" | "X" | "*");
    let is_number = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let first_wildcard = parts.iter().position(|p| is_wildcard(p));
    match first_wildcard {
        None => false,
        Some(index) => parts[..index].iter().all(|p| is_number(p)) && parts[index..].iter().all(|p| is_wildcard(p)),
    }
}

/// Is this a semver range containing multiple parts?
/// Such as OR ("||"), AND (" ") or hyphen range (" - ")
#[must_use]
pub fn is_complex_range(specifier: &str) -> bool {
    // Quick reject: must contain a space or an OR operator
    if !specifier.contains(' ') && !specifier.contains("||") {
        return false;
    }
    // Split on "||" first (OR conditions), then each part is whitespace-separated
    // AND conditions; a lone "-" is the hyphen range operator.
    let mut total_parts = 0;
    for or_part in specifier.split("||") {
        let mut tokens = 0;
        for token in or_part.split_whitespace().filter(|token| *token != "-") {
            if !is_simple_semver(token) && !is_x_range(token) {
                return false;
            }
            tokens += 1;
        }
        if tokens == 0 {
            return false;
        }
        total_parts += tokens;
    }
    total_parts > 1
}

/// "alpha", "beta", etc.
#[must_use]
pub fn is_tag(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// git+ssh://, git://, github:, git@, git+https://
#[must_use]
pub fn is_git(s: &str) -> bool {
    s.starts_with("git://")
        || s.starts_with("github:")
        || s.starts_with("git@")
        || s.starts_with("git+https://")
        || s.starts_with("git+ssh://")
}

/// "link:../path"
#[must_use]
pub fn is_link(s: &str) -> bool {
    s.starts_with("link:")
}
