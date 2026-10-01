//! Claude Code versions (ClaudeBinaryLocator.swift `ClaudeCodeVersion`): which
//! hook events a settings.json may name, and whether exec-form hooks may be
//! written, both follow the oldest Claude Code that can read the file.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClaudeCodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ClaudeCodeVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> ClaudeCodeVersion {
        ClaudeCodeVersion {
            major,
            minor,
            patch,
        }
    }

    /// The first `X.Y.Z` in arbitrary text: `2.1.88`, `v2.1.88`,
    /// `2.1.88 (Claude Code)`, an extension folder `anthropic.claude-code-2.1.88-win32-x64`.
    pub fn parse(text: &str) -> Option<ClaudeCodeVersion> {
        let bytes = text.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            if bytes[start].is_ascii_digit() && (start == 0 || !bytes[start - 1].is_ascii_digit()) {
                if let Some(version) = Self::parse_at(&text[start..]) {
                    return Some(version);
                }
            }
            start += 1;
        }
        None
    }

    /// `X.Y.Z` at the very start of `text`.
    fn parse_at(text: &str) -> Option<ClaudeCodeVersion> {
        let mut parts = [0u32; 3];
        let mut rest = text;
        for (index, part) in parts.iter_mut().enumerate() {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            *part = rest[..digits].parse().ok()?;
            rest = &rest[digits..];
            if index < 2 {
                rest = rest.strip_prefix('.')?;
            }
        }
        Some(ClaudeCodeVersion::new(parts[0], parts[1], parts[2]))
    }
}

impl fmt::Display for ClaudeCodeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_first_triple() {
        let v = ClaudeCodeVersion::new(2, 1, 88);
        for text in [
            "2.1.88",
            "v2.1.88",
            "2.1.88 (Claude Code)",
            "anthropic.claude-code-2.1.88-win32-x64",
            "Claude Code 2.1.88\r\n",
        ] {
            assert_eq!(ClaudeCodeVersion::parse(text), Some(v), "{text}");
        }
        assert_eq!(ClaudeCodeVersion::parse("2.1"), None);
        assert_eq!(ClaudeCodeVersion::parse("latest"), None);
        assert_eq!(ClaudeCodeVersion::parse(""), None);
        // A number glued to a longer one is not split: "12.1.3" is 12.1.3.
        assert_eq!(
            ClaudeCodeVersion::parse("x12.1.3"),
            Some(ClaudeCodeVersion::new(12, 1, 3))
        );
        assert_eq!(
            ClaudeCodeVersion::parse("1.2.x 3.4.5"),
            Some(ClaudeCodeVersion::new(3, 4, 5))
        );
    }

    #[test]
    fn orders_numerically() {
        assert!(ClaudeCodeVersion::new(2, 1, 100) > ClaudeCodeVersion::new(2, 1, 99));
        assert!(ClaudeCodeVersion::new(2, 10, 0) > ClaudeCodeVersion::new(2, 9, 99));
        assert_eq!(ClaudeCodeVersion::new(2, 1, 7).to_string(), "2.1.7");
    }
}
