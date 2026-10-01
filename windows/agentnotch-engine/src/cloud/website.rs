//! The website's address (CL§3): the one this build syncs with comes from
//! `app-config.json` at the repository root (the glue compiles it in), a
//! development run may override it with `AGENTNOTCH_WEB_URL`, and every
//! address is kept in one canonical spelling, because string equality
//! decides "the same website" (a session is bound to the website it was
//! made through).
//!
//! Only https, or plain http to this PC (`localhost`, `127.0.0.1`, `[::1]`)
//! for development. The canonical string is built by hand: `url::Url` would
//! add a `/` to an empty path and drop a port it thinks is the default.

use serde_json::Value;

const LOCAL_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "::1", "[::1]"];

/// The pieces of `scheme://authority/path?query#fragment`, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Parts<'a> {
    scheme: &'a str,
    authority: &'a str,
    path: &'a str,
    has_query: bool,
    has_fragment: bool,
}

fn split(text: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = text.split_once("://")?;
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let after = &rest[end..];
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    Some(Parts {
        scheme,
        authority,
        path: &after[..path_end],
        has_query: after.contains('?'),
        has_fragment: after.contains('#'),
    })
}

/// The host (lowercased; an IPv6 literal keeps its brackets) and the port
/// as written, when the authority is one the app takes: no user or
/// password, a host, a numeric port if any.
fn host_and_port(authority: &str) -> Option<(String, Option<&str>)> {
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let close = rest.find(']')?;
        let host = &authority[..close + 2];
        let after = &rest[close + 1..];
        match after.strip_prefix(':') {
            Some(port) => (host, Some(port)),
            None if after.is_empty() => (host, None),
            None => return None,
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if let Some(port) = port {
        if port.is_empty()
            || !port.chars().all(|c| c.is_ascii_digit())
            || port.parse::<u16>().is_err()
        {
            return None;
        }
    }
    Some((host.to_lowercase(), port))
}

fn is_local(host: &str) -> bool {
    LOCAL_HOSTS.contains(&host)
}

/// The address as the app keeps it: `https://host[:port][/path]` with no
/// trailing slash, query, fragment or credentials; or `http://` to this PC.
/// `None` for anything else. A bare host gets `https://`.
pub fn validated(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    let owned;
    let text = if text.contains("://") {
        text
    } else {
        owned = format!("https://{text}");
        &owned
    };
    if url::Url::parse(text).is_err() {
        return None;
    }
    let parts = split(text)?;
    if parts.has_query || parts.has_fragment {
        return None;
    }
    let scheme = parts.scheme.to_lowercase();
    let (host, port) = host_and_port(parts.authority)?;
    match scheme.as_str() {
        "https" => {}
        "http" if is_local(&host) => {}
        _ => return None,
    }
    if parts.path.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let path = parts.path.trim_end_matches('/');
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{scheme}://{host}{port}{path}"))
}

/// A URL the website hands back (its dashboard, Supabase's address): the
/// same scheme and host rule, with the rest kept as given.
pub fn validated_link(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() || url::Url::parse(text).is_err() {
        return None;
    }
    let parts = split(text)?;
    let (host, _) = host_and_port(parts.authority)?;
    match parts.scheme.to_lowercase().as_str() {
        "https" => Some(text.to_owned()),
        "http" if is_local(&host) => Some(text.to_owned()),
        _ => None,
    }
}

/// `<website>/<path>` (a website is kept without a trailing slash).
pub fn endpoint(website: &str, path: &str) -> String {
    format!(
        "{}/{}",
        website.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// The website a run syncs with and whether `AGENTNOTCH_WEB_URL` set it:
/// the override when it is one the app accepts, else the build's, else
/// none. A sealed run has none (and never reads the override).
pub fn effective(
    build: Option<&str>,
    override_url: Option<&str>,
    sealed: bool,
) -> (Option<String>, bool) {
    if sealed {
        return (None, false);
    }
    if let Some(url) = validated(override_url) {
        return (Some(url), true);
    }
    (validated(build), false)
}

/// The sync website `app-config.json` names, checked by the rules the Mac
/// build applies before it builds (`Scripts/spm-build-app.sh`): a JSON
/// object with a `websiteURL` string, `""` for a build with no website,
/// otherwise an address written exactly as [`validated`] keeps it. `None`
/// for no website or a file that breaks a rule ([`check_app_config`] says
/// which).
pub fn from_app_config(text: &str) -> Option<String> {
    check_app_config(text).ok().flatten()
}

/// [`from_app_config`] with the reason a file is refused.
pub fn check_app_config(text: &str) -> Result<Option<String>, String> {
    let config: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let Some(url) = config.get("websiteURL").and_then(Value::as_str) else {
        return Err(
            "must be a JSON object with a \"websiteURL\" string (\"\" for an app with no website)"
                .into(),
        );
    };
    if url.is_empty() {
        return Ok(None);
    }
    let refuse = |why: &str| Err(format!("websiteURL \"{url}\" {why}"));
    if url != url.trim() || url.chars().any(char::is_whitespace) {
        return refuse("has spaces in it");
    }
    let Some(parts) = split(url) else {
        return refuse("is not a URL");
    };
    let scheme = parts.scheme.to_lowercase();
    if scheme != "https" && scheme != "http" {
        return refuse("must start with https://");
    }
    let netloc = parts.authority;
    if netloc.contains('@') {
        return refuse("must not carry a user name or password");
    }
    if netloc.ends_with(':') {
        return refuse("has a colon with no port after it");
    }
    if url.contains('?') || url.contains('#') {
        return refuse("must not have a query or fragment");
    }
    let Some((host, _)) = host_and_port(netloc) else {
        return refuse("is not a URL");
    };
    let bare_host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let local = matches!(bare_host.as_str(), "localhost" | "127.0.0.1" | "::1");
    if scheme == "http" && !local {
        return refuse("is http:// to another computer");
    }
    if bare_host != "::1" && !is_host_name(&bare_host) {
        return refuse("has no usable host");
    }
    if !is_kept_path(parts.path) {
        return refuse(
            "has a path the app would change (a trailing slash, an empty or odd segment)",
        );
    }
    let kept = format!("{scheme}://{}{}", netloc.to_lowercase(), parts.path);
    if url != kept {
        return refuse(&format!("is not written as the app keeps it (\"{kept}\")"));
    }
    if validated(Some(url)).as_deref() != Some(url) {
        return refuse("is not written as the app keeps it");
    }
    Ok(Some(url.to_owned()))
}

/// `label(.label)*`, each `[a-z0-9](?:[a-z0-9-]*[a-z0-9])?`.
fn is_host_name(host: &str) -> bool {
    host.split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes
                .iter()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || *b == b'-')
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
    })
}

/// `(?:/[A-Za-z0-9._~!$&*+,;=:@%-]+)*`.
fn is_kept_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    rest.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._~!$&*+,;=:@%-".contains(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_pc() {
        let v = |s: &str| validated(Some(s));
        assert_eq!(
            v("https://agentnotch.example.com/").as_deref(),
            Some("https://agentnotch.example.com")
        );
        assert_eq!(
            v("agentnotch.example.com").as_deref(),
            Some("https://agentnotch.example.com")
        );
        assert_eq!(
            v("HTTPS://Example.com/app/").as_deref(),
            Some("https://example.com/app")
        );
        assert_eq!(
            v("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            v("http://127.0.0.1:3000").as_deref(),
            Some("http://127.0.0.1:3000")
        );
        assert_eq!(v("http://[::1]:3000").as_deref(), Some("http://[::1]:3000"));
        assert_eq!(
            v("https://example.com:443").as_deref(),
            Some("https://example.com:443")
        );
        assert_eq!(
            v(" https://AgentNotch.example.com/ ").as_deref(),
            Some("https://agentnotch.example.com")
        );
        for bad in [
            "http://example.com",
            "ftp://example.com",
            "https://example.com/?next=x",
            "https://example.com/#x",
            "https://user:pw@example.com",
            "https://example.com:",
            "   ",
        ] {
            assert_eq!(v(bad), None, "{bad}");
        }
        assert_eq!(validated(None), None);
        assert_eq!(
            endpoint(
                "https://example.com/app",
                super::super::contract::path::SYNC
            ),
            "https://example.com/app/api/app/v1/sync"
        );
    }

    #[test]
    fn links_keep_their_path() {
        let v = |s: &str| validated_link(Some(s));
        assert_eq!(
            v("https://agentnotch.example.com/dashboard").as_deref(),
            Some("https://agentnotch.example.com/dashboard")
        );
        assert_eq!(
            v("http://localhost:54321").as_deref(),
            Some("http://localhost:54321")
        );
        assert_eq!(v("http://evil.example.com"), None);
        assert_eq!(v("https://u:p@example.com"), None);
        assert_eq!(v("not a url"), None);
    }

    #[test]
    fn the_build_rules() {
        let config = |url: &str| format!(r#"{{"websiteURL": "{url}"}}"#);
        assert_eq!(
            from_app_config(&config("https://agentnotch.rivant.in")).as_deref(),
            Some("https://agentnotch.rivant.in")
        );
        assert_eq!(
            from_app_config(&config("http://localhost:3000")).as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(check_app_config(&config("")), Ok(None));
        for bad in [
            "https://agentnotch.rivant.in/",
            "HTTPS://agentnotch.rivant.in",
            "https://Agentnotch.rivant.in",
            "http://agentnotch.rivant.in",
            "https://agentnotch.rivant.in?x=1",
            "https://agentnotch.rivant.in:",
            "https://a b.example",
            "https://-bad.example",
            "https://example.com//x",
        ] {
            assert!(check_app_config(&config(bad)).is_err(), "{bad}");
        }
        assert!(check_app_config("{}").is_err());
        assert!(check_app_config("[1]").is_err());
        assert!(check_app_config("{\"websiteURL\": 3}").is_err());
    }
}
