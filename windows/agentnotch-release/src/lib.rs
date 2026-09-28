//! The Windows update key, its signatures and the update feed (DESIGN-WIN §6.4).
//!
//! Every function here is pure: the command-line tool in `main.rs` reads the seed from stdin and
//! the files from disk, and hands them in. Keeping the format code apart from the I/O is what
//! lets the tests pin it byte for byte.
//!
//! **The key.** A separate Ed25519 key is derived from the Sparkle seed with HKDF-SHA256
//! (`salt = "com.rivantmedia.agentnotch"`, `info = "tauri-updater minisign ed25519 v1"`, 40
//! bytes): the first 32 are the signing seed, the last 8 the minisign key id. Domain separation
//! is the point: Sparkle and the Tauri updater never see the same Ed25519 key, so a signature
//! made for one can never be replayed to the other. Rotating only the Windows key means bumping
//! `v1` in the label (with one bridge release, §6.4 item 6).
//!
//! **The formats** are minisign's prehashed ones, exactly as `tauri signer sign` writes them and
//! as `tauri-plugin-updater` 2.12.0 decodes them (`base64(text)` for both the public key and the
//! `.sig`; `version:<V>` in the trusted comment for `requireSignedVersion`). Verification goes
//! through `minisign-verify` 0.2.5 with `allow_legacy = true`, the updater's own crate and call,
//! so a signature this crate accepts is one every installed copy accepts.

use std::fmt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use blake2::{Blake2b512, Digest};
use ed25519_dalek::{Signer, SigningKey};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

/// HKDF salt: the app's identifier, so no other product can arrive at the same key by accident.
pub const HKDF_SALT: &[u8] = b"com.rivantmedia.agentnotch";
/// HKDF info: names the purpose and the generation. Bumping `v1` rotates the Windows key alone.
pub const HKDF_INFO: &[u8] = b"tauri-updater minisign ed25519 v1";
/// The first line of every signature this tool writes (not covered by any signature).
pub const SIGNATURE_UNTRUSTED_COMMENT: &str =
    "untrusted comment: signature from agentnotch release key";
/// The feed installed copies read (release builds carry it; the tool writes tag-specific URLs).
pub const FEED_URL: &str =
    "https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json";
/// The updater's platform keys: it looks for `windows-x86_64-nsis` first, then `windows-x86_64`.
pub const FEED_PLATFORMS: [&str; 2] = ["windows-x86_64-nsis", "windows-x86_64"];

/// Why an input was refused. The message never contains key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    fn new(message: impl Into<String>) -> Error {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The Sparkle seed: the base64 of 32 bytes, as `SPARKLE_ED_PRIVATE_KEY` holds it. Zeroised on
/// drop.
pub struct Seed(Zeroizing<[u8; 32]>);

impl Seed {
    /// Decodes what came in on stdin: ASCII whitespace anywhere is ignored (the secret may carry
    /// a trailing newline, and `release-build.sh` strips every space too), the rest must be the
    /// padded standard base64 of exactly 32 bytes.
    pub fn from_base64(input: &[u8]) -> Result<Seed, Error> {
        let mut compact = Zeroizing::new(Vec::with_capacity(input.len()));
        compact.extend(input.iter().copied().filter(|b| !b.is_ascii_whitespace()));
        if compact.is_empty() {
            return Err(Error::new("no seed on stdin"));
        }
        // Decoded into a fixed buffer we own, so the only copy of the seed is one we zeroise.
        let mut decoded = Zeroizing::new([0u8; 64]);
        let len = match STANDARD.decode_slice(&compact[..], &mut decoded[..]) {
            Ok(len) => len,
            Err(_) => {
                return Err(Error::new(
                    "the seed on stdin is not the base64 of 32 bytes",
                ))
            }
        };
        if len != 32 {
            return Err(Error::new(
                "the seed on stdin is not the base64 of 32 bytes",
            ));
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&decoded[..32]);
        Ok(Seed(seed))
    }

    /// The Ed25519 public key of the seed itself: what `Scripts/sparkle-public-ed-key.txt`
    /// holds. The release jobs compare it with the committed file, so the Windows key can only
    /// ever be derived from the seed the Mac releases are signed with.
    pub fn sparkle_public_key(&self) -> String {
        let key = SigningKey::from_bytes(&self.0);
        STANDARD.encode(key.verifying_key().as_bytes())
    }
}

/// A minisign key id: 8 bytes, stored in blobs in this order and printed (as minisign prints
/// it) as the little-endian u64 in upper-case hex, i.e. the bytes reversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyId(pub [u8; 8]);

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016X}", u64::from_le_bytes(self.0))
    }
}

/// The derived Windows update key.
pub struct UpdateKey {
    signing: SigningKey,
    key_id: KeyId,
}

impl UpdateKey {
    /// HKDF-SHA256 over the seed; see the module comment. The HKDF context keeps its own
    /// pseudo-random key internally, which the `hkdf` crate does not zeroise; it lives only for
    /// the duration of this call.
    pub fn derive(seed: &Seed) -> UpdateKey {
        let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), &seed.0[..]);
        let mut okm = Zeroizing::new([0u8; 40]);
        // 40 bytes is far below HKDF-SHA256's 8160-byte limit, so this cannot fail.
        if hk.expand(HKDF_INFO, &mut okm[..]).is_err() {
            unreachable!("HKDF-SHA256 can always produce 40 bytes");
        }
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&okm[..32]);
        let mut id = [0u8; 8];
        id.copy_from_slice(&okm[32..40]);
        UpdateKey {
            // SigningKey zeroises itself on drop (ed25519-dalek's default `zeroize` feature).
            signing: SigningKey::from_bytes(&secret),
            key_id: KeyId(id),
        }
    }

    pub fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The raw 32-byte Ed25519 public key, base64.
    pub fn raw_public_key(&self) -> String {
        STANDARD.encode(self.signing.verifying_key().as_bytes())
    }

    /// minisign's public key line: `base64("Ed" ‖ key id ‖ public key)`, starting `RW`.
    pub fn minisign_public_key(&self) -> String {
        let mut blob = Vec::with_capacity(42);
        blob.extend_from_slice(b"Ed");
        blob.extend_from_slice(&self.key_id.0);
        blob.extend_from_slice(self.signing.verifying_key().as_bytes());
        STANDARD.encode(blob)
    }

    /// The `minisign.pub` text, with its trailing newline, as `tauri signer generate` writes it.
    pub fn public_key_text(&self) -> String {
        format!(
            "untrusted comment: minisign public key: {}\n{}\n",
            self.key_id,
            self.minisign_public_key()
        )
    }

    /// The value of `plugins.updater.pubkey`: the standard base64 of the public key text.
    pub fn tauri_pubkey(&self) -> String {
        STANDARD.encode(self.public_key_text())
    }

    /// Signs `data` in minisign's prehashed format and returns the `.sig` content: the standard
    /// base64 of the signature text, with no trailing newline (the updater decodes the feed's
    /// `signature` field with a strict decoder, so a newline there would break every update).
    ///
    /// `sig = Ed25519(BLAKE2b-512(data))`, the trusted comment is
    /// `timestamp:<unix s>\tfile:<file name>\tversion:<V>`, and the global signature covers
    /// `sig ‖ trusted comment`, which is what makes `version:` trustworthy.
    pub fn sign(
        &self,
        data: &[u8],
        file_name: &str,
        version: &str,
        timestamp: u64,
    ) -> Result<String, Error> {
        check_version(version)?;
        check_file_name(file_name)?;
        let digest = Blake2b512::digest(data);
        let signature = self.signing.sign(&digest);
        let trusted = trusted_comment(timestamp, file_name, version);
        let mut global_message = Vec::with_capacity(64 + trusted.len());
        global_message.extend_from_slice(&signature.to_bytes());
        global_message.extend_from_slice(trusted.as_bytes());
        let global = self.signing.sign(&global_message);

        let mut blob = Vec::with_capacity(74);
        blob.extend_from_slice(b"ED");
        blob.extend_from_slice(&self.key_id.0);
        blob.extend_from_slice(&signature.to_bytes());
        let text = format!(
            "{SIGNATURE_UNTRUSTED_COMMENT}\n{}\ntrusted comment: {trusted}\n{}\n",
            STANDARD.encode(blob),
            STANDARD.encode(global.to_bytes())
        );
        Ok(STANDARD.encode(text))
    }
}

/// The trusted comment in the field order the Tauri CLI writes.
pub fn trusted_comment(timestamp: u64, file_name: &str, version: &str) -> String {
    format!("timestamp:{timestamp}\tfile:{file_name}\tversion:{version}")
}

/// Versions are `VERSION`'s: `major.minor.patch`, numbers only (the release workflow's rule).
pub fn check_version(version: &str) -> Result<(), Error> {
    let parts: Vec<&str> = version.split('.').collect();
    let numeric = |p: &&str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if parts.len() == 3 && parts.iter().all(numeric) {
        Ok(())
    } else {
        Err(Error::new(format!(
            "version '{version}' is not major.minor.patch"
        )))
    }
}

/// A file name that fits in a trusted comment field and a download URL unescaped.
pub fn check_file_name(name: &str) -> Result<(), Error> {
    let fits = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_');
    if fits {
        Ok(())
    } else {
        Err(Error::new(format!(
            "file name '{name}' must be letters, digits, '.', '-' or '_' only"
        )))
    }
}

/// What a verified signature says, read only after minisign checked the global signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub key_id: KeyId,
    pub trusted_comment: String,
    pub file: String,
    pub version: String,
}

/// Decodes a public key given as the Tauri config value (base64 of the text) or as the bare
/// minisign line (`RW…`), into the text `minisign_verify::PublicKey::decode` takes.
pub fn public_key_text(pubkey: &str) -> Result<String, Error> {
    let pubkey = pubkey.trim();
    if pubkey.starts_with("RW") {
        return Ok(format!(
            "untrusted comment: minisign public key\n{pubkey}\n"
        ));
    }
    let bytes = STANDARD
        .decode(pubkey)
        .map_err(|_| Error::new("the public key is neither a Tauri pubkey nor a minisign key"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new("the public key is neither a Tauri pubkey nor a minisign key"))?;
    if !text.starts_with("untrusted comment:") {
        return Err(Error::new(
            "the public key is neither a Tauri pubkey nor a minisign key",
        ));
    }
    Ok(text)
}

/// The key id a public key names (bytes 2..10 of its `Ed` blob).
pub fn key_id_of_pubkey(pubkey: &str) -> Result<KeyId, Error> {
    let text = public_key_text(pubkey)?;
    let line = text
        .lines()
        .nth(1)
        .ok_or_else(|| Error::new("the public key has no key line"))?;
    let blob = STANDARD
        .decode(line.trim())
        .map_err(|_| Error::new("the public key's key line is not base64"))?;
    if blob.len() != 42 || &blob[..2] != b"Ed" {
        return Err(Error::new("the public key is not a minisign Ed25519 key"));
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&blob[2..10]);
    Ok(KeyId(id))
}

/// The signature text inside a `.sig` value, decoded exactly as the updater decodes it (strict
/// standard base64, no whitespace).
fn signature_text(signature: &str) -> Result<String, Error> {
    let bytes = STANDARD.decode(signature).map_err(|_| {
        Error::new("the signature is not strict base64 (the updater would refuse it)")
    })?;
    String::from_utf8(bytes).map_err(|_| Error::new("the signature text is not UTF-8"))
}

/// The key id a signature was made with (bytes 2..10 of its blob), and whether it is prehashed.
fn signature_blob_facts(text: &str) -> Result<(KeyId, bool), Error> {
    let line = text
        .lines()
        .nth(1)
        .ok_or_else(|| Error::new("the signature has no signature line"))?;
    let blob = STANDARD
        .decode(line.trim())
        .map_err(|_| Error::new("the signature line is not base64"))?;
    if blob.len() != 74 {
        return Err(Error::new("the signature line is not a minisign signature"));
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(&blob[2..10]);
    Ok((KeyId(id), &blob[..2] == b"ED"))
}

/// The key id a `.sig` value was made with.
pub fn key_id_of_signature(signature: &str) -> Result<KeyId, Error> {
    Ok(signature_blob_facts(&signature_text(signature)?)?.0)
}

/// Verifies `signature` (a `.sig` value) over `data` the way `tauri-plugin-updater` does
/// (`PublicKey::decode`, `Signature::decode`, `verify(data, sig, true)`), then checks what the
/// trusted comment says: `version:` must be `version` (the updater's `requireSignedVersion`
/// reads the first such field, and so does this) and `file:` must be `file_name`. It also
/// insists on the prehashed algorithm, which is all this tool and the Tauri CLI ever write, so a
/// legacy signature from somewhere else never passes the release gate.
pub fn verify(
    pubkey: &str,
    data: &[u8],
    signature: &str,
    version: &str,
    file_name: &str,
) -> Result<Verified, Error> {
    let key = minisign_verify::PublicKey::decode(&public_key_text(pubkey)?)
        .map_err(|e| Error::new(format!("the public key does not decode: {e}")))?;
    let text = signature_text(signature)?;
    let decoded = minisign_verify::Signature::decode(&text)
        .map_err(|e| Error::new(format!("the signature does not decode: {e}")))?;
    key.verify(data, &decoded, true)
        .map_err(|e| Error::new(format!("the signature does not verify: {e}")))?;
    let (key_id, prehashed) = signature_blob_facts(&text)?;
    if !prehashed {
        return Err(Error::new(
            "the signature is minisign's legacy (not prehashed) kind",
        ));
    }
    // Only now is the trusted comment trustworthy: the global signature covering it was checked.
    let trusted = decoded.trusted_comment().to_string();
    let field = |name: &str| {
        trusted
            .split('\t')
            .find_map(|f| f.strip_prefix(name))
            .map(str::to_string)
    };
    let signed_version =
        field("version:").ok_or_else(|| Error::new("the signature carries no signed version"))?;
    if signed_version != version {
        return Err(Error::new(format!(
            "the signature is for version {signed_version}, not {version}"
        )));
    }
    let signed_file = field("file:").ok_or_else(|| Error::new("the signature names no file"))?;
    if signed_file != file_name {
        return Err(Error::new(format!(
            "the signature is for the file {signed_file}, not {file_name}"
        )));
    }
    Ok(Verified {
        key_id,
        trusted_comment: trusted,
        file: signed_file,
        version: signed_version,
    })
}

/// Everything `latest.json` is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedInput {
    pub version: String,
    pub tag: String,
    pub repo: String,
    pub installer: String,
    pub signature: String,
    pub notes_url: String,
    pub pub_date: String,
}

impl FeedInput {
    /// The installer's download URL: always the tag's own (like the appcast's enclosure), never
    /// `latest/download`, so a feed can only ever point at the release it was made for.
    pub fn installer_url(&self) -> String {
        format!(
            "https://github.com/{}/releases/download/{}/{}",
            self.repo, self.tag, self.installer
        )
    }

    fn check(&self) -> Result<(), Error> {
        check_version(&self.version)?;
        check_file_name(&self.installer)?;
        if self.tag != format!("agentnotch-v{}", self.version) {
            return Err(Error::new(format!(
                "tag '{}' is not agentnotch-v{}",
                self.tag, self.version
            )));
        }
        let repo_ok = self.repo.split('/').count() == 2
            && self.repo.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            });
        if !repo_ok {
            return Err(Error::new(format!(
                "repository '{}' is not owner/name",
                self.repo
            )));
        }
        let notes_ok = self.notes_url.starts_with("https://")
            && !self
                .notes_url
                .chars()
                .any(|c| c.is_whitespace() || c.is_control());
        if !notes_ok {
            return Err(Error::new(format!(
                "notes URL '{}' is not an https address",
                self.notes_url
            )));
        }
        check_rfc3339(&self.pub_date)?;
        signature_text(&self.signature)?;
        Ok(())
    }
}

/// `latest.json` in the static format the updater reads: both platform keys carry the same
/// signature and URL (`windows-x86_64-nsis` is looked up first, `windows-x86_64` is the
/// fallback older plugin versions use). Written by hand so the key order is the documented one.
pub fn feed_json(input: &FeedInput) -> Result<String, Error> {
    input.check()?;
    let s = |v: &str| serde_json::to_string(v).unwrap_or_default();
    let url = input.installer_url();
    let entry = format!(
        "{{\n      \"signature\": {},\n      \"url\": {}\n    }}",
        s(&input.signature),
        s(&url)
    );
    Ok(format!(
        "{{\n  \"version\": {},\n  \"notes\": {},\n  \"pub_date\": {},\n  \"platforms\": {{\n    {}: {entry},\n    {}: {entry}\n  }}\n}}\n",
        s(&input.version),
        s(&input.notes_url),
        s(&input.pub_date),
        s(FEED_PLATFORMS[0]),
        s(FEED_PLATFORMS[1]),
    ))
}

/// One platform entry of a parsed feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    pub platform: String,
    pub signature: String,
    pub url: String,
}

/// A parsed `latest.json`, with only what the updater reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub entries: Vec<FeedEntry>,
}

/// Parses a feed. Every Windows platform entry it names is kept; entries for other platforms
/// are ignored (this fork publishes none).
pub fn parse_feed(json: &str) -> Result<Feed, Error> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::new(format!("the feed is not JSON: {e}")))?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let version = text("version").ok_or_else(|| Error::new("the feed has no version"))?;
    let platforms = value
        .get("platforms")
        .and_then(|p| p.as_object())
        .ok_or_else(|| Error::new("the feed has no platforms"))?;
    let mut entries = Vec::new();
    for name in FEED_PLATFORMS {
        if let Some(entry) = platforms.get(name) {
            let field = |key: &str| {
                entry
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| Error::new(format!("the feed's {name} entry has no {key}")))
            };
            entries.push(FeedEntry {
                platform: name.to_string(),
                signature: field("signature")?,
                url: field("url")?,
            });
        }
    }
    if entries.is_empty() {
        return Err(Error::new("the feed names no Windows platform"));
    }
    Ok(Feed {
        version,
        notes: text("notes"),
        pub_date: text("pub_date"),
        entries,
    })
}

/// The key id every Windows entry of a feed was signed with; entries signed with different keys
/// are refused (a feed is made by one key).
pub fn key_id_of_feed(json: &str) -> Result<KeyId, Error> {
    let feed = parse_feed(json)?;
    let mut ids = feed
        .entries
        .iter()
        .map(|e| key_id_of_signature(&e.signature));
    let first = ids
        .next()
        .unwrap_or_else(|| Err(Error::new("empty feed")))?;
    for id in ids {
        if id? != first {
            return Err(Error::new(
                "the feed's platform entries were signed with different keys",
            ));
        }
    }
    Ok(first)
}

/// Re-reads a written feed as the updater would and checks it describes exactly `input`, with a
/// signature taken from the JSON that verifies over `installer_bytes`.
pub fn check_feed(
    json: &str,
    input: &FeedInput,
    pubkey: &str,
    installer_bytes: &[u8],
) -> Result<Verified, Error> {
    let feed = parse_feed(json)?;
    if feed.version != input.version {
        return Err(Error::new(format!(
            "the feed says version {}, not {}",
            feed.version, input.version
        )));
    }
    match &feed.pub_date {
        Some(date) => check_rfc3339(date)?,
        None => return Err(Error::new("the feed has no pub_date")),
    }
    if feed.entries.len() != FEED_PLATFORMS.len() {
        return Err(Error::new("the feed lacks one of its two Windows entries"));
    }
    let url = input.installer_url();
    let mut verified = None;
    for entry in &feed.entries {
        if entry.url != url {
            return Err(Error::new(format!(
                "the feed's {} URL is {}, not {url}",
                entry.platform, entry.url
            )));
        }
        verified = Some(verify(
            pubkey,
            installer_bytes,
            &entry.signature,
            &input.version,
            &input.installer,
        )?);
    }
    verified.ok_or_else(|| Error::new("the feed has no entries"))
}

/// RFC 3339 as the updater's `pub_date` parser takes it: `YYYY-MM-DDTHH:MM:SS`, optional
/// fraction, then `Z` or `±HH:MM`, with real calendar dates.
pub fn check_rfc3339(value: &str) -> Result<(), Error> {
    let bad = || Error::new(format!("'{value}' is not an RFC 3339 date-time"));
    let b = value.as_bytes();
    if b.len() < 20 {
        return Err(bad());
    }
    let num = |range: std::ops::Range<usize>| -> Result<u32, Error> {
        let part = &value[range];
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse::<u32>().map_err(|_| bad())
        } else {
            Err(bad())
        }
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(bad());
    }
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(bad());
    }
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(bad());
        }
        rest = &fraction[digits..];
    }
    match rest.as_bytes() {
        [b'Z' | b'z'] => Ok(()),
        [sign, h1, h2, b':', m1, m2]
            if matches!(sign, b'+' | b'-')
                && [h1, h2, m1, m2].iter().all(|c| c.is_ascii_digit()) =>
        {
            let hours = (h1 - b'0') * 10 + (h2 - b'0');
            let minutes = (m1 - b'0') * 10 + (m2 - b'0');
            if hours <= 23 && minutes <= 59 {
                Ok(())
            } else {
                Err(bad())
            }
        }
        _ => Err(bad()),
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        _ => 28,
    }
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC), for the feed's default `pub_date`.
pub fn rfc3339_utc(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let secs = unix_seconds % 86_400;
    // Howard Hinnant's days-to-civil: exact for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Appendix B of DESIGN-WIN: a test seed only (bytes 0x00..0x1f), never a real key. Computed
    // independently with Python's hmac/hashlib HKDF and CryptoKit Ed25519.
    const TEST_SEED: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    fn test_key() -> UpdateKey {
        UpdateKey::derive(&Seed::from_base64(TEST_SEED.as_bytes()).unwrap())
    }

    #[test]
    fn the_design_vector_derives_the_same_key() {
        let key = test_key();
        assert_eq!(
            hex(&key.signing.to_bytes()),
            "96767d4eebd08c92379fb197f9e0d3a3500f80a8c87faca08b3f00b3440b8f2c"
        );
        assert_eq!(hex(&key.key_id().0), "19d0fb618363a5b5");
        assert_eq!(key.key_id().to_string(), "B5A5638361FBD019");
        assert_eq!(
            key.raw_public_key(),
            "zA8/6NE42whHHOfU14QWOKEVItB9KMnwL3wVMV/+O/g="
        );
        assert_eq!(
            key.minisign_public_key(),
            "RWQZ0Pthg2OltcwPP+jRONsIRxzn1NeEFjihFSLQfSjJ8C98FTFf/jv4"
        );
        assert_eq!(
            key.tauri_pubkey(),
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI1QTU2MzgzNjFGQkQwMTkKUldRWjBQdGhnMk9sdGN3UFAralJPTnNJUnh6bjFOZUVGamloRlNMUWZTako4Qzk4RlRGZi9qdjQK"
        );
        assert_eq!(key_id_of_pubkey(&key.tauri_pubkey()).unwrap(), key.key_id());
    }

    #[test]
    fn the_sparkle_public_key_is_the_seeds_own() {
        // CryptoKit: `swift Scripts/release-ed25519.swift public` of the same test seed.
        let seed = Seed::from_base64(TEST_SEED.as_bytes()).unwrap();
        assert_eq!(
            seed.sparkle_public_key(),
            "A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="
        );
    }

    #[test]
    fn seeds_are_trimmed_but_must_be_32_bytes() {
        assert!(Seed::from_base64(format!("  {TEST_SEED}\r\n").as_bytes()).is_ok());
        assert!(Seed::from_base64(b"").is_err());
        assert!(Seed::from_base64(b" \n").is_err());
        // 31 bytes, 33 bytes, not base64, unpadded.
        assert!(Seed::from_base64(b"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHg==").is_err());
        assert!(Seed::from_base64(b"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g").is_err());
        assert!(Seed::from_base64(b"not a key at all!").is_err());
        assert!(Seed::from_base64(b"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8").is_err());
    }

    #[test]
    fn errors_never_echo_the_input() {
        let secret = "c2VjcmV0LXNlZWQtbWF0ZXJpYWwtdGhhdC1pcy13cm9uZw==";
        let error = Seed::from_base64(secret.as_bytes()).err().unwrap();
        assert!(!error.0.contains(secret));
        assert!(!error.0.contains("c2Vj"));
    }

    #[test]
    fn versions_and_file_names() {
        for good in ["1.1.0", "0.0.1", "10.20.30"] {
            assert!(check_version(good).is_ok(), "{good}");
        }
        for bad in [
            "1.1",
            "1.1.0.0",
            "v1.1.0",
            "1.1.0-beta",
            "",
            "1..0",
            "1.1.x",
        ] {
            assert!(check_version(bad).is_err(), "{bad}");
        }
        assert!(check_file_name("AgentNotch-1.1.0-Setup.exe").is_ok());
        for bad in [
            "", ".", "..", "a b.exe", "a\tb", "a/b.exe", "a\\b.exe", "é.exe",
        ] {
            assert!(check_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn dates() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_790_594_405), "2026-09-28T11:20:05Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(4_107_542_399), "2100-02-28T23:59:59Z");
        for good in [
            "2026-10-01T12:00:00Z",
            "2026-10-01T12:00:00.123Z",
            "2024-02-29T00:00:00+05:30",
            "2026-10-01t12:00:00z",
        ] {
            assert!(check_rfc3339(good).is_ok(), "{good}");
        }
        for bad in [
            "2026-10-01",
            "2026-10-01 12:00:00Z",
            "2026-13-01T12:00:00Z",
            "2025-02-29T12:00:00Z",
            "2026-10-01T24:00:00Z",
            "2026-10-01T12:00:00",
            "2026-10-01T12:00:00.Z",
            "2026-10-01T12:00:00+5:30",
        ] {
            assert!(check_rfc3339(bad).is_err(), "{bad}");
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
