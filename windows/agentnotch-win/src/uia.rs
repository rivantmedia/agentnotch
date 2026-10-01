//! Windows Terminal tabs through UI Automation (DESIGN-WIN §4.9; WP6): `IUIAutomation2` with 2 s
//! connection and transaction timeouts, the TabItem names of one WT window and selecting the one
//! that matches exactly.
//!
//! Only a window of class `CASCADIA_HOSTING_WINDOW_CLASS` is ever walked: another app's tabs are
//! never read or selected. Which tab is the session's is the engine's decision
//! (`control::focus::wt_tab_match`); this module only reads the names and presses the one it
//! names. Everything fails soft: an elevated Windows Terminal refuses UIA from a non-elevated app
//! (UIPI), a busy one times out after 2 s, a tab can close between the read and the select. Each
//! of those is `None` / `TabSelect::Failed`, which the plan turns into "raise the window only".
//! No retries and no fuzzy matching (R5).
//!
//! COM is initialised per call on the calling thread (`an-ui`, which may be a different thread
//! from one call to the next) and every COM pointer is local to the call: none is cached, so none
//! crosses threads or outlives its apartment.

#![cfg(windows)]

use std::marker::PhantomData;

use agentnotch_engine::control::focus::{wt_tab_match, TabMatch};
use agentnotch_engine::control::hosts::WT_WINDOW_CLASS;
use windows::core::Result;
use windows::Win32::Foundation::{E_INVALIDARG, HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation2, IUIAutomationElement, IUIAutomationElementArray,
    IUIAutomationSelectionItemPattern, TreeScope_Descendants, UIA_ControlTypePropertyId,
    UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

/// How long UIA waits to connect to Windows Terminal, and for any one call into it, in
/// milliseconds: a hung or busy terminal costs the user's click at most this (DESIGN-WIN §4.9).
const TIMEOUT_MS: u32 = 2000;

/// What selecting a session's tab came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabSelect {
    /// Exactly one tab carried the title, and it is now the selected tab.
    Selected,
    /// No tab carries the title (renamed tab, `suppressApplicationTitle`, a pane).
    NoMatch,
    /// Several tabs carry it: none was touched.
    Ambiguous,
    /// Not a Windows Terminal window, or UI Automation failed (access denied against an elevated
    /// terminal, a timeout, the window or tab gone); the reason is for the log.
    Failed(String),
}

/// The tabs of the Windows Terminal window `window` (an `HWND` as `u64`), as (name, selected) in
/// UI Automation's order, which is the tab strip's order.
///
/// `None` when `window` isn't a Windows Terminal window or UI Automation fails. A tab without a
/// SelectionItem pattern counts as not selected.
pub fn tab_titles(window: u64) -> Option<Vec<(String, bool)>> {
    let hwnd = wt_window(window).ok()?;
    let com = ComGuard::new().ok()?;
    // Every COM object made by `read_tabs` is dropped by the end of this statement.
    let titles = read_tabs(hwnd).map(|(_, titles)| titles).ok();
    drop(com);
    titles
}

/// Selects the tab of the Windows Terminal window `window` whose name is exactly `title`
/// (trimmed; `wt_tab_match`). Never guesses: no tab or several tabs leave the window as it is.
pub fn select_tab(window: u64, title: &str) -> TabSelect {
    let hwnd = match wt_window(window) {
        Ok(hwnd) => hwnd,
        Err(reason) => return TabSelect::Failed(reason),
    };
    let com = match ComGuard::new() {
        Ok(guard) => guard,
        Err(error) => return TabSelect::Failed(format!("COM is unavailable: {error}")),
    };
    // Every COM object is made and dropped inside this match, before the guard uninitialises.
    let outcome = match read_tabs(hwnd) {
        Err(error) => TabSelect::Failed(format!("UI Automation failed: {error}")),
        Ok((elements, titles)) => match wt_tab_match(&titles, title) {
            TabMatch::None => TabSelect::NoMatch,
            TabMatch::Ambiguous => TabSelect::Ambiguous,
            TabMatch::One(index) => match select_element(&elements, index) {
                Ok(()) => TabSelect::Selected,
                Err(error) => TabSelect::Failed(format!("selecting the tab failed: {error}")),
            },
        },
    };
    drop(com);
    outcome
}

/// `window` as an `HWND`, when it is a Windows Terminal window; otherwise why not.
fn wt_window(window: u64) -> std::result::Result<HWND, String> {
    if window == 0 {
        return Err("no window".to_string());
    }
    let hwnd = HWND(window as usize as *mut core::ffi::c_void);
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is a valid slice for the call; a stale handle just returns 0.
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    let class = String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)]);
    if class == WT_WINDOW_CLASS {
        Ok(hwnd)
    } else {
        Err(format!("not a Windows Terminal window (class {class:?})"))
    }
}

/// The window's TabItem elements, with their (name, selected) in the same order.
fn read_tabs(hwnd: HWND) -> Result<(IUIAutomationElementArray, Vec<(String, bool)>)> {
    // SAFETY: plain COM calls on interfaces this function owns; COM is initialised on this
    // thread by the caller's guard, which outlives every object made here.
    unsafe {
        let automation: IUIAutomation2 =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
        // Before any call that reaches into Windows Terminal's process.
        automation.SetConnectionTimeout(TIMEOUT_MS)?;
        automation.SetTransactionTimeout(TIMEOUT_MS)?;
        let root = automation.ElementFromHandle(hwnd)?;
        let condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_TabItemControlTypeId.0),
        )?;
        let elements = root.FindAll(TreeScope_Descendants, &condition)?;
        let count = elements.Length()?;
        let mut titles = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        for index in 0..count {
            let element = elements.GetElement(index)?;
            let name = element.CurrentName()?.to_string();
            titles.push((name, is_selected(&element)));
        }
        Ok((elements, titles))
    }
}

/// Whether a tab is the selected one; a tab without the SelectionItem pattern (or one that
/// fails to answer) is not.
fn is_selected(element: &IUIAutomationElement) -> bool {
    // SAFETY: plain COM calls on a live element; COM is initialised by the caller's guard.
    unsafe {
        element
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
            .and_then(|pattern| pattern.CurrentIsSelected())
            .map(|selected| selected.as_bool())
            .unwrap_or(false)
    }
}

/// Presses element `index`'s SelectionItem pattern.
fn select_element(elements: &IUIAutomationElementArray, index: usize) -> Result<()> {
    let index = i32::try_from(index).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
    // SAFETY: plain COM calls on interfaces this function owns; COM is initialised by the
    // caller's guard.
    unsafe {
        let element = elements.GetElement(index)?;
        let pattern = element
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)?;
        pattern.Select()
    }
}

/// COM on this thread for one call. Uninitialises on drop only when its own `CoInitializeEx`
/// succeeded (S_OK, or S_FALSE when the thread already was in the multithreaded apartment: each
/// of those counts and needs its matching `CoUninitialize`). A thread already in a
/// single-threaded apartment (`RPC_E_CHANGED_MODE`) can still use UIA, but this call didn't
/// initialise it and must not uninitialise it.
struct ComGuard {
    initialised: bool,
    /// Not `Send`: the matching `CoUninitialize` must run on the thread that initialised.
    _thread_bound: PhantomData<*const ()>,
}

impl ComGuard {
    fn new() -> Result<Self> {
        // SAFETY: no reserved pointer; balanced by Drop when it succeeded.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let initialised = if result.is_ok() {
            true
        } else if result == RPC_E_CHANGED_MODE {
            false
        } else {
            return Err(result.into());
        };
        Ok(Self {
            initialised,
            _thread_bound: PhantomData,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialised {
            // SAFETY: matches this guard's own successful CoInitializeEx on this thread (the
            // guard is not Send, so it is dropped where it was made).
            unsafe { CoUninitialize() };
        }
    }
}
