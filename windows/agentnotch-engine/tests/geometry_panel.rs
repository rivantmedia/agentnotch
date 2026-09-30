//! The sessions panel's placement (`geometry::panel`, a port of ClaudePanelGeometry): the Swift
//! suite ported (each test names its origin), the sealed self-test's fit check as a vector test,
//! and the DPI vectors of design 7.2. Physical pixels, y down.
//!
//! The Windows chrome (tail 32 long x 36 wide, corner 16, margin 8) replaces the Mac's
//! (28.2 / 32.7 / 18.6), so the ported tests' expected numbers move with it; each says how.
//! The Mac's y-up 1512 x 982 screen with a 38 pt menu bar becomes a y-down screen with the bar
//! on top: work area (0, 38, 1512, 944). Tail inset 80 (the Mac's 80.5, whole pixels).

mod geometry {
    use agentnotch_engine::geometry::panel::{
        fallback_ring, height_cap_css, place, tail_offset_limit, tail_tip, width_css, PanelEdge,
        PanelInput, PanelMode, PanelPlacement, PxRect, MINIMUM_HEIGHT_CSS, MINIMUM_WIDTH_CSS,
        TAIL_LENGTH_CSS, UPRIGHT_TIP_INSET_CSS,
    };
    use PanelEdge::{Bottom, Left, Right, Top};
    use PanelMode::{Chat, List};

    const MARGIN: i32 = 8;
    const TAIL: i32 = 32;
    const INSET: i32 = 80;
    const SCREEN: PxRect = PxRect::new(0, 0, 1512, 982);
    const WORK: PxRect = PxRect::new(0, 38, 1512, 944);
    const EPS: f64 = 1e-9;

    // MARK: - Helpers

    fn input(
        edge: Option<PanelEdge>,
        ring: Option<PxRect>,
        work: PxRect,
        scale: f64,
        mode: PanelMode,
        ideal: f64,
    ) -> PanelInput {
        PanelInput {
            edge,
            ring,
            work_area: work,
            scale,
            mode,
            content_height_css: ideal,
        }
    }

    /// Scale 1, the 14" work area, a ring given as a point.
    fn place_at(
        edge: PanelEdge,
        ring: PxRect,
        mode: PanelMode,
        ideal: f64,
        work: PxRect,
    ) -> PanelPlacement {
        place(&input(Some(edge), Some(ring), work, 1.0, mode, ideal))
    }

    fn float(mode: PanelMode, ideal: f64, work: PxRect) -> PanelPlacement {
        place(&input(None, None, work, 1.0, mode, ideal))
    }

    /// The ring at `along` from the notch window's start (top on the side edges, left on top and
    /// bottom), `inset` in from the bezel: the Mac's `ringPoint`, y down.
    fn ring_at(edge: PanelEdge, notch: PxRect, along: i32, inset: i32) -> PxRect {
        match edge {
            Right => PxRect::new(notch.right() - inset, notch.y + along, 0, 0),
            Left => PxRect::new(notch.x + inset, notch.y + along, 0, 0),
            Top => PxRect::new(notch.x + along, notch.y + inset, 0, 0),
            Bottom => PxRect::new(notch.x + along, notch.bottom() - inset, 0, 0),
        }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    fn expect_inside(rect: PxRect, bounds: PxRect, margin: i32) {
        let inner = PxRect::new(
            bounds.x + margin,
            bounds.y + margin,
            bounds.w - 2 * margin,
            bounds.h - 2 * margin,
        );
        assert!(inner.contains(&rect), "{rect:?} is not inside {inner:?}");
    }

    /// The panel window holds the card and the tail, and nothing else.
    fn expect_window_wraps_card(p: &PanelPlacement, edge: PanelEdge) {
        assert!(p.window.contains(&p.card));
        let side = matches!(edge, Right | Left);
        let (win, card) = if side {
            (p.window.w, p.card.w)
        } else {
            (p.window.h, p.card.h)
        };
        assert_eq!(win, card + TAIL);
        let length = if side { p.card.h } else { p.card.w };
        assert!(p.tail_offset.abs() <= tail_offset_limit(f64::from(length)) + EPS);
        assert!(!p.floating);
    }

    fn side_notch(edge: PanelEdge) -> PxRect {
        // 334 wide (body plus the reserved tooltip depth), 782 tall.
        if edge == Right {
            PxRect::new(1512 - 334, 100, 334, 782)
        } else {
            PxRect::new(0, 100, 334, 782)
        }
    }

    fn bar_notch(edge: PanelEdge) -> PxRect {
        // 700 long, 300 deep.
        if edge == Top {
            PxRect::new(406, 0, 700, 300)
        } else {
            PxRect::new(406, 982 - 300, 700, 300)
        }
    }

    // MARK: - Side edges (ClaudePanelGeometryTests)

    /// sideEdgeHangsOffTheRingOnTheInnerSide. The tip lands on the ring (whole pixels here, so
    /// exactly); chrome is Windows'.
    #[test]
    fn side_edge_hangs_off_the_ring_on_the_inner_side() {
        for edge in [Right, Left] {
            for along in [60, 391, 720] {
                let notch = side_notch(edge);
                let ring = ring_at(edge, notch, along, INSET);
                let p = place_at(edge, ring, List, 400.0, WORK);
                assert_eq!(p.width_css, 400.0);
                expect_window_wraps_card(&p, edge);
                expect_inside(p.card, WORK, MARGIN);
                let tip = tail_tip(&p, edge, 1.0).unwrap();
                close(tip.0, f64::from(ring.x));
                close(tip.1, f64::from(ring.y));
                if edge == Right {
                    assert_eq!(p.window.right(), notch.right() - INSET);
                    assert_eq!(p.card.right(), notch.right() - INSET - TAIL);
                } else {
                    assert_eq!(p.window.x, notch.x + INSET);
                    assert_eq!(p.card.x, notch.x + INSET + TAIL);
                }
                // Centred on the ring unless the screen's edge is in the way.
                if ring.y - 200 >= WORK.y + MARGIN && ring.y + 200 <= WORK.bottom() - MARGIN {
                    assert_eq!(p.card.y + p.card.h / 2, ring.y);
                    close(p.tail_offset, 0.0);
                }
            }
        }
    }

    /// sideEdgeRingNearTheTopSlidesTheTailUp (the top margin sits under the work area's top).
    #[test]
    fn side_edge_ring_near_the_top_slides_the_tail_up() {
        let ring = ring_at(Right, side_notch(Right), 60, INSET);
        let p = place_at(Right, ring, List, 400.0, WORK);
        // Card pinned under the top margin; the tail moves up along it.
        assert_eq!(p.card.y, WORK.y + MARGIN);
        assert!(p.tail_offset < 0.0);
    }

    /// sideEdgeRingAtTheVeryCornerClampsTheTailOutOfTheRoundedCorner (limit 400/2 - 16 - 18 = 166).
    #[test]
    fn side_edge_ring_at_the_very_corner_clamps_the_tail_out_of_the_rounded_corner() {
        let notch = PxRect::new(1512 - 334, 0, 334, 982);
        let ring = ring_at(Right, notch, 2, INSET);
        let p = place_at(Right, ring, List, 400.0, WORK);
        let limit = tail_offset_limit(f64::from(p.card.h));
        close(limit, 166.0);
        close(p.tail_offset, -limit);
        close(p.tail_limit, limit);
        expect_inside(p.card, WORK, MARGIN);
    }

    /// sideEdgeChatIsWiderButNeverWiderThanTheRoomBesideTheNotch. Room = 500 - 8 - (80 + 32).
    #[test]
    fn side_edge_chat_is_wider_but_never_wider_than_the_room_beside_the_notch() {
        let ring = ring_at(Left, side_notch(Left), 391, INSET);
        assert_eq!(place_at(Left, ring, Chat, 400.0, WORK).width_css, 440.0);
        let narrow = PxRect::new(0, 0, 500, 800);
        let squeezed = ring_at(Left, PxRect::new(0, 0, 334, 800), 400, INSET);
        let p = place_at(Left, squeezed, Chat, 400.0, narrow);
        close(p.width_css, f64::from(500 - MARGIN - (INSET + TAIL)));
        expect_inside(p.card, narrow, MARGIN);
    }

    /// sideEdgeCardStaysOutOfADockOnTheSameSide: a taskbar on the right leaves the work area
    /// 132 px short of the bezel, under where the tail's root would be.
    #[test]
    fn side_edge_card_stays_out_of_a_dock_on_the_same_side() {
        let docked = PxRect::new(0, 38, 1380, 944);
        let ring = ring_at(Right, side_notch(Right), 391, INSET);
        let p = place_at(Right, ring, List, 400.0, docked);
        expect_inside(p.card, docked, MARGIN);
        expect_window_wraps_card(&p, Right);
    }

    // MARK: - Top and bottom

    /// barHangsOffTheRingOnTheInnerSide.
    #[test]
    fn bar_hangs_off_the_ring_on_the_inner_side() {
        for edge in [Top, Bottom] {
            for along in [40, 350, 660] {
                let notch = bar_notch(edge);
                let ring = ring_at(edge, notch, along, INSET);
                let p = place_at(edge, ring, List, 400.0, WORK);
                assert_eq!(p.width_css, 440.0);
                expect_window_wraps_card(&p, edge);
                expect_inside(p.card, WORK, MARGIN);
                let tip = tail_tip(&p, edge, 1.0).unwrap();
                close(tip.0, f64::from(ring.x));
                close(tip.1, f64::from(ring.y));
                if edge == Top {
                    assert_eq!(p.window.y, notch.y + INSET);
                    assert_eq!(p.card.y, notch.y + INSET + TAIL);
                } else {
                    assert_eq!(p.window.bottom(), notch.bottom() - INSET);
                    assert_eq!(p.card.bottom(), notch.bottom() - INSET - TAIL);
                }
                assert_eq!(p.card.x + p.card.w / 2, ring.x);
            }
        }
    }

    /// splitTopAroundTheCameraHangsJustUnderTheMenuBar. Split inset 38 + 10 (the Mac's 48.5). The
    /// tail is 32 long, not 28.2, so the card starts at 48 + 32 = 80: the Mac's "within 40 of the
    /// work area's top" bound (a 1.8 pt margin there) becomes 44 here.
    #[test]
    fn split_top_around_the_camera_hangs_just_under_the_menu_bar() {
        let notch = PxRect::new(356, 0, 800, 330);
        let split_inset = 38 + 10;
        for along in [250, 560] {
            let ring = ring_at(Top, notch, along, split_inset);
            let p = place_at(Top, ring, List, 400.0, WORK);
            assert_eq!(p.window.y, SCREEN.y + split_inset);
            assert!(p.card.y >= WORK.y + MARGIN);
            assert!(
                p.card.y < WORK.y + 44,
                "the card starts right under the menu bar"
            );
            let tip = tail_tip(&p, Top, 1.0).unwrap();
            close(tip.0, f64::from(ring.x));
            close(tip.1, f64::from(ring.y));
            expect_inside(p.card, WORK, MARGIN);
        }
    }

    /// barRingAtTheScreenCornerClampsTheCardAndTheTail (limit 440/2 - 16 - 18 = 186).
    #[test]
    fn bar_ring_at_the_screen_corner_clamps_the_card_and_the_tail() {
        let notch = PxRect::new(-150, 0, 700, 300);
        let ring = ring_at(Top, notch, 160, INSET);
        let p = place_at(Top, ring, List, 400.0, WORK);
        assert_eq!(p.card.x, WORK.x + MARGIN);
        let limit = tail_offset_limit(f64::from(p.card.w));
        close(limit, 186.0);
        close(p.tail_offset, -limit);

        let right = ring_at(Bottom, PxRect::new(1100, 682, 700, 300), 400, INSET);
        let q = place_at(Bottom, right, List, 400.0, WORK);
        assert_eq!(q.card.right(), WORK.right() - MARGIN);
        close(q.tail_offset, tail_offset_limit(f64::from(q.card.w)));
    }

    /// barHeightIsBoundedByTheRoomBeyondTheNotch.
    #[test]
    fn bar_height_is_bounded_by_the_room_beyond_the_notch() {
        let top = ring_at(Top, bar_notch(Top), 350, INSET);
        let p = place_at(Top, top, Chat, 5000.0, WORK);
        close(f64::from(p.card.h), 780.0);
        close(p.max_height_css, 780.0);
        assert_eq!(p.width_css, 520.0);

        let short = PxRect::new(0, 0, 1280, 600);
        let bottom = ring_at(Bottom, PxRect::new(290, 300, 700, 300), 350, INSET);
        let q = place_at(Bottom, bottom, Chat, 5000.0, short);
        close(
            q.max_height_css,
            f64::from(short.bottom() - MARGIN - (INSET + TAIL)),
        );
        close(f64::from(q.card.h), q.max_height_css);
        expect_inside(q.card, short, MARGIN);
    }

    // MARK: - Heights

    /// heightStaysBetweenTheMinimumAndTheMaximum.
    #[test]
    fn height_stays_between_the_minimum_and_the_maximum() {
        for edge in [Right, Left, Top, Bottom] {
            let notch = if matches!(edge, Right | Left) {
                side_notch(edge)
            } else {
                bar_notch(edge)
            };
            let ring = ring_at(edge, notch, 350, INSET);
            for mode in [List, Chat] {
                let small = place_at(edge, ring, mode, 10.0, WORK);
                assert_eq!(f64::from(small.card.h), MINIMUM_HEIGHT_CSS);
                let tall = place_at(edge, ring, mode, 5000.0, WORK);
                close(f64::from(tall.card.h), tall.max_height_css);
                assert!(tall.max_height_css <= height_cap_css(mode));
                expect_inside(tall.card, WORK, MARGIN);
                let fits = place_at(edge, ring, mode, 333.0, WORK);
                close(f64::from(fits.card.h), 333.0);
            }
        }
    }

    /// aNonFiniteHeightFallsBackToTheMinimum.
    #[test]
    fn a_non_finite_height_falls_back_to_the_minimum() {
        let ring = ring_at(Right, side_notch(Right), 391, INSET);
        for ideal in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let p = place_at(Right, ring, List, ideal, WORK);
            assert_eq!(f64::from(p.card.h), MINIMUM_HEIGHT_CSS);
        }
        // The same for a scale that is not a scale: 1.0.
        let p = place(&input(Some(Right), Some(ring), WORK, f64::NAN, List, 400.0));
        assert_eq!(p.card.w, 400);
    }

    /// smallScreensKeepTheCardOnScreen.
    #[test]
    fn small_screens_keep_the_card_on_screen() {
        let small = PxRect::new(0, 0, 800, 560);
        for edge in [Right, Left, Top, Bottom] {
            let notch = match edge {
                Right => PxRect::new(800 - 334, 0, 334, 560),
                Left => PxRect::new(0, 0, 334, 560),
                Top => PxRect::new(50, 0, 700, 300),
                Bottom => PxRect::new(50, 560 - 300, 700, 300),
            };
            let ring = ring_at(edge, notch, 280, INSET);
            for mode in [List, Chat] {
                let p = place_at(edge, ring, mode, 5000.0, small);
                expect_inside(p.card, small, MARGIN);
                expect_window_wraps_card(&p, edge);
            }
        }
    }

    /// aScreenShorterThanTheMinimumKeepsTheCardsTopOnScreen.
    #[test]
    fn a_screen_shorter_than_the_minimum_keeps_the_cards_top_on_screen() {
        let tiny = PxRect::new(0, 0, 800, 200);
        let ring = ring_at(Right, PxRect::new(466, 0, 334, 200), 100, INSET);
        let p = place_at(Right, ring, List, 400.0, tiny);
        assert_eq!(f64::from(p.card.h), MINIMUM_HEIGHT_CSS);
        assert_eq!(p.card.y, tiny.y + MARGIN);
    }

    // MARK: - Widths

    /// listAndChatWidths.
    #[test]
    fn list_and_chat_widths() {
        assert_eq!(width_css(Some(Right), List), 400.0);
        assert_eq!(width_css(Some(Left), Chat), 440.0);
        assert_eq!(width_css(Some(Top), List), 440.0);
        assert_eq!(width_css(Some(Bottom), Chat), 520.0);
        assert_eq!(width_css(None, List), 440.0);
        assert_eq!(width_css(None, Chat), 520.0);
    }

    // MARK: - Floating

    /// noAnchorFloatsWithoutATailUnderTheMenuBar. Also: an edge without a ring rect, or a ring
    /// rect without an edge, floats.
    #[test]
    fn no_anchor_floats_without_a_tail_under_the_menu_bar() {
        for (mode, width) in [(List, 440), (Chat, 520)] {
            let p = float(mode, 300.0, WORK);
            assert!(p.floating);
            assert_eq!(p.window, p.card);
            assert_eq!(p.tail_offset, 0.0);
            assert_eq!(p.tail_length, 0);
            assert_eq!(p.width_css, f64::from(width));
            assert_eq!(p.card.x + p.card.w / 2, WORK.x + WORK.w / 2);
            assert_eq!(p.card.y, WORK.y + MARGIN);
            close(p.max_height_css, f64::from(WORK.h - 2 * MARGIN));
            assert!(tail_tip(&p, Right, 1.0).is_none());
        }
        let ring = Some(PxRect::new(1400, 400, 60, 60));
        assert!(place(&input(Some(Right), None, WORK, 1.0, List, 300.0)).floating);
        assert!(place(&input(None, ring, WORK, 1.0, List, 300.0)).floating);
    }

    /// floatingUsesThePointersScreen: no 680 / 780 cap when floating, as on the Mac (the work
    /// area alone: 1055 - 16).
    #[test]
    fn floating_uses_the_pointers_screen() {
        let second = PxRect::new(1512, -200, 1920, 1055);
        let p = float(List, 5000.0, second);
        expect_inside(p.card, second, MARGIN);
        assert_eq!(p.card.x + p.card.w / 2, second.x + second.w / 2);
        assert_eq!(p.card.h, second.h - 2 * MARGIN);
    }

    // MARK: - PanelFitsTheScreenTests

    /// aBusyPanelFitsTheScreenAtEverySizeAndEdge, as a pure vector: the sealed self-test's
    /// hosted runner is 1024 x 768. Every edge and floating, list and chat, a busy list, rings at
    /// the start, middle and end, at 100, 125 and 150 %: the window lies inside the work area.
    #[test]
    fn a_busy_panel_fits_the_screen_at_every_size_and_edge() {
        let work = PxRect::new(0, 0, 1024, 768);
        for scale in [1.0, 1.25, 1.5] {
            let tip = (UPRIGHT_TIP_INSET_CSS * scale).round() as i32;
            for mode in [List, Chat] {
                let f = place(&input(None, None, work, scale, mode, 5000.0));
                assert!(work.contains(&f.window), "floating {mode:?} {scale}: {f:?}");
                for edge in [Right, Left, Top, Bottom] {
                    let notch = match edge {
                        Right => PxRect::new(1024 - 334, 0, 334, 768),
                        Left => PxRect::new(0, 0, 334, 768),
                        Top => PxRect::new(162, 0, 700, 300),
                        Bottom => PxRect::new(162, 468, 700, 300),
                    };
                    let length = if matches!(edge, Right | Left) {
                        768
                    } else {
                        700
                    };
                    for along in [0, length / 2, length] {
                        let ring = ring_at(edge, notch, along, tip);
                        let p = place(&input(Some(edge), Some(ring), work, scale, mode, 5000.0));
                        assert!(
                            work.contains(&p.window),
                            "{edge:?} {mode:?} x{scale} along {along}: {p:?}"
                        );
                        assert!(work.contains(&p.card));
                    }
                }
            }
        }
    }

    // MARK: - DPI vectors (design 7.2)

    /// Scale 1.0, 1.25, 1.5 on the same physical work area and a ring 60..80 px across on a
    /// right-hand notch. Constants scale then round: margin 8/10/12, tail 32/40/48, card 400
    /// CSS px wide = 400/500/600, cap 680 = 680/850/1020; the CSS outputs do not move.
    #[test]
    fn scale_vectors_right_edge() {
        let work = PxRect::new(0, 0, 1920, 1080);
        // (scale, ring rect, card, window)
        let cases = [
            (
                1.0,
                PxRect::new(1800, 500, 40, 40),
                PxRect::new(1368, 320, 400, 400),
                PxRect::new(1368, 320, 432, 400),
            ),
            (
                1.25,
                PxRect::new(1800, 500, 60, 60),
                PxRect::new(1260, 280, 500, 500),
                PxRect::new(1260, 280, 540, 500),
            ),
            (
                1.5,
                PxRect::new(1780, 490, 80, 80),
                PxRect::new(1132, 230, 600, 600),
                PxRect::new(1132, 230, 648, 600),
            ),
        ];
        for (scale, ring, card, window) in cases {
            let p = place(&input(Some(Right), Some(ring), work, scale, List, 400.0));
            assert_eq!(p.card, card, "card at x{scale}");
            assert_eq!(p.window, window, "window at x{scale}");
            assert_eq!(p.tail_length, (32.0 * scale).round() as i32);
            close(p.width_css, 400.0);
            close(p.max_height_css, 680.0);
            close(p.tail_offset, 0.0);
            close(p.tail_limit, 166.0);
            let tip = tail_tip(&p, Right, scale).unwrap();
            close(tip.0, f64::from(ring.x));
            close(tip.1, f64::from(ring.y + ring.h / 2));
        }
    }

    /// The same at 150 % for a bar: card 440 CSS px = 660 px, centred on the ring, tail offset
    /// and its limit in CSS px (a 40 px ring offset = 26.67 CSS px).
    #[test]
    fn scale_vector_bar_reports_css_offsets() {
        let work = PxRect::new(0, 0, 1920, 1080);
        // A ring near the left end: the card is pulled against the left margin, so the tail
        // slides left of the card's centre.
        let ring = PxRect::new(230, 60, 60, 60); // centre x 260, bottom 120
        let p = place(&input(Some(Top), Some(ring), work, 1.5, List, 300.0));
        // width 660, x = max(260 - 330, 12) = 12, tip 120, card.y = 120 + 48.
        assert_eq!(p.card, PxRect::new(12, 168, 660, 450));
        assert_eq!(p.window, PxRect::new(12, 120, 660, 498));
        // Offset 260 - (12 + 330) = -82 px = -54.67 CSS px; the limit is 440/2 - 34 = 186.
        close(p.tail_offset, -82.0 / 1.5);
        close(p.tail_limit, 186.0);
        close(p.width_css, 440.0);
    }

    /// A ring on a 150 % monitor whose panel lands on a 100 % one: two monitors side by side, A
    /// (0, 0, 2880 x 1620 at 150 %) then B (2880, 0, 1920 x 1080 at 100 %). The ring sits
    /// 30 px inside A's right end, the glue decided the panel goes to B. Sizes follow B (a 440 px
    /// card, not 660), the card is pulled onto B's work area, and the tail slides (clamped) to
    /// keep pointing back at the ring across the seam.
    #[test]
    fn a_ring_on_a_150_percent_monitor_lands_on_a_100_percent_one() {
        let b_work = PxRect::new(2880, 0, 1920, 1080);
        let ring = PxRect::new(2790, 60, 60, 60); // centre x 2820 on A; bottom edge y 120
        let p = place(&input(Some(Top), Some(ring), b_work, 1.0, List, 400.0));
        assert_eq!(p.card, PxRect::new(2888, 152, 440, 400));
        assert_eq!(p.window, PxRect::new(2888, 120, 440, 432));
        assert!(b_work.contains(&p.window));
        close(p.width_css, 440.0);
        close(p.max_height_css, 680.0);
        close(p.tail_limit, 186.0);
        close(p.tail_offset, -186.0);

        // The same ring placed on A's own work area at 150 %: 660 px wide, centred on the ring.
        let a_work = PxRect::new(0, 0, 2880, 1620);
        let q = place(&input(Some(Top), Some(ring), a_work, 1.5, List, 400.0));
        // 660 px wide (440 CSS px); the ring is near A's right end, so the card is pulled back
        // to A's right margin (2880 - 12) and the tail, 282 px = 188 CSS px off centre, clamps.
        assert_eq!(q.card.w, 660);
        assert_eq!(q.card.right(), 2880 - 12);
        close(q.tail_offset, 186.0);
        close(q.width_css, 440.0);
    }

    /// A taskbar on top (work area starts at y 48): a bar notch's card is pulled below it, and a
    /// floating card sits under it.
    #[test]
    fn work_area_with_a_top_taskbar() {
        let work = PxRect::new(0, 48, 1920, 1032);
        let ring = ring_at(Top, PxRect::new(560, 0, 800, 300), 400, 10);
        // Tip at y 10, root would be 42: pulled to 48 + 8.
        let p = place_at(Top, ring, List, 400.0, work);
        assert_eq!(p.card.y, 56);
        assert_eq!(p.window.y, 24);
        // Room below the pulled card (1016) is more than the 680 cap.
        close(p.max_height_css, 680.0);
        expect_inside(p.card, work, MARGIN);
        let f = float(List, 300.0, work);
        assert_eq!(f.card.y, 56);
        // A bottom-edge notch is not moved by it.
        let bottom = ring_at(Bottom, PxRect::new(560, 780, 800, 300), 400, INSET);
        let q = place_at(Bottom, bottom, List, 400.0, work);
        assert_eq!(q.card.bottom(), 1080 - INSET - TAIL);
    }

    /// A taskbar on the left (work area starts at x 60): a left notch's tail root is pulled to
    /// the work area, its card is narrower by the pull, and a right notch is unaffected.
    #[test]
    fn work_area_with_a_left_taskbar() {
        let work = PxRect::new(60, 0, 1860, 1080);
        // Ring 30 px from the screen's left edge, under the taskbar.
        let ring = PxRect::new(30, 500, 0, 0);
        let p = place_at(Left, ring, List, 400.0, work);
        assert_eq!(p.card.x, 68); // max(30 + 32, 60 + 8)
        assert_eq!(p.window.x, 68 - TAIL);
        assert_eq!(p.card.w, 400);
        expect_inside(p.card, work, MARGIN);
        // The same at 150 %: root max(30 + 48, 60 + 12) = 78.
        let q = place(&input(Some(Left), Some(ring), work, 1.5, List, 400.0));
        assert_eq!(q.card.x, 78);
        assert_eq!(q.card.w, 600);
        let right = ring_at(Right, PxRect::new(1586, 200, 334, 700), 350, INSET);
        let r = place_at(Right, right, List, 400.0, work);
        assert_eq!(r.card.right(), 1920 - INSET - TAIL);
    }

    /// The tail offset clamps at both card corners on a side edge (the Mac tests cover the top
    /// one; the bottom one is new), in CSS px, at 150 % too.
    #[test]
    fn tail_offset_clamps_at_both_card_corners() {
        let work = PxRect::new(0, 0, 1920, 1080);
        for scale in [1.0, 1.5] {
            let top_ring = PxRect::new(1800, 0, 0, 0);
            let bottom_ring = PxRect::new(1800, 1080, 0, 0);
            let up = place(&input(
                Some(Right),
                Some(top_ring),
                work,
                scale,
                List,
                400.0,
            ));
            let down = place(&input(
                Some(Right),
                Some(bottom_ring),
                work,
                scale,
                List,
                400.0,
            ));
            close(up.tail_limit, 166.0);
            close(up.tail_offset, -166.0);
            close(down.tail_offset, 166.0);
            // A ring level with the middle of the work area needs no offset.
            let mid = PxRect::new(1800, 540, 0, 0);
            let m = place(&input(Some(Right), Some(mid), work, scale, List, 400.0));
            close(m.tail_offset, 0.0);
        }
        // Bars, both ends.
        let left_end = place_at(Top, PxRect::new(0, 10, 0, 0), List, 400.0, work);
        let right_end = place_at(Top, PxRect::new(1920, 10, 0, 0), List, 400.0, work);
        close(left_end.tail_offset, -186.0);
        close(right_end.tail_offset, 186.0);
    }

    /// A work area too small for the width: beside the notch the card falls to the minimum width
    /// 200 (and may then overflow the room), on a flat edge to the work area minus the margins,
    /// then to 200; floating has no minimum, as on the Mac.
    #[test]
    fn a_work_area_too_small_for_the_width_falls_to_the_minimum() {
        // Side: the room is the root (ring - tail) less the far margin.
        let work = PxRect::new(0, 0, 1000, 800);
        let near = ring_at(Right, PxRect::new(266, 0, 334, 800), 400, 80); // tip x = 520
        let p = place_at(Right, near, List, 400.0, work);
        // root 488, room 480: the width 400 fits.
        assert_eq!(p.card.w, 400);
        let tighter = ring_at(Right, PxRect::new(0, 0, 334, 800), 400, 80); // tip x = 254
        let q = place_at(Right, tighter, Chat, 400.0, work);
        // root 222, room 214 < 440: squeezed to 214, above the minimum.
        assert_eq!(q.card.w, 214);
        let tightest = ring_at(Right, PxRect::new(0, 0, 300, 800), 400, 80); // tip x = 220
        let r = place_at(Right, tightest, Chat, 400.0, work);
        // root 188, room 180 < 200: the minimum wins.
        assert_eq!(r.card.w, 200);
        assert_eq!(r.card.x, 188 - 200);
        close(r.width_css, MINIMUM_WIDTH_CSS);

        // Flat edge: 300 wide work area -> 284; 180 wide -> 200.
        for (w, expect) in [(300, 284), (180, 200)] {
            let work = PxRect::new(0, 0, w, 900);
            let ring = PxRect::new(w / 2, 20, 0, 0);
            let p = place_at(Top, ring, List, 400.0, work);
            assert_eq!(p.card.w, expect, "work width {w}");
            assert_eq!(p.card.x, 8, "low wins when the range is empty or tight");
        }
        // Floating: no minimum.
        let f = float(List, 300.0, PxRect::new(0, 0, 150, 900));
        assert_eq!(f.card.w, 134);
        let g = float(List, 300.0, PxRect::new(0, 0, 10, 900));
        assert_eq!(g.card.w, 0);
    }

    /// A work area too small for the cap: the card is capped at the work area minus the margins,
    /// then at the minimum height when even that is too small; scaled too.
    #[test]
    fn a_work_area_too_small_for_the_cap_falls_to_the_work_area_then_the_minimum() {
        let work = PxRect::new(0, 0, 1280, 500);
        let ring = ring_at(Right, PxRect::new(946, 0, 334, 500), 250, INSET);
        let p = place_at(Right, ring, Chat, 5000.0, work);
        close(p.max_height_css, 484.0);
        assert_eq!(p.card.h, 484);
        assert_eq!(p.card.y, 8);
        // 150 %: the work area minus 2 x 12 = 476 physical = 317.33 CSS px.
        let q = place(&input(Some(Right), Some(ring), work, 1.5, Chat, 5000.0));
        assert_eq!(q.card.h, 476);
        close(q.max_height_css, 476.0 / 1.5);
        // Shorter than the minimum: 220 CSS px wins, top kept on the work area.
        let tiny = PxRect::new(0, 0, 1280, 200);
        let tiny_ring = ring_at(Right, PxRect::new(946, 0, 334, 200), 100, INSET);
        let r = place_at(Right, tiny_ring, List, 5000.0, tiny);
        assert_eq!(r.card.h, 220);
        assert_eq!(r.card.y, 8);
        close(r.max_height_css, 220.0);
        // The floating card has no 680 / 780 cap either, only the work area.
        let f = float(Chat, 5000.0, PxRect::new(0, 0, 1280, 1000));
        assert_eq!(f.card.h, 984);
    }

    // MARK: - The fallback anchor

    /// The middle of the notch window's side that touches the screen edge, moved in by the tip
    /// inset (scaled); with no inset it is that side's middle exactly.
    #[test]
    fn fallback_ring_is_the_middle_of_the_side_on_the_screen_edge() {
        let window = PxRect::new(1500, 100, 420, 800);
        assert_eq!(
            fallback_ring(Right, window, 1.0, 0.0),
            PxRect::new(1920, 500, 0, 0)
        );
        assert_eq!(
            fallback_ring(Left, window, 1.0, 0.0),
            PxRect::new(1500, 500, 0, 0)
        );
        assert_eq!(
            fallback_ring(Top, window, 1.0, 0.0),
            PxRect::new(1710, 100, 0, 0)
        );
        assert_eq!(
            fallback_ring(Bottom, window, 1.0, 0.0),
            PxRect::new(1710, 900, 0, 0)
        );
        // 69 CSS px at 125 % = 86 px inward.
        assert_eq!(
            fallback_ring(Right, window, 1.25, UPRIGHT_TIP_INSET_CSS).x,
            1920 - 86
        );
        assert_eq!(
            fallback_ring(Left, window, 1.25, UPRIGHT_TIP_INSET_CSS).x,
            1500 + 86
        );
        assert_eq!(fallback_ring(Top, window, 1.0, 69.0).y, 169);
        assert_eq!(fallback_ring(Bottom, window, 1.0, 69.0).y, 831);
        // And it places like any ring.
        let ring = fallback_ring(Right, window, 1.0, UPRIGHT_TIP_INSET_CSS);
        let work = PxRect::new(0, 0, 1920, 1080);
        let p = place_at(Right, ring, List, 400.0, work);
        assert_eq!(p.window.right(), 1920 - 69);
        assert_eq!(p.card.y + p.card.h / 2, 500);
    }

    #[test]
    fn placement_and_input_serialise_for_the_self_test_report() {
        let p = place_at(Right, PxRect::new(1400, 400, 0, 0), List, 400.0, WORK);
        let v = serde_json::to_value(p).unwrap();
        assert_eq!(v["window"]["w"], p.window.w);
        assert_eq!(v["floating"], false);
        let i = input(Some(Bottom), None, WORK, 1.0, Chat, 1.0);
        let s = serde_json::to_string(&i).unwrap();
        assert!(s.contains("\"edge\":\"bottom\"") && s.contains("\"mode\":\"chat\""));
        let back: PanelInput = serde_json::from_str(&s).unwrap();
        assert_eq!(back, i);
        assert_eq!(TAIL_LENGTH_CSS, 32.0);
    }
}
