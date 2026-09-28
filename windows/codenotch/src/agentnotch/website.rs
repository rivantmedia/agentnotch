//! The sync website this build was made for (DESIGN-WIN §2.4, §4.11): `app-config.json` at the
//! repository root, the same file the Mac build and the release workflow read, compiled in and
//! checked by the engine with the Mac's build-time rules (CL§3.2). An invalid file means "no
//! website", and the engine test that reads the same file fails, so it can't ship unnoticed.

const APP_CONFIG: &str = include_str!("../../../../app-config.json");

pub fn website() -> Option<String> {
    super::engine::website(APP_CONFIG)
}
