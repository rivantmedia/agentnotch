//! Where the sessions panel goes (port of `ClaudePanelGeometry.swift`): beside the notch window,
//! on the side facing the screen, its tail pointing at the ring that was clicked; or floating
//! under the top of the work area when there is no ring to hang off.
//!
//! Pure, so the glue places the panel without owning a rule. Every rect is in **physical pixels,
//! origin top-left, y down** (what Win32 hands out); the Mac's y-up points are translated here,
//! once. The design constants below are CSS px; they are multiplied by the landing monitor's
//! scale and rounded to whole pixels first, so everything after that is integer arithmetic and
//! the window is exactly the card plus a whole-pixel tail strip.
//!
//! The panel window is the card plus its tail strip on the notch side. The card is centred on the
//! ring along the edge and kept inside the work area; when it has to move to stay there, the tail
//! slides along the card to keep pointing at the ring, but never into its rounded corners.
//!
//! `scale` and `work_area` are those of the monitor the panel lands on. The ring rect is in
//! virtual-desktop pixels and may sit on another monitor (mixed DPI): sizes follow `scale`, never
//! the ring's own monitor.
//!
//! Shared-file note: `geometry/**` is WP7's; this file was added by WP9 (M1 needs placement
//! before WP7 exists). The tail (32 x 36), corner (16) and margin (8) are the Windows values, not
//! the Mac's 28.2 / 32.7 / 18.6.

use serde::{Deserialize, Serialize};

/// The least distance between the card and the work area's edges (CSS px).
pub const MARGIN_CSS: f64 = 8.0;
/// How far the tail reaches out from the card (CSS px).
pub const TAIL_LENGTH_CSS: f64 = 32.0;
/// How wide the tail is where it leaves the card (CSS px).
pub const TAIL_WIDTH_CSS: f64 = 36.0;
/// The card's corner radius (CSS px).
pub const CORNER_CSS: f64 = 16.0;
/// The shortest the card gets, whatever the content asks for (CSS px).
pub const MINIMUM_HEIGHT_CSS: f64 = 220.0;
/// The narrowest a card is squeezed to on a small work area (CSS px).
pub const MINIMUM_WIDTH_CSS: f64 = 200.0;
/// Bezel to tail tip on an upright notch (CSS px, before the notch Size zoom): a hint for the
/// glue's [`fallback_ring`] when the page sent no ring rect.
pub const UPRIGHT_TIP_INSET_CSS: f64 = 69.0;

/// The screen edge the notch is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelEdge {
    Right,
    Left,
    Top,
    Bottom,
}

/// What the panel shows: the session list or one chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelMode {
    List,
    Chat,
}

/// A rect in physical pixels, y down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    /// True when `inner` lies wholly inside `self`.
    pub const fn contains(&self, inner: &PxRect) -> bool {
        inner.x >= self.x
            && inner.y >= self.y
            && inner.right() <= self.right()
            && inner.bottom() <= self.bottom()
    }
}

/// Everything the placement depends on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PanelInput {
    /// The notch's edge; `None` places the panel floating.
    pub edge: Option<PanelEdge>,
    /// The ring the tail points at, in virtual-desktop pixels; `None` places it floating.
    /// The tail's tip lands on the ring's side that faces the screen (its left side for a
    /// right-hand notch, its bottom for a top one, and so on), level with its centre.
    pub ring: Option<PxRect>,
    /// The work area of the monitor the panel lands on.
    pub work_area: PxRect,
    /// That monitor's scale (1.0 = 100 %). Not finite or not positive counts as 1.0.
    pub scale: f64,
    pub mode: PanelMode,
    /// The content height the page wants, in CSS px. Not finite counts as the minimum.
    pub content_height_css: f64,
}

/// Where the panel goes and what the page must draw.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PanelPlacement {
    /// The panel window in physical pixels: the card plus the tail strip.
    pub window: PxRect,
    /// The card in physical pixels.
    pub card: PxRect,
    /// The card's width in CSS px.
    pub width_css: f64,
    /// The tallest the card may grow, in CSS px (`panel_report_size` clamps to it).
    pub max_height_css: f64,
    /// The tail's offset from the card's centre along the card's edge, in CSS px, the way the
    /// page measures it: downwards on the side edges, rightwards on top and bottom.
    pub tail_offset: f64,
    /// The most the tail may be offset: `cardLength / 2 - corner - tailWidth / 2`, in CSS px.
    pub tail_limit: f64,
    /// The tail strip's length in physical pixels (0 when floating).
    pub tail_length: i32,
    pub floating: bool,
}

/// The card's width in CSS px: narrower beside a side notch, where it eats into the screen's
/// width, and wider for the chat, which reads better with room.
pub fn width_css(edge: Option<PanelEdge>, mode: PanelMode) -> f64 {
    match (edge, mode) {
        (Some(PanelEdge::Right | PanelEdge::Left), PanelMode::List) => 400.0,
        (Some(PanelEdge::Right | PanelEdge::Left), PanelMode::Chat) => 440.0,
        (_, PanelMode::List) => 440.0,
        (_, PanelMode::Chat) => 520.0,
    }
}

/// The tallest the card gets before the work area has a say, in CSS px.
pub fn height_cap_css(mode: PanelMode) -> f64 {
    match mode {
        PanelMode::List => 680.0,
        PanelMode::Chat => 780.0,
    }
}

/// `±(cardLength/2 - corner - tailWidth/2)`: the tail never runs into a rounded corner. CSS px.
pub fn tail_offset_limit(card_length_css: f64) -> f64 {
    (card_length_css / 2.0 - CORNER_CSS - TAIL_WIDTH_CSS / 2.0).max(0.0)
}

/// The anchor for a page that sent no ring rect: a point (a zero-size rect) in the middle of the
/// notch window's side that touches the screen edge, moved `inset_css` (times `scale`) in towards
/// the screen. With `inset_css` 0 it is exactly that side's middle; the glue passes
/// [`UPRIGHT_TIP_INSET_CSS`] times the notch Size zoom so the tail's tip lands where the ring's
/// inner side would be.
pub fn fallback_ring(edge: PanelEdge, notch_window: PxRect, scale: f64, inset_css: f64) -> PxRect {
    let inset = px(inset_css.max(0.0), sane_scale(scale));
    let mid_x = notch_window.x + notch_window.w / 2;
    let mid_y = notch_window.y + notch_window.h / 2;
    let (x, y) = match edge {
        PanelEdge::Right => (notch_window.right() - inset, mid_y),
        PanelEdge::Left => (notch_window.x + inset, mid_y),
        PanelEdge::Top => (mid_x, notch_window.y + inset),
        PanelEdge::Bottom => (mid_x, notch_window.bottom() - inset),
    };
    PxRect::new(x, y, 0, 0)
}

/// Where the drawn tail's tip lands in physical pixels; `None` without a tail. The inverse of
/// the placement, for tests and diagnostics.
pub fn tail_tip(placement: &PanelPlacement, edge: PanelEdge, scale: f64) -> Option<(f64, f64)> {
    if placement.floating {
        return None;
    }
    let scale = sane_scale(scale);
    let card = placement.card;
    let window = placement.window;
    let across = placement.tail_offset * scale;
    let mid_x = f64::from(card.x) + f64::from(card.w) / 2.0;
    let mid_y = f64::from(card.y) + f64::from(card.h) / 2.0;
    Some(match edge {
        PanelEdge::Right => (f64::from(window.right()), mid_y + across),
        PanelEdge::Left => (f64::from(window.x), mid_y + across),
        PanelEdge::Top => (mid_x + across, f64::from(window.y)),
        PanelEdge::Bottom => (mid_x + across, f64::from(window.bottom())),
    })
}

/// Places the panel.
pub fn place(input: &PanelInput) -> PanelPlacement {
    let scale = sane_scale(input.scale);
    let m = Metrics::new(scale, input.mode);
    // A height reported mid-transition can be NaN; it must not reach a window rect.
    let ideal = if input.content_height_css.is_finite() {
        px(input.content_height_css.max(0.0), scale)
    } else {
        m.min_h
    };
    match (input.edge, input.ring) {
        (Some(edge @ (PanelEdge::Right | PanelEdge::Left)), Some(ring)) => {
            beside(edge, ring, input.work_area, &m, ideal)
        }
        (Some(edge @ (PanelEdge::Top | PanelEdge::Bottom)), Some(ring)) => {
            above_or_below(edge, ring, input.work_area, &m, ideal)
        }
        _ => floating(input.work_area, &m, ideal),
    }
}

// MARK: - Layouts

/// Right and left: the card beside the notch, centred on the ring's y.
fn beside(edge: PanelEdge, ring: PxRect, work: PxRect, m: &Metrics, ideal: i32) -> PanelPlacement {
    let (rx, ry) = ring_point(edge, ring);
    let is_right = edge == PanelEdge::Right;

    // The card's edge nearest the notch: at the tail's root, or pulled back inside the work
    // area when the notch sits over something (a taskbar on that side).
    let root = if is_right {
        (rx - m.tail).min(work.right() - m.margin)
    } else {
        (rx + m.tail).max(work.x + m.margin)
    };
    // Never wider than the space between the notch and the far side.
    let room = if is_right {
        root - (work.x + m.margin)
    } else {
        (work.right() - m.margin) - root
    };
    let width = m.min_w.max(m.side_width.min(room));

    let max_h = m.min_h.max(m.cap.min(work.h - 2 * m.margin));
    let height = clamp(ideal, m.min_h, max_h);
    // Centred on the ring, inside the work area; a card taller than the work area (only below
    // the minimum height) keeps its top on it.
    let y = if height > work.h - 2 * m.margin {
        work.y + m.margin
    } else {
        clamp(
            ry - height / 2,
            work.y + m.margin,
            work.bottom() - m.margin - height,
        )
    };
    let card = PxRect::new(if is_right { root - width } else { root }, y, width, height);

    // Measured downwards from the card's centre; screen y grows downwards here too.
    let offset = f64::from(ry) - (f64::from(card.y) + f64::from(card.h) / 2.0);
    let window = if is_right {
        PxRect::new(card.x, card.y, width + m.tail, height)
    } else {
        PxRect::new(card.x - m.tail, card.y, width + m.tail, height)
    };
    finish(window, card, offset, false, max_h, height, m)
}

/// Top and bottom: the card below or above the notch, centred on the ring's x.
fn above_or_below(
    edge: PanelEdge,
    ring: PxRect,
    work: PxRect,
    m: &Metrics,
    ideal: i32,
) -> PanelPlacement {
    let (rx, ry) = ring_point(edge, ring);
    let is_top = edge == PanelEdge::Top;

    let width = m.min_w.max(m.flat_width.min(work.w - 2 * m.margin));
    let x = clamp(
        rx - width / 2,
        work.x + m.margin,
        work.right() - m.margin - width,
    );

    let (card, max_h);
    if is_top {
        // The notch is at the top: the card hangs below the tail.
        let top = (ry + m.tail).max(work.y + m.margin);
        max_h = m.min_h.max(m.cap.min(work.bottom() - top - m.margin));
        let height = clamp(ideal, m.min_h, max_h);
        card = PxRect::new(x, top, width, height);
    } else {
        let bottom = (ry - m.tail).min(work.bottom() - m.margin);
        max_h = m.min_h.max(m.cap.min(bottom - work.y - m.margin));
        let height = clamp(ideal, m.min_h, max_h);
        card = PxRect::new(x, bottom - height, width, height);
    }

    let offset = f64::from(rx) - (f64::from(card.x) + f64::from(card.w) / 2.0);
    let window = if is_top {
        PxRect::new(card.x, card.y - m.tail, width, card.h + m.tail)
    } else {
        PxRect::new(card.x, card.y, width, card.h + m.tail)
    };
    finish(window, card, offset, false, max_h, width, m)
}

/// No notch to hang off (hidden, the ring is off, or no rect is known): a plain card at the
/// top-centre of the work area of the monitor the pointer is on. As on the Mac, the height cap
/// is the work area alone (no 680 / 780), and the width has no minimum.
fn floating(work: PxRect, m: &Metrics, ideal: i32) -> PanelPlacement {
    let width = 0.max(m.flat_width.min(work.w - 2 * m.margin));
    let max_h = m.min_h.max(work.h - 2 * m.margin);
    let height = clamp(ideal, m.min_h, max_h);
    let card = PxRect::new(
        work.x + (work.w - width).div_euclid(2),
        work.y + m.margin,
        width,
        height,
    );
    let mut placement = finish(card, card, 0.0, true, max_h, width, m);
    placement.tail_length = 0;
    placement.tail_limit = 0.0;
    placement
}

// MARK: - Helpers

/// The design constants in whole physical pixels for one monitor scale.
struct Metrics {
    scale: f64,
    margin: i32,
    tail: i32,
    min_w: i32,
    min_h: i32,
    cap: i32,
    /// The card's width beside a side notch, and on a flat edge (which floating shares).
    side_width: i32,
    flat_width: i32,
}

impl Metrics {
    fn new(scale: f64, mode: PanelMode) -> Self {
        Metrics {
            scale,
            margin: px(MARGIN_CSS, scale),
            tail: px(TAIL_LENGTH_CSS, scale),
            min_w: px(MINIMUM_WIDTH_CSS, scale),
            min_h: px(MINIMUM_HEIGHT_CSS, scale),
            cap: px(height_cap_css(mode), scale),
            side_width: px(width_css(Some(PanelEdge::Right), mode), scale),
            flat_width: px(width_css(None, mode), scale),
        }
    }
}

fn finish(
    window: PxRect,
    card: PxRect,
    offset_px: f64,
    floating: bool,
    max_h_px: i32,
    card_length_px: i32,
    m: &Metrics,
) -> PanelPlacement {
    let scale = m.scale;
    let limit = tail_offset_limit(f64::from(card_length_px) / scale);
    PanelPlacement {
        window,
        card,
        width_css: f64::from(card.w) / scale,
        max_height_css: f64::from(max_h_px) / scale,
        tail_offset: (offset_px / scale).clamp(-limit, limit),
        tail_limit: limit,
        tail_length: m.tail,
        floating,
    }
}

/// The ring the tail points at: level with its centre along the notch, and on its side facing
/// the screen across it.
fn ring_point(edge: PanelEdge, ring: PxRect) -> (i32, i32) {
    let cx = ring.x + ring.w / 2;
    let cy = ring.y + ring.h / 2;
    match edge {
        PanelEdge::Right => (ring.x, cy),
        PanelEdge::Left => (ring.right(), cy),
        PanelEdge::Top => (cx, ring.bottom()),
        PanelEdge::Bottom => (cx, ring.y),
    }
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// CSS px to whole physical pixels (saturating at the i32 range).
fn px(css: f64, scale: f64) -> i32 {
    (css * scale).round() as i32
}

/// `value` within `low..=high`; `low` wins when the range is empty.
fn clamp(value: i32, low: i32, high: i32) -> i32 {
    if low > high {
        low
    } else {
        value.max(low).min(high)
    }
}
