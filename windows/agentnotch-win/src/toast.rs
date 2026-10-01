//! Windows notifications (DESIGN-WIN §3.2 `Notifier`, §4.10; WP6): WinRT toasts through
//! `CreateToastNotifierWithId("com.rivantmedia.agentnotch")`, silent, protocol activation
//! (`agentnotch://open?…`), replaced and withdrawn by tag and group.
//!
//! The XML ([`toast_xml`]) is built on every system so its vectors run on the Mac; the WinRT half
//! (`Toasts`) is Windows only. Whether a banner should be posted at all (permission, looking at
//! the session, full screen) is the engine's decision: `post` just tries, and every failure is
//! swallowed, because a missed banner must never take the hub down or be retried into a storm.

use agentnotch_engine::platform::Toast;

/// Escapes text for an XML attribute or element. Session titles, project names, prompts and
/// URLs all come from outside, so all five predefined entities are written, and characters XML
/// 1.0 forbids (most C0 controls, lone surrogates cannot occur in a `str`, U+FFFE/FFFF) are
/// dropped: WinRT's `LoadXml` refuses a document that holds one, and the banner would be lost.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The toast's XML: protocol activation everywhere (no COM activator), a generic binding with
/// the title, the optional subtitle and the body, an `<actions>` block only when the toast has
/// buttons, and exactly one `<audio silent="true"/>` (the app chimes itself).
pub fn toast_xml(t: &Toast) -> String {
    let mut xml = format!(
        "<toast launch=\"{}\" activationType=\"protocol\"><visual><binding template=\"ToastGeneric\"><text>{}</text>",
        escape(&t.launch_url),
        escape(&t.title)
    );
    if let Some(sub) = t.subtitle.as_deref().filter(|s| !s.is_empty()) {
        xml.push_str(&format!("<text>{}</text>", escape(sub)));
    }
    xml.push_str(&format!(
        "<text>{}</text></binding></visual>",
        escape(&t.body)
    ));
    if !t.actions.is_empty() {
        xml.push_str("<actions>");
        for (label, url) in &t.actions {
            xml.push_str(&format!(
                "<action content=\"{}\" activationType=\"protocol\" arguments=\"{}\"/>",
                escape(label),
                escape(url)
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("<audio silent=\"true\"/></toast>");
    xml
}

#[cfg(windows)]
pub use real::Toasts;

#[cfg(windows)]
mod real {
    use super::toast_xml;
    use agentnotch_engine::core::roots::IDENTIFIER;
    use agentnotch_engine::platform::{Notifier, NotifyPermission, Toast};
    use windows::core::{Result, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::UI::Notifications::{
        NotificationSetting, ToastNotification, ToastNotificationManager,
    };

    #[derive(Debug, Default)]
    pub struct Toasts;

    impl Toasts {
        pub fn new() -> Self {
            Toasts
        }
    }

    /// WinRT needs COM on the calling thread. The result is ignored on purpose: S_FALSE (already
    /// initialised) and RPC_E_CHANGED_MODE (the thread chose another apartment) both leave COM
    /// usable for these calls, and anything worse surfaces as the call's own error below.
    fn init_com() {
        // SAFETY: no reserved pointer; a matching CoUninitialize is not needed, the thread keeps
        // COM for its life like every thread the hub's UI lane runs on.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    pub(super) fn try_post(t: &Toast) -> Result<()> {
        init_com();
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(toast_xml(t)))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.SetTag(&HSTRING::from(t.tag.as_str()))?;
        toast.SetGroup(&HSTRING::from(t.group.as_str()))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(IDENTIFIER))?
            .Show(&toast)
    }

    pub(super) fn try_withdraw(tag: &str, group: &str) -> Result<()> {
        init_com();
        ToastNotificationManager::History()?.RemoveGroupedTagWithId(
            &HSTRING::from(tag),
            &HSTRING::from(group),
            &HSTRING::from(IDENTIFIER),
        )
    }

    pub(super) fn try_permission() -> Result<NotifyPermission> {
        init_com();
        let setting =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(IDENTIFIER))?
                .Setting()?;
        Ok(if setting == NotificationSetting::Enabled {
            NotifyPermission::Allowed
        } else if setting == NotificationSetting::DisabledForApplication {
            NotifyPermission::DisabledForApp
        } else if setting == NotificationSetting::DisabledForUser {
            NotifyPermission::DisabledForUser
        } else if setting == NotificationSetting::DisabledByGroupPolicy {
            NotifyPermission::DisabledByPolicy
        } else {
            // DisabledByManifest (a package manifest we do not have) or a value Windows adds
            // later: no banners either way, and the Settings row says so.
            NotifyPermission::Unavailable
        })
    }

    impl Notifier for Toasts {
        fn post(&self, t: &Toast) {
            let _ = try_post(t);
        }

        fn withdraw(&self, tag: &str, group: &str) {
            let _ = try_withdraw(tag, group);
        }

        /// No Start-menu shortcut carries the AUMID (a dev or portable run): the notifier or its
        /// setting cannot be read, and that is `Unavailable`, not an error to show.
        fn permission(&self) -> NotifyPermission {
            try_permission().unwrap_or(NotifyPermission::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentnotch_engine::platform::ToastKind;

    fn toast(kind: ToastKind) -> Toast {
        Toast {
            tag: "needs".into(),
            group: "s1".into(),
            kind,
            title: "Fix the build".into(),
            subtitle: None,
            body: "Needs your permission".into(),
            launch_url: "agentnotch://open?session=s1".into(),
            actions: vec![("Open".into(), "agentnotch://open?session=s1".into())],
        }
    }

    fn count(xml: &str, needle: &str) -> usize {
        xml.matches(needle).count()
    }

    #[test]
    fn a_needs_you_toast_has_open_only() {
        let xml = toast_xml(&toast(ToastKind::NeedsInput));
        assert_eq!(
            xml,
            "<toast launch=\"agentnotch://open?session=s1\" activationType=\"protocol\">\
             <visual><binding template=\"ToastGeneric\"><text>Fix the build</text>\
             <text>Needs your permission</text></binding></visual>\
             <actions><action content=\"Open\" activationType=\"protocol\" \
             arguments=\"agentnotch://open?session=s1\"/></actions>\
             <audio silent=\"true\"/></toast>"
        );
    }

    #[test]
    fn a_review_toast_has_open_and_mark_reviewed() {
        let mut t = toast(ToastKind::Review);
        t.actions.push((
            "Mark Reviewed".into(),
            "agentnotch://review?session=s1&completed=1700000000000".into(),
        ));
        let xml = toast_xml(&t);
        assert_eq!(count(&xml, "<action "), 2);
        assert!(xml.contains("content=\"Open\""));
        assert!(xml.contains("content=\"Mark Reviewed\""));
        // The review URL's `&` is escaped so the attribute stays well-formed.
        assert!(xml
            .contains("arguments=\"agentnotch://review?session=s1&amp;completed=1700000000000\""));
        assert!(!xml.contains("&completed"));
        assert!(xml.find("Open").unwrap() < xml.find("Mark Reviewed").unwrap());
    }

    #[test]
    fn no_actions_means_no_actions_element() {
        let mut t = toast(ToastKind::Failed);
        t.actions.clear();
        let xml = toast_xml(&t);
        assert!(!xml.contains("<actions>"));
        assert!(!xml.contains("<action "));
        assert!(xml.ends_with("</visual><audio silent=\"true\"/></toast>"));
    }

    #[test]
    fn the_subtitle_is_the_second_text_when_present() {
        let mut t = toast(ToastKind::NeedsInput);
        t.subtitle = Some("Work account".into());
        let xml = toast_xml(&t);
        assert_eq!(count(&xml, "<text>"), 3);
        let title = xml.find("<text>Fix the build</text>").unwrap();
        let sub = xml.find("<text>Work account</text>").unwrap();
        let body = xml.find("<text>Needs your permission</text>").unwrap();
        assert!(title < sub && sub < body);
    }

    #[test]
    fn an_absent_or_empty_subtitle_adds_no_text() {
        for sub in [None, Some(String::new())] {
            let mut t = toast(ToastKind::NeedsInput);
            t.subtitle = sub;
            assert_eq!(count(&toast_xml(&t), "<text>"), 2);
        }
    }

    #[test]
    fn the_five_specials_are_escaped_everywhere() {
        let t = Toast {
            tag: "needs".into(),
            group: "s1".into(),
            kind: ToastKind::Review,
            title: "a & b < c > d \" e ' f".into(),
            subtitle: Some("<&>\"'".into()),
            body: "x&y<z>\"'".into(),
            launch_url: "agentnotch://open?session=a&b\"'<>".into(),
            actions: vec![("O&\"'<>k".into(), "agentnotch://r?a=1&b=2".into())],
        };
        let xml = toast_xml(&t);
        assert!(xml.contains("<text>a &amp; b &lt; c &gt; d &quot; e &apos; f</text>"));
        assert!(xml.contains("<text>&lt;&amp;&gt;&quot;&apos;</text>"));
        assert!(xml.contains("<text>x&amp;y&lt;z&gt;&quot;&apos;</text>"));
        assert!(xml.contains("launch=\"agentnotch://open?session=a&amp;b&quot;&apos;&lt;&gt;\""));
        assert!(xml.contains("content=\"O&amp;&quot;&apos;&lt;&gt;k\""));
        assert!(xml.contains("arguments=\"agentnotch://r?a=1&amp;b=2\""));
        // Nothing raw is left between the tags the builder wrote itself.
        let stripped = xml
            .replace("&amp;", "")
            .replace("&lt;", "")
            .replace("&gt;", "");
        let stripped = stripped.replace("&quot;", "").replace("&apos;", "");
        assert!(!stripped.contains('&'));
    }

    #[test]
    fn non_ascii_and_emoji_are_kept() {
        let mut t = toast(ToastKind::Review);
        t.title = "Fertig: Übersetzung 日本語 🎉".into();
        t.body = "Prêt à relire · café ✅".into();
        let xml = toast_xml(&t);
        assert!(xml.contains("<text>Fertig: Übersetzung 日本語 🎉</text>"));
        assert!(xml.contains("<text>Prêt à relire · café ✅</text>"));
    }

    #[test]
    fn characters_xml_forbids_are_dropped() {
        let mut t = toast(ToastKind::Failed);
        t.title = "a\u{0}b\u{8}c\u{1b}d\u{FFFE}e".into();
        t.body = "line1\nline2\ttab".into();
        let xml = toast_xml(&t);
        assert!(xml.contains("<text>abcde</text>"));
        assert!(xml.contains("line1\nline2\ttab"));
    }

    #[test]
    fn exactly_one_silent_audio_and_every_action_is_protocol() {
        for kind in [
            ToastKind::NeedsInput,
            ToastKind::Review,
            ToastKind::Failed,
            ToastKind::Limit,
        ] {
            let mut t = toast(kind);
            t.actions.push((
                "Mark Reviewed".into(),
                "agentnotch://review?session=s1".into(),
            ));
            let xml = toast_xml(&t);
            assert_eq!(count(&xml, "<audio "), 1);
            assert_eq!(count(&xml, "<audio silent=\"true\"/>"), 1);
            assert_eq!(count(&xml, "<action "), count(&xml, "<action content="));
            assert_eq!(
                count(&xml, "activationType=\"protocol\""),
                1 + count(&xml, "<action ")
            );
            assert!(!xml.contains("activationType=\"foreground\""));
            assert!(!xml.contains("activationType=\"background\""));
            assert!(xml.starts_with("<toast launch=\""));
            assert!(xml.ends_with("</toast>"));
        }
    }
}
