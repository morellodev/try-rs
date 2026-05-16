//! Parsing of git clone URIs.
//!
//! Supported forms (mirroring upstream `tobi/try`):
//!
//! - `https://github.com/<user>/<repo>[.git]`
//! - `http://<host>/<user>/<repo>[.git]`
//! - `git@<host>:<user>/<repo>[.git]`
//!
//! Trailing path segments and query strings after the repo name are ignored.
//! Anything else returns `None`.

/// Components extracted from a git URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitUri {
    pub host: String,
    pub user: String,
    pub repo: String,
}

/// Parse a git clone URI. Returns `None` for unrecognized forms.
///
/// # Examples
///
/// ```
/// use try_rs::git::parse;
///
/// let u = parse("https://github.com/tobi/try.git").unwrap();
/// assert_eq!(u.host, "github.com");
/// assert_eq!(u.user, "tobi");
/// assert_eq!(u.repo, "try");
///
/// let u = parse("git@github.com:tobi/try.git").unwrap();
/// assert_eq!(u.host, "github.com");
/// assert_eq!(u.user, "tobi");
/// assert_eq!(u.repo, "try");
///
/// let u = parse("https://gitlab.com/group/proj").unwrap();
/// assert_eq!(u.host, "gitlab.com");
///
/// assert!(parse("not-a-uri").is_none());
/// ```
#[must_use]
pub fn parse(uri: &str) -> Option<GitUri> {
    let trimmed = uri.strip_suffix(".git").unwrap_or(uri);

    if let Some(rest) = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        && let Some((host, path)) = rest.split_once('/')
        && let Some((user, repo_rest)) = path.split_once('/')
        && let Some(repo) = first_segment(repo_rest)
    {
        return validate(host, user, repo);
    }

    if let Some(rest) = trimmed.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
        && let Some((user, repo_rest)) = path.split_once('/')
        && let Some(repo) = first_segment(repo_rest)
    {
        return validate(host, user, repo);
    }

    None
}

/// Heuristic to detect arguments that look like a git URI. Used by the
/// `try <url>` shorthand to route into the clone flow.
///
/// # Examples
///
/// ```
/// use try_rs::git::looks_like_uri;
///
/// assert!(looks_like_uri("https://github.com/x/y"));
/// assert!(looks_like_uri("git@host.com:x/y.git"));
/// assert!(looks_like_uri("anything.git"));
/// assert!(!looks_like_uri("redis"));
/// ```
#[must_use]
pub fn looks_like_uri(arg: &str) -> bool {
    arg.starts_with("https://")
        || arg.starts_with("http://")
        || arg.starts_with("git@")
        || arg.contains("github.com")
        || arg.contains("gitlab.com")
        || arg.ends_with(".git")
}

/// Build the dated directory name for a clone (`YYYY-MM-DD-<user>-<repo>`).
///
/// # Examples
///
/// ```
/// use try_rs::git::{clone_dir_name, GitUri};
///
/// let u = GitUri { host: "github.com".into(), user: "tobi".into(), repo: "try".into() };
/// assert_eq!(clone_dir_name(&u, "2025-08-27"), "2025-08-27-tobi-try");
/// ```
#[must_use]
pub fn clone_dir_name(uri: &GitUri, date: &str) -> String {
    format!("{date}-{}-{}", uri.user, uri.repo)
}

fn first_segment(s: &str) -> Option<&str> {
    let end = s.find(['/', '?', '#']).unwrap_or(s.len());
    let head = &s[..end];
    if head.is_empty() { None } else { Some(head) }
}

fn validate(host: &str, user: &str, repo: &str) -> Option<GitUri> {
    if host.is_empty() || user.is_empty() || repo.is_empty() {
        return None;
    }
    Some(GitUri {
        host: host.to_string(),
        user: user.to_string(),
        repo: repo.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_dot_git_suffix() {
        assert_eq!(parse("https://github.com/a/b.git").unwrap().repo, "b");
        assert_eq!(parse("git@github.com:a/b.git").unwrap().repo, "b");
    }

    #[test]
    fn ignores_trailing_segments_and_query() {
        let u = parse("https://github.com/a/b/tree/main").unwrap();
        assert_eq!(u.repo, "b");
        let u = parse("https://github.com/a/b?ref=main").unwrap();
        assert_eq!(u.repo, "b");
    }

    #[test]
    fn rejects_unknown_schemes() {
        assert!(parse("ftp://example.com/a/b").is_none());
        assert!(parse("").is_none());
        assert!(parse("git@host-no-colon").is_none());
    }
}
