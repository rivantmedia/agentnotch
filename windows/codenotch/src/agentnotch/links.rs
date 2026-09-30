//! Which URL a page may have opened (`open_url`, DESIGN-WIN §3.5).
//!
//! The string ends up in `ShellExecuteW`, which runs whatever is registered for it: a program
//! path, a `file:` or UNC target, any installed app's scheme. So the rule is a parser, not a
//! prefix test, and it lives in this one pure function: only an absolute `https://` URL with a
//! plain host gets through. It is deliberately narrower than what a browser accepts (no
//! credentials before the host, ASCII only, none of the characters RFC 3986 leaves out of a
//! URL): a quote or a space could end the argument on the command line of the browser Windows
//! starts, and a page can always percent-encode (`new URL(x).href` does).
//!
//! The one other thing that may be opened is a link the engine itself made (`cloud_url`), which
//! is `http://localhost…` in a dev run against a local website: [`verdict`] says when it is
//! worth asking the engine, and the caller opens it only when the engine's answer is that exact
//! string.

/// Longer than any link the app shows; a bound so a page can't hand the shell megabytes.
pub(super) const MAX_URL_LEN: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// An `https://` URL: open it.
    Open,
    /// Not one, but well-formed enough to be compared with the engine's own links; opened only
    /// when it equals one of them.
    IfTheEngines,
    Refused,
}

pub(super) fn verdict(url: &str) -> Verdict {
    if is_https(url) {
        Verdict::Open
    } else if !url.is_empty()
        && url.len() <= MAX_URL_LEN
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        Verdict::IfTheEngines
    } else {
        Verdict::Refused
    }
}

/// `https://host[:port][/path][?query][#fragment]`, the scheme in any case.
pub(super) fn is_https(url: &str) -> bool {
    if url.len() > MAX_URL_LEN || !url.bytes().all(url_byte) {
        return false;
    }
    // All ASCII from here on, so byte offsets are character offsets.
    const SCHEME: &str = "https://";
    let Some(scheme) = url.get(..SCHEME.len()) else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return false;
    }
    let rest = &url[SCHEME.len()..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    is_authority(authority)
}

/// A byte a URL may hold unescaped: visible ASCII without the ones RFC 3986 excludes.
fn url_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
        && !matches!(
            byte,
            b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
        )
}

/// `host` or `host:port`. No `user@` part: it is how a link pretends to go somewhere else.
fn is_authority(authority: &str) -> bool {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        // An IPv6 literal: its colons are not the port's.
        let Some((address, after)) = bracketed.split_once(']') else {
            return false;
        };
        let address_ok = !address.is_empty()
            && address
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'));
        if !address_ok {
            return false;
        }
        match after.strip_prefix(':') {
            Some(port) => (None, Some(port)),
            None if after.is_empty() => (None, None),
            None => return false,
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (Some(host), Some(port)),
            None => (Some(authority), None),
        }
    };
    let host_ok = host.is_none_or(|host| {
        !host.is_empty()
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
    });
    let port_ok = port.is_none_or(|port| {
        (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit())
    });
    host_ok && port_ok
}

#[cfg(test)]
mod tests {
    use super::{is_https, verdict, Verdict, MAX_URL_LEN};

    #[test]
    fn https_urls_with_a_host_are_opened() {
        for url in [
            "https://example.com/a?b=c",
            "HTTPS://EXAMPLE.COM",
            "hTtPs://example.com",
            "https://example.com",
            "https://example.com/",
            "https://example.com:8443/x",
            "https://sub.example-site.co.uk/a/b.html?x=1&y=%20z#frag",
            "https://example.com?x=1",
            "https://example.com#top",
            "https://127.0.0.1/x",
            "https://[::1]:3000/x",
            "https://[2001:db8::1]/",
            "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.1.0",
        ] {
            assert!(is_https(url), "{url}");
            assert_eq!(verdict(url), Verdict::Open, "{url}");
        }
        let longest = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN - 20));
        assert_eq!(longest.len(), MAX_URL_LEN);
        assert!(is_https(&longest));
    }

    #[test]
    fn everything_else_is_not_an_https_url() {
        let too_long = format!("https://example.com/{}", "a".repeat(3000 - 20));
        assert_eq!(too_long.len(), 3000);
        for url in [
            "",
            "http://example.com",
            "http://localhost:3000/dashboard",
            "file:///C:/x",
            "javascript:alert(1)",
            "ms-settings:notifications",
            "agentnotch://open",
            "\\\\server\\share",
            "C:\\Windows\\notepad.exe",
            "notepad.exe",
            "/etc/passwd",
            "example.com",
            "//example.com",
            "https://",
            "https:///path",
            "https://?x=1",
            "https://#x",
            "https:",
            "https:/example.com",
            "https:example.com",
            "https:\\\\example.com",
            "https://example.com\\@evil.example",
            "https://user@example.com",
            "https://user:pw@example.com/",
            "https://example.com:",
            "https://example.com:port",
            "https://example.com:123456",
            "https://:443",
            "https://.example.com",
            "https://example..com",
            "https://example.com.",
            "https://exa_mple.com",
            "https://exa%6dple.com",
            "https://[::1",
            "https://[]",
            "https://[::1]x",
            "https://[example.com]",
            "https://example.com/a\nb",
            "https://example.com/\r\n",
            "https://example.com/a\0b",
            "https://example.com/a\tb",
            " https://example.com",
            "https://example.com ",
            "https://example.com/a b",
            "https://example.com/\" --new-window \"file:///C:/x",
            "https://example.com/<script>",
            "https://example.com/a|b",
            "https://example.com/a^b",
            "https://example.com/a`b",
            "https://example.com/{a}",
            "https://example.com/caf\u{e9}",
            "https://ex\u{e4}mple.com",
            "https://example.com/\u{202e}gpj.exe",
            "https://example.com/\u{a0}",
            "https\u{ff1a}//example.com",
            too_long.as_str(),
        ] {
            assert!(!is_https(url), "{url:?}");
            assert_ne!(verdict(url), Verdict::Open, "{url:?}");
        }
    }

    #[test]
    fn only_a_short_clean_string_is_worth_comparing_with_the_engines_links() {
        for url in [
            "http://localhost:3000/dashboard",
            "http://127.0.0.1:3000/pools",
            "file:///C:/x",
            "javascript:alert(1)",
            "C:\\Windows\\notepad.exe",
        ] {
            // Not opened by this verdict: the caller opens it only when the engine made it.
            assert_eq!(verdict(url), Verdict::IfTheEngines, "{url:?}");
        }
        let too_long = format!("http://localhost/{}", "a".repeat(3000));
        for url in [
            "",
            " http://localhost:3000/",
            "http://localhost:3000/ ",
            "http://localhost:3000/a b",
            "http://localhost:3000/\n",
            "http://localhost:3000/\0",
            "http://localhost:3000/\u{a0}",
            "http://localhost:3000/\u{2028}",
            too_long.as_str(),
        ] {
            assert_eq!(verdict(url), Verdict::Refused, "{url:?}");
        }
    }
}
