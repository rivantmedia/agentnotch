//! `agentnotch-release`: the Windows update key, signature and feed (DESIGN-WIN §6.4).
//!
//! | Subcommand | Input | Output |
//! |---|---|---|
//! | `derive-public` | seed on stdin | `{"pubkey","key_id","minisign_public_key","sparkle_public_key"}` |
//! | `sign --file F --version V --out S [--timestamp UNIX]` | seed on stdin | writes S; `{"key_id","trusted_comment"}` |
//! | `verify --pubkey P --file F --sig S --version V` | — | exit 0 and `{"key_id","trusted_comment"}`, or 1 with the reason |
//! | `feed --version V --tag T --repo R --installer NAME --sig S --pubkey P --file F --notes-url U --out J [--pub-date RFC3339]` | — | writes and re-verifies `latest.json`; `{"key_id","url"}` |
//! | `key-id-of-feed J` / `key-id-of-pubkey P` | — | the 16-hex key id |
//!
//! The seed (the Sparkle key's, base64) is only ever read from stdin, never from argv, never
//! printed, and zeroised after use: the release jobs pipe it in from a secret held in one step's
//! environment. `P` is the Tauri pubkey (the value of `plugins.updater.pubkey`) or the bare
//! minisign key line. Exit codes: 0 done, 1 refused (bad input, a signature that does not
//! verify), 2 a usage error. Nothing here is parsed with an argument crate: the tool is small,
//! and every flag it takes is listed below.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use agentnotch_release::{
    check_feed, check_file_name, check_version, feed_json, key_id_of_feed, key_id_of_pubkey,
    rfc3339_utc, verify, Error, FeedInput, Seed, UpdateKey,
};
use zeroize::Zeroizing;

const USAGE: &str = "usage: agentnotch-release <command>
  derive-public                                   (seed on stdin)
  sign --file F --version V --out S [--timestamp UNIX]   (seed on stdin)
  verify --pubkey P --file F --sig S --version V
  feed --version V --tag T --repo R --installer NAME --sig S --pubkey P --file F
       --notes-url U --out J [--pub-date RFC3339]
  key-id-of-feed J
  key-id-of-pubkey P";

/// The longest stdin a seed can come in on: 44 base64 characters plus generous whitespace. More
/// means something else was piped in, which is refused rather than decoded.
const MAX_SEED_INPUT: u64 = 4096;

enum Failure {
    Usage(String),
    Refused(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        Failure::Refused(e)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = match std::env::args_os()
        .skip(1)
        .map(|a| a.into_string())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(args) => args,
        Err(_) => {
            eprintln!("agentnotch-release: arguments must be UTF-8\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(output) => {
            let mut stdout = std::io::stdout().lock();
            if writeln!(stdout, "{output}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(message)) => {
            eprintln!("agentnotch-release: {message}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Refused(error)) => {
            eprintln!("agentnotch-release: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<String, Failure> {
    let Some((command, rest)) = args.split_first() else {
        return Err(Failure::Usage("no command".into()));
    };
    match command.as_str() {
        "derive-public" => {
            options(rest, &[], &[])?;
            derive_public()
        }
        "sign" => {
            let o = options(rest, &["file", "version", "out"], &["timestamp"])?;
            sign(&o)
        }
        "verify" => {
            let o = options(rest, &["pubkey", "file", "sig", "version"], &[])?;
            verify_command(&o)
        }
        "feed" => {
            let o = options(
                rest,
                &[
                    "version",
                    "tag",
                    "repo",
                    "installer",
                    "sig",
                    "pubkey",
                    "file",
                    "notes-url",
                    "out",
                ],
                &["pub-date"],
            )?;
            feed(&o)
        }
        "key-id-of-feed" => {
            let path = single_argument(rest)?;
            let json = read_text(&path)?;
            Ok(key_id_of_feed(&json)?.to_string())
        }
        "key-id-of-pubkey" => {
            let pubkey = single_argument(rest)?;
            Ok(key_id_of_pubkey(&pubkey)?.to_string())
        }
        "-h" | "--help" | "help" => Ok(USAGE.to_string()),
        other => Err(Failure::Usage(format!("unknown command '{other}'"))),
    }
}

/// `--name value` pairs: every required flag once, optional ones at most once, nothing else.
/// A seed can never be passed this way: no subcommand has a flag for it.
fn options(
    args: &[String],
    required: &[&str],
    optional: &[&str],
) -> Result<BTreeMap<String, String>, Failure> {
    let mut found = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let Some(name) = flag.strip_prefix("--") else {
            // Never echoed: a seed pasted as an argument must not end up in a log.
            return Err(Failure::Usage("unexpected positional argument".into()));
        };
        if !required.contains(&name) && !optional.contains(&name) {
            return Err(Failure::Usage(format!("unknown option '--{name}'")));
        }
        let Some(value) = iter.next() else {
            return Err(Failure::Usage(format!("--{name} needs a value")));
        };
        if found.insert(name.to_string(), value.clone()).is_some() {
            return Err(Failure::Usage(format!("--{name} given twice")));
        }
    }
    for name in required {
        if !found.contains_key(*name) {
            return Err(Failure::Usage(format!("--{name} is required")));
        }
    }
    Ok(found)
}

fn single_argument(args: &[String]) -> Result<String, Failure> {
    match args {
        [one] if !one.starts_with("--") => Ok(one.clone()),
        _ => Err(Failure::Usage("expects exactly one argument".into())),
    }
}

fn read_seed() -> Result<Seed, Failure> {
    let mut input = Zeroizing::new(Vec::new());
    std::io::stdin()
        .lock()
        .take(MAX_SEED_INPUT + 1)
        .read_to_end(&mut input)
        .map_err(|e| Error(format!("cannot read stdin: {}", e.kind())))?;
    if input.len() as u64 > MAX_SEED_INPUT {
        return Err(Error("stdin holds more than a seed".into()).into());
    }
    Ok(Seed::from_base64(&input)?)
}

fn now_unix() -> Result<u64, Failure> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error("the clock is before 1970".into()))?
        .as_secs())
}

fn derive_public() -> Result<String, Failure> {
    let seed = read_seed()?;
    let key = UpdateKey::derive(&seed);
    let output = serde_json::json!({
        "pubkey": key.tauri_pubkey(),
        "key_id": key.key_id().to_string(),
        "minisign_public_key": key.minisign_public_key(),
        "sparkle_public_key": seed.sparkle_public_key(),
    });
    Ok(output.to_string())
}

fn sign(o: &BTreeMap<String, String>) -> Result<String, Failure> {
    let file = &o["file"];
    let version = &o["version"];
    let timestamp = match o.get("timestamp") {
        Some(t) => match t.parse::<u64>() {
            Ok(value) if t.bytes().all(|b| b.is_ascii_digit()) => value,
            _ => {
                return Err(Failure::Usage(format!(
                    "--timestamp '{t}' is not a Unix time"
                )))
            }
        },
        None => now_unix()?,
    };
    // Everything that can be refused without the seed is refused before it is read.
    let name = file_name(file)?;
    check_version(version)?;
    check_file_name(&name)?;
    let data = read_bytes(file)?;
    let seed = read_seed()?;
    let key = UpdateKey::derive(&seed);
    drop(seed);
    let signature = key.sign(&data, &name, version, timestamp)?;
    // A signature is only written after it verifies, through the updater's own crate.
    let verified = verify(&key.tauri_pubkey(), &data, &signature, version, &name)?;
    std::fs::write(&o["out"], signature.as_bytes())
        .map_err(|e| Error(format!("cannot write {}: {e}", o["out"])))?;
    Ok(serde_json::json!({
        "key_id": verified.key_id.to_string(),
        "trusted_comment": verified.trusted_comment,
    })
    .to_string())
}

fn verify_command(o: &BTreeMap<String, String>) -> Result<String, Failure> {
    let name = file_name(&o["file"])?;
    let data = read_bytes(&o["file"])?;
    let signature = read_text(&o["sig"])?;
    let verified = verify(&o["pubkey"], &data, &signature, &o["version"], &name)?;
    Ok(serde_json::json!({
        "key_id": verified.key_id.to_string(),
        "trusted_comment": verified.trusted_comment,
    })
    .to_string())
}

fn feed(o: &BTreeMap<String, String>) -> Result<String, Failure> {
    let name = file_name(&o["file"])?;
    if o["installer"] != name {
        return Err(Error(format!(
            "--installer {} is not the file's name ({name})",
            o["installer"]
        ))
        .into());
    }
    let pub_date = match o.get("pub-date") {
        Some(date) => date.clone(),
        None => rfc3339_utc(now_unix()?),
    };
    let input = FeedInput {
        version: o["version"].clone(),
        tag: o["tag"].clone(),
        repo: o["repo"].clone(),
        installer: o["installer"].clone(),
        signature: read_text(&o["sig"])?,
        notes_url: o["notes-url"].clone(),
        pub_date,
    };
    let data = read_bytes(&o["file"])?;
    // The signature must verify before a feed is written around it…
    verify(
        &o["pubkey"],
        &data,
        &input.signature,
        &input.version,
        &input.installer,
    )?;
    let json = feed_json(&input)?;
    std::fs::write(&o["out"], json.as_bytes())
        .map_err(|e| Error(format!("cannot write {}: {e}", o["out"])))?;
    // …and again as read back from the file, with the signature taken from the JSON.
    let written = read_text(&o["out"])?;
    let verified = check_feed(&written, &input, &o["pubkey"], &data)?;
    Ok(serde_json::json!({
        "key_id": verified.key_id.to_string(),
        "url": input.installer_url(),
    })
    .to_string())
}

fn file_name(path: &str) -> Result<String, Failure> {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| Error(format!("{path} names no file")).into())
}

fn read_bytes(path: &str) -> Result<Vec<u8>, Failure> {
    std::fs::read(path).map_err(|e| Error(format!("cannot read {path}: {e}")).into())
}

fn read_text(path: &str) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|e| Error(format!("cannot read {path}: {e}")).into())
}
