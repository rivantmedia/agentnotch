//! WinRT toasts on a real Windows runner (WP6 wp6-8). The CI runner has no Start-menu shortcut
//! carrying the AUMID, so nothing here may depend on a banner being shown: the XML must load into
//! a real `XmlDocument`, the permission must read "unavailable", and posting or withdrawing must
//! return without panicking.
#![cfg(windows)]

use agentnotch_engine::platform::{Notifier, NotifyPermission, Toast, ToastKind};
use agentnotch_win::toast::{toast_xml, Toasts};
use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

fn init_com() {
    // SAFETY: no reserved pointer; S_FALSE / RPC_E_CHANGED_MODE are fine for these calls.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn base(kind: ToastKind) -> Toast {
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

/// The same vectors as the unit tests in `toast.rs`.
fn vectors() -> Vec<Toast> {
    let mut review = base(ToastKind::Review);
    review.tag = "review".into();
    review.actions.push((
        "Mark Reviewed".into(),
        "agentnotch://review?session=s1&completed=1700000000000".into(),
    ));
    let mut subtitled = base(ToastKind::NeedsInput);
    subtitled.subtitle = Some("Work account".into());
    let mut bare = base(ToastKind::Failed);
    bare.actions.clear();
    let specials = Toast {
        tag: "needs".into(),
        group: "s1".into(),
        kind: ToastKind::Review,
        title: "a & b < c > d \" e ' f".into(),
        subtitle: Some("<&>\"'".into()),
        body: "x&y<z>\"'".into(),
        launch_url: "agentnotch://open?session=a&b\"'<>".into(),
        actions: vec![("O&\"'<>k".into(), "agentnotch://r?a=1&b=2".into())],
    };
    let mut unicode = base(ToastKind::Review);
    unicode.title = "Fertig: Übersetzung 日本語 🎉".into();
    unicode.body = "Prêt à relire · café ✅".into();
    let mut controls = base(ToastKind::Failed);
    controls.title = "a\u{0}b\u{8}c\u{1b}d\u{FFFE}e".into();
    vec![
        base(ToastKind::NeedsInput),
        review,
        subtitled,
        bare,
        specials,
        unicode,
        controls,
        base(ToastKind::Limit),
    ]
}

#[test]
fn every_vector_loads_into_a_real_xml_document() {
    init_com();
    for t in vectors() {
        let xml = toast_xml(&t);
        let doc = XmlDocument::new().expect("XmlDocument");
        doc.LoadXml(&HSTRING::from(xml.as_str()))
            .unwrap_or_else(|e| panic!("WinRT refused the XML: {e}\n{xml}"));
    }
}

#[test]
fn permission_is_unavailable_without_a_start_menu_shortcut() {
    assert_eq!(Toasts::new().permission(), NotifyPermission::Unavailable);
}

#[test]
fn post_and_withdraw_return_without_panicking() {
    let toasts = Toasts::new();
    for t in vectors() {
        toasts.post(&t);
        toasts.withdraw(&t.tag, &t.group);
    }
    toasts.withdraw("", "");
}
