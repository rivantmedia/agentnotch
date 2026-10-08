//! The fork's mark on upstream's tray icon (DESIGN-WIN §2.4 `tray.rs`, §4.10): the needs-you
//! count in the tooltip ("Agent Notch — 2 need you") and a dot drawn on upstream's image while
//! something needs the user and the `trayBadge` setting is on. Windows' tray has no room for a
//! digit, so the number lives in the tooltip.
//!
//! The dot is drawn on the pixels of upstream's own icon (`trayicon::app_mark`), never on a copy
//! of the file, and the plain icon comes back when the count returns to zero. Upstream's tray code
//! is not edited: it rebuilds the tooltip from its readings ("Claude 34%") when it has some, and
//! that text does not carry the count. The count is put back whenever it changes; it is not
//! re-applied after upstream's own rebuild, which would need a hook in `src/tray.rs`.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

/// The dot's fill: the theme's needs-you colour on the dark notch (`--an-needs-you` in
/// `ui/agentnotch/theme.css`), so the tray and the notch's marks read as one thing.
const DOT: [u8; 3] = [0xF2, 0xFF, 0x00];
/// The dot's 1 px outline, so it stays visible on a light taskbar and a pale icon.
const RING: [u8; 3] = [0x10, 0x10, 0x10];
/// The disc's diameter as a share of the icon's smaller side.
const DIAMETER: f64 = 0.4;

/// Whether the setting `trayBadge` is on, as the hub last said (the default is on).
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Whether the tray now carries the dot (touched on the main thread only).
static DRAWN: AtomicBool = AtomicBool::new(false);

/// The hub's `TrayBadge(count)`.
pub(super) fn set_badge(app: &AppHandle, count: u32) {
    if super::NEEDS_YOU.swap(count, Ordering::Relaxed) == count {
        return;
    }
    refresh(app);
}

/// The tray was built again (a failed update's cleanup dropped it): the new icon is plain.
pub(super) fn rebuilt(app: &AppHandle) {
    DRAWN.store(false, Ordering::Relaxed);
    refresh(app);
}

/// The setting `trayBadge` (from every settings event).
pub(super) fn set_dot_enabled(app: &AppHandle, on: bool) {
    if ENABLED.swap(on, Ordering::Relaxed) != on {
        refresh(app);
    }
}

/// Puts the count in the tooltip and draws or removes the dot, on the main thread (the tray is a
/// window-system object). Posted, never waited for: `set_badge` runs on `an-core`, and a tray
/// call made off the main thread waits for the main thread with no timeout, which at Quit is
/// busy waiting for `an-core` to stop (emit.rs: nothing there may block).
fn refresh(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        // No tray yet (the hub is quicker than upstream's setup): nothing is recorded, so the
        // next event tries again.
        let Some(tray) = handle.tray_by_id("main") else {
            return;
        };
        // Upstream rebuilds its tooltip from its readings; with none to show it asks
        // `tray_tooltip()`, which reads the count. Nudge it now instead of at its next poll.
        let _ = tray.set_tooltip(Some(super::tray_tooltip()));
        let wanted = dot_wanted(
            super::NEEDS_YOU.load(Ordering::Relaxed),
            ENABLED.load(Ordering::Relaxed),
        );
        if wanted == DRAWN.load(Ordering::Relaxed) {
            return;
        }
        let Some(plain) = crate::trayicon::app_mark() else {
            return;
        };
        let icon = if wanted {
            let rgba = with_dot(plain.rgba(), plain.width(), plain.height(), 1);
            tauri::image::Image::new_owned(rgba, plain.width(), plain.height())
        } else {
            plain
        };
        match tray.set_icon(Some(icon)) {
            Ok(()) => DRAWN.store(wanted, Ordering::Relaxed),
            Err(e) => super::log(&format!("tray icon: {e}")),
        }
    });
}

/// Whether the dot is on the icon: something needs the user and the setting allows it.
fn dot_wanted(count: u32, enabled: bool) -> bool {
    enabled && count > 0
}

/// Where the dot lies on a `width` x `height` icon: the centre and the radii of the fill and of
/// the fill plus its ring, in pixels. The ring's outer edge touches the bottom-right corner.
fn dot_shape(width: u32, height: u32) -> (f64, f64, f64, f64) {
    let side = f64::from(width.min(height));
    let fill = (side * DIAMETER / 2.0).max(1.5);
    let outer = fill + 1.0;
    (
        f64::from(width) - outer,
        f64::from(height) - outer,
        fill,
        outer,
    )
}

/// `rgba` (straight, 4 bytes a pixel, row by row) with the needs-you dot in its bottom-right
/// corner; unchanged for a count of 0 or bytes that aren't `width` x `height` pixels. Edges are
/// anti-aliased; nothing farther than a pixel from the disc changes.
fn with_dot(rgba: &[u8], width: u32, height: u32, count: u32) -> Vec<u8> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if count == 0 || expected != Some(rgba.len()) {
        return rgba.to_vec();
    }
    let (cx, cy, fill, outer) = dot_shape(width, height);
    let mut out = rgba.to_vec();
    for y in 0..height {
        for x in 0..width {
            let distance = (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy);
            let ring = coverage(outer, distance);
            if ring == 0.0 {
                continue;
            }
            let at = (y as usize * width as usize + x as usize) * 4;
            let mut pixel = [out[at], out[at + 1], out[at + 2], out[at + 3]];
            pixel = over(pixel, RING, ring);
            pixel = over(pixel, DOT, coverage(fill, distance));
            out[at..at + 4].copy_from_slice(&pixel);
        }
    }
    out
}

/// How much of a pixel whose centre is `distance` from a disc's centre lies inside a disc of
/// `radius` (0..=1, a one-pixel soft edge).
fn coverage(radius: f64, distance: f64) -> f64 {
    (radius - distance + 0.5).clamp(0.0, 1.0)
}

/// `colour` at opacity `alpha` over a straight-alpha `pixel`.
fn over(pixel: [u8; 4], colour: [u8; 3], alpha: f64) -> [u8; 4] {
    if alpha <= 0.0 {
        return pixel;
    }
    let below = f64::from(pixel[3]) / 255.0;
    let out = alpha + below * (1.0 - alpha);
    let mix = |over: u8, under: u8| {
        let value = (f64::from(over) * alpha + f64::from(under) * below * (1.0 - alpha)) / out;
        value.round().clamp(0.0, 255.0) as u8
    };
    [
        mix(colour[0], pixel[0]),
        mix(colour[1], pixel[1]),
        mix(colour[2], pixel[2]),
        (out * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An icon with something in every pixel (so an untouched pixel can be told from a redrawn
    /// one), opaque in the middle and clear at the edges like a real mark.
    fn icon(size: u32) -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let opaque = x > 1 && y > 1 && x < size - 2 && y < size - 2;
                rgba.extend_from_slice(&[
                    (x * 7 % 251) as u8,
                    (y * 13 % 251) as u8,
                    ((x + y) * 5 % 251) as u8,
                    if opaque { 255 } else { 90 },
                ]);
            }
        }
        rgba
    }

    fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    #[test]
    fn the_dot_lies_inside_the_image_and_changes_nothing_outside_it() {
        for size in [32u32, 16] {
            let plain = icon(size);
            let dotted = with_dot(&plain, size, size, 3);
            assert_eq!(dotted.len(), plain.len());
            let (cx, cy, fill, outer) = dot_shape(size, size);
            // The whole disc and its ring lie inside the image.
            assert!(cx - outer >= 0.0 && cx + outer <= f64::from(size), "{size}");
            assert!(cy - outer >= 0.0 && cy + outer <= f64::from(size), "{size}");
            // About 40 % of the icon across.
            let share = 2.0 * fill / f64::from(size);
            assert!((0.3..=0.5).contains(&share), "{size}: {share}");
            let mut changed = 0;
            for y in 0..size {
                for x in 0..size {
                    let distance = (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy);
                    if distance > outer + 0.5 {
                        assert_eq!(
                            pixel(&dotted, size, x, y),
                            pixel(&plain, size, x, y),
                            "{size}: ({x}, {y}) is outside the disc"
                        );
                    } else if pixel(&dotted, size, x, y) != pixel(&plain, size, x, y) {
                        changed += 1;
                    }
                }
            }
            assert!(changed > 0, "{size}: the dot drew nothing");
        }
    }

    #[test]
    fn the_dot_is_solid_in_the_middle_with_a_dark_ring_and_sits_in_the_bottom_right() {
        for size in [32u32, 16] {
            let dotted = with_dot(&icon(size), size, size, 1);
            let (cx, cy, fill, outer) = dot_shape(size, size);
            assert!(cx > f64::from(size) / 2.0 && cy > f64::from(size) / 2.0);
            let middle = pixel(&dotted, size, cx as u32, cy as u32);
            assert_eq!(middle, [DOT[0], DOT[1], DOT[2], 255], "{size}");
            // The ring's middle (between fill and outer edge), on the row of the centre.
            let ring_x = (cx + (fill + outer) / 2.0) as u32;
            let ring = pixel(&dotted, size, ring_x.min(size - 1), cy as u32);
            // Nearly the ring's colour and opaque: its soft edge may not be quite full here.
            for (got, want) in ring.iter().zip([RING[0], RING[1], RING[2], 255]) {
                assert!(got.abs_diff(want) <= 6, "{size}: {ring:?}");
            }
        }
    }

    #[test]
    fn a_count_of_zero_or_bytes_of_another_size_come_back_unchanged() {
        let plain = icon(32);
        assert_eq!(with_dot(&plain, 32, 32, 0), plain);
        assert_eq!(with_dot(&plain, 16, 16, 2), plain);
        assert_eq!(with_dot(&[], 0, 0, 2), Vec::<u8>::new());
    }

    #[test]
    fn a_dot_over_clear_pixels_stays_a_clean_disc() {
        let clear = vec![0u8; 32 * 32 * 4];
        let dotted = with_dot(&clear, 32, 32, 1);
        let (cx, cy, _, _) = dot_shape(32, 32);
        assert_eq!(pixel(&dotted, 32, cx as u32, cy as u32)[3], 255);
        // Nothing appears in the opposite corner.
        assert_eq!(pixel(&dotted, 32, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn the_dot_wants_a_count_and_the_setting() {
        assert!(dot_wanted(1, true));
        assert!(!dot_wanted(0, true));
        assert!(!dot_wanted(4, false));
        assert!(!dot_wanted(0, false));
    }
}
