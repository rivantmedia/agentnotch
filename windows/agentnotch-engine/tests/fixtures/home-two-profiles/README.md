# home-two-profiles

A sample home folder for `accounts_inspect.rs`: two plain Claude Code profiles
(`.claude-work`, `.claude-personal`), each signed in, with made-up identities
(example.com addresses, random UUIDs) and nothing that resembles a credential.
Plain folders and files only, no links. The test copies it into a temporary
folder and runs the inspector over the copy.

`.claude-work` holds a cached usage reading of its own account (the inspector
reads only its `accountUuid` and `fetchedAtMs`); `.claude-personal`'s cache names
another account, so it must not count for anyone.
