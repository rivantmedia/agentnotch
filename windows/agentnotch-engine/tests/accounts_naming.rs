//! Two accounts never read the same by default: names follow upstream's rule
//! for Claude rings, and badges are told apart by more than colour.
//! The Mac's A2_AccountNamingTests, under both path styles.

mod accounts_support;

use accounts_support::{folder, mac, signed, win, FolderExt};
use agentnotch_engine::accounts::naming::{self, Names};
use agentnotch_engine::accounts::Folder;
use agentnotch_engine::core::paths::Paths;
use std::collections::HashMap;

/// A folder in the home folder, as AccountNamingTests' `account` makes one.
fn account(paths: &Paths, name: &str, email: Option<&str>) -> Folder {
    let dir = paths.join(paths.home(), name);
    let made = match email {
        Some(email) => signed(paths, &dir, Some(email), None),
        None => folder(paths, &dir),
    };
    if name == ".claude" {
        made
    } else {
        made.env(&dir)
    }
}

fn names(paths: &Paths, folders: &[Folder]) -> HashMap<String, Names> {
    naming::assign(folders, paths)
}

fn both() -> [Paths; 2] {
    [mac(), win()]
}

// AccountNamingTests.codenotchsWordForAnAddress
#[test]
fn upstreams_word_for_an_address() {
    assert_eq!(
        naming::domain_label(Some("someone@gmail.com")).as_deref(),
        Some("Gmail")
    );
    assert_eq!(
        naming::domain_label(Some("vinz@acme.co.uk")).as_deref(),
        Some("Acme")
    );
    assert_eq!(
        naming::domain_label(Some("a@b@acme.com")).as_deref(),
        Some("Acme")
    );
    assert_eq!(
        naming::domain_label(Some("someone@IBM.com")).as_deref(),
        Some("IBM")
    );
    for address in [
        None,
        Some(""),
        Some("no-at-sign"),
        Some("someone@"),
        Some("someone@.com"),
        Some("someone@123.45"),
    ] {
        assert_eq!(naming::domain_label(address), None, "{address:?}");
    }
}

// AccountNamingTests.baseNames
#[test]
fn base_names() {
    for paths in both() {
        let p = &paths;
        assert_eq!(
            naming::base_label(&account(p, ".claude", Some("me@gmail.com")), p),
            "Claude Gmail"
        );
        assert_eq!(
            naming::base_label(&account(p, ".claude", None), p),
            "Claude"
        );
        assert_eq!(
            naming::base_label(&account(p, ".claude-work", None), p),
            "Claude (work)"
        );
        assert_eq!(
            naming::base_label(&account(p, ".claude_side", None), p),
            "Claude (side)"
        );
        assert_eq!(
            naming::base_label(&account(p, ".claude-x", Some("odd@123.45")), p),
            "Claude odd@123.45"
        );
    }
    assert_eq!(
        naming::base_label(&folder(&mac(), "/Volumes/X/claude-profile"), &mac()),
        "Claude (claude-profile)"
    );
    assert_eq!(
        naming::base_label(&folder(&win(), r"D:\X\claude-profile"), &win()),
        "Claude (claude-profile)"
    );
    // The default folder has no word of its own, however it is spelled.
    assert_eq!(
        naming::base_label(&folder(&win(), r"c:\users\ME\.Claude"), &win()),
        "Claude"
    );
    assert_eq!(
        naming::base_label(&folder(&win(), r"C:\Users\me\.CLAUDE-Work"), &win()),
        "Claude (Work)"
    );
}

// AccountNamingTests.distinctAccountsKeepTheShortName
#[test]
fn distinct_accounts_keep_the_short_name() {
    for paths in both() {
        let p = &paths;
        let personal = account(p, ".claude", Some("paulo@gmail.com"));
        let work = account(p, ".claude-work", Some("paulo@acme.com"));
        let result = names(p, &[personal.clone(), work.clone()]);
        assert_eq!(result[personal.dir()].label, "Claude Gmail");
        assert_eq!(result[work.dir()].label, "Claude Acme");
        assert_eq!(result[personal.dir()].monogram, "GM");
        assert_eq!(result[work.dir()].monogram, "AC");
    }
}

// AccountNamingTests.sameDomainFallsBackToTheAddress
/// Two gmail logins fall back to the whole address, as upstream does.
#[test]
fn same_domain_falls_back_to_the_address() {
    for paths in both() {
        let p = &paths;
        let one = account(p, ".claude", Some("paulo@gmail.com"));
        let two = account(p, ".claude-work", Some("eureka@gmail.com"));
        let result = names(p, &[one.clone(), two.clone()]);
        assert_eq!(result[one.dir()].label, "Claude paulo@gmail.com");
        assert_eq!(result[two.dir()].label, "Claude eureka@gmail.com");
        assert_eq!(result[one.dir()].monogram, "PA");
        assert_eq!(result[two.dir()].monogram, "EU");
    }
}

// AccountNamingTests.sameAddressInTwoOrganizations
/// The same address in two organizations: the organization tells them apart.
#[test]
fn same_address_in_two_organizations() {
    for paths in both() {
        let p = &paths;
        let personal = account(p, ".claude", Some("me@x.com")).plan("max");
        let team = account(p, ".claude-team", Some("me@x.com"))
            .organization_name("Acme Team")
            .plan("team");
        let result = names(p, &[personal.clone(), team.clone()]);
        assert_eq!(result[team.dir()].label, "Claude me@x.com · Acme Team");
        assert_eq!(result[personal.dir()].label, "Claude me@x.com · Max");
        assert_ne!(result[personal.dir()].monogram, result[team.dir()].monogram);
    }
}

// AccountNamingTests.sameFolderNameElsewhere
/// Nothing but the folder to go on, and even that equal: the path.
#[test]
fn same_folder_name_elsewhere() {
    let p = mac();
    let one = folder(&p, "/Users/me/.claude-work");
    let two = folder(&p, "/Volumes/Backup/.claude-work");
    let result = names(&p, &[one.clone(), two.clone()]);
    assert_eq!(result[one.dir()].label, "Claude (~/.claude-work)");
    assert_eq!(
        result[two.dir()].label,
        "Claude (/Volumes/Backup/.claude-work)"
    );
    assert_ne!(result[one.dir()].monogram, result[two.dir()].monogram);

    let p = win();
    let one = folder(&p, r"C:\Users\me\.claude-work");
    let two = folder(&p, r"D:\Backup\.claude-work");
    let result = names(&p, &[one.clone(), two.clone()]);
    assert_eq!(result[one.dir()].label, r"Claude (~\.claude-work)");
    assert_eq!(result[two.dir()].label, r"Claude (D:\Backup\.claude-work)");
    assert_ne!(result[one.dir()].monogram, result[two.dir()].monogram);
}

// AccountNamingTests.customNamesStayAndTakePart
/// A default name never equals someone's custom one; custom names stay.
#[test]
fn custom_names_stay_and_take_part() {
    for paths in both() {
        let p = &paths;
        let named = account(p, ".claude-a", None).custom("Claude Gmail");
        let plain = account(p, ".claude", Some("me@gmail.com"));
        let result = names(p, &[named.clone(), plain.clone()]);
        assert_eq!(result[named.dir()].label, "Claude Gmail");
        assert_eq!(result[plain.dir()].label, "Claude me@gmail.com");
    }
}

// AccountNamingTests.badgesNeverCollide
#[test]
fn badges_never_collide() {
    for paths in both() {
        let p = &paths;
        let folders: Vec<Folder> = (0..12)
            .map(|n| {
                account(
                    p,
                    &format!(".claude-p{n}"),
                    Some(&format!("person{n}@gmail.com")),
                )
            })
            .collect();
        let result = names(p, &folders);
        let mut monograms: Vec<&str> = folders
            .iter()
            .map(|f| result[f.dir()].monogram.as_str())
            .collect();
        monograms.sort_unstable();
        monograms.dedup();
        assert_eq!(monograms.len(), folders.len());
    }
}

// AccountNamingTests.theRegistryAppliesTheNames
/// The registry names every folder it publishes.
#[test]
fn the_registry_applies_the_names() {
    for paths in both() {
        let p = &paths;
        let named = naming::named(
            &[
                account(p, ".claude", Some("a@gmail.com")),
                account(p, ".claude-w", Some("b@gmail.com")),
            ],
            p,
        );
        let mut labels: Vec<String> = named.iter().map(|f| f.label(p)).collect();
        labels.sort();
        assert_eq!(labels, ["Claude a@gmail.com", "Claude b@gmail.com"]);
        assert_ne!(named[0].monogram(p), named[1].monogram(p));
        // An untracked account is named as if it joined the tracked ones.
        let hidden = account(p, ".claude-h", Some("c@gmail.com")).hidden();
        let with_hidden = naming::named(&[account(p, ".claude", Some("a@acme.com")), hidden], p);
        let untracked = with_hidden.iter().find(|f| f.is_hidden).unwrap();
        assert_eq!(untracked.label(p), "Claude Gmail");
    }
}

/// Fix_AccountOwnLabelTests.pathsAreAbbreviatedOneWay.
#[test]
fn paths_are_abbreviated_one_way() {
    let m = mac();
    assert_eq!(m.abbreviate("/Users/me/.claude"), "~/.claude");
    assert_eq!(m.abbreviate("/Users/me/"), "~");
    assert_eq!(m.abbreviate("/Users/meow/.claude"), "/Users/meow/.claude");
    let w = win();
    assert_eq!(w.abbreviate(r"C:\Users\me\.claude"), r"~\.claude");
    assert_eq!(w.abbreviate(r"c:\users\ME\"), "~");
    assert_eq!(
        w.abbreviate(r"C:\Users\meow\.claude"),
        r"C:\Users\meow\.claude"
    );
}
