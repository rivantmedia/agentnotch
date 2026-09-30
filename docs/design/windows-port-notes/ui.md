SPEC: AGENT NOTCH CLAUDE UI FOR THE WINDOWS (TAURI 2 / WEBVIEW2) PORT
Area: panel, chat, settings pane, notch decorations and theme, plus how they map onto windows/codenotch/ui

The repo was only read. I rendered the snapshots with Scripts/spm-snapshots.sh to the scratchpad and looked at every sheet:
  <scratch>/winmap/snapshots/
There are 28 sheets, each at a content width of 400 and 520 pt, rendered at 2x. Each PNG is (width+32)*2 px wide, so a 16 pt grey desktop margin surrounds the card. The tall settings sheets are cut into readable chunks in .../winmap/crops2/. The .../winmap/crops/ folder holds wrong crops from sips and can be ignored.
The notch-side sheets (badges, folded marks, panel chrome per edge) only come from a sealed launch of the app (spm-run-sealed.sh --snapshot-claude), so I did not render them. They are described below from code: Sources/ClaudeBridge/ClaudeAppSnapshots+Notch.swift and +Panel.swift.

==========================================================================================
0. FILE MAP (what was read)
==========================================================================================
Package UI (Packages/ClaudeControl/Sources/ClaudeControl/UI/):
  Public/ClaudeControlTheme.swift     theme tokens
  Support/ClaudeInk.swift             token and font lookup
  Support/ClaudeMotion.swift          animation constants
  Public/ClaudeSessionsPanel.swift    live wiring and actions
  Public/ClaudePanelState.swift       routes, drafts, undo, height
  Panel/SessionsPanelContent.swift    the page
  Panel/PanelHeader.swift
  Panel/SetupBanners.swift            banners and ConsentCopy
  Panel/SessionList.swift
  Panel/SessionRow.swift
  Panel/RowActionBar.swift
  Panel/SessionRowModel.swift
  Panel/SessionsPanelModel.swift      HookHealth and actions
  Panel/AnswerGate.swift
  Panel/ClaudeKeyRouter.swift
  Panel/PanelKeys.swift
  Components/*                        buttons, status ring, meters, account tag, text field, sealed badge
  Components/Chat/*                   chat header, task board, approval bars, question panel and form
  Views/ChatView.swift
  Views/ToolResultViews.swift
  Components/MarkdownRenderer.swift
  Settings/SettingsPaneContent.swift
  Settings/AccountsSection.swift
  Settings/CloudSection.swift
  Settings/SettingsPaneModel.swift

Engine logic that the UI copy depends on:
  Engine/Attention/SessionRowContent.swift     what each row says and offers
  Engine/Attention/SessionSections.swift       grouping, ordering, folding
  Engine/Attention/ClaudeAttentionPolicy.swift reactions, resting marks
  Engine/Panel/ClaudePanelPolicy.swift         panel window decisions
  Engine/Geometry/ClaudePanelGeometry.swift
  Engine/Geometry/ClaudeRingBadgeLayout.swift
  Engine/Core/ClaudeControlSettings.swift      settings keys and enums
  Engine/Models/SessionAttention.swift
  Engine/Public/ClaudeHostProjections.swift:325-381   hover-card rows

Bridge (Sources/ClaudeBridge/):
  ClaudeRingDecoration.swift       badges and the settle switch
  ClaudeRestingMarks.swift         marks on the folded notch
  ClaudeAttentionReactions.swift
  ClaudePanelController.swift
  ClaudePanel.swift
  ClaudePanelChromeView.swift
  NotchFleet+ClaudeAnchor.swift
  ClaudeNotchHold.swift
  ClaudeNotchEnvironment.swift
  ClaudeSessionFeed.swift
  ClaudeBridge.swift:217-280       hover-row and ring clicks
  ClaudeCodeSettingsHost.swift
  ClaudeTheme+Codenotch.swift

Upstream reference:
  Sources/DesignSystem/Palette.swift, Design.swift
  Sources/Notch/NotchLayout.swift
  Sources/Features/ProviderRing.swift:216-270   activity arc
  Sources/Features/TooltipCard.swift:980-1080   hover-card session rows

Windows port:
  windows/codenotch/ui/notch.html (1605 lines)
  windows/codenotch/ui/settings.html (1626 lines)
  windows/codenotch/src/main.rs, state.rs, server.rs, focus.rs, hooks_install.rs, usage.rs, settings_window.rs
  windows/codenotch/tauri.conf.json

==========================================================================================
1. DESIGN TOKENS
==========================================================================================
Source: ClaudeControlTheme.swift:154-190 (codenotchDark) and the derived tokens at :128-138. The live app builds the theme in ClaudeTheme+Codenotch.swift:35-61 from Palette.swift:21-84.
Every view reads colour only from these tokens (ClaudeInk.swift:43-66). The web port should expose them as CSS variables on :root and on :root[data-theme=light], and apply data-theme the same way notch.html:300-307 does (Rust command get_theme_resolved, event theme_resolved).

1.1 Base colours

  | token         | dark                                | light (glass, light appearance) |
  |---------------|-------------------------------------|---------------------------------|
  | textPrimary   | #FFFFFF                             | #000000 (Windows notch uses #1d1d1f; either works) |
  | textSecondary | #808080                             | #6B6B6B |
  | needsYou      | #F2FF00 (Palette.watch)             | #B08800 |
  | review        | #00FF88 (ample)                     | #00A356 |
  | working       | = textPrimary                       | = textPrimary |
  | critical      | #FF3F00                             | #FF3F00 |
  | track         | rgba(255,255,255,.188), #303030 on black | rgba(0,0,0,.16) |
  | barTrack      | rgba(255,255,255,.176), #2D2D2D     | rgba(0,0,0,.15) |
  | accent        | #D97757 in snapshots; live app uses the notch accent from Preferences | same |
  | card          | #000000 (solid surface); clear on glass | panel surface colour |

1.2 Derived tokens (computed from textPrimary; hex values are the result over black)
  textTertiary        textPrimary at .30   #4D4D4D   separators and disabled glyphs, never body text
  controlFill         .09                  #171717   resting fill of secondary buttons, chips, fields, banners
  controlFillHover    .16                  #292929
  rowHover            .05                  #0D0D0D
  rowSelection        .08                  #141414
  rowSelectionStroke  .24                  #3D3D3D   outline of the selected row, focused text field
  separator           = track
  primaryFill         .95                  #F2F2F2
  onPrimary           #000 dark, #FFF light
  In light mode every derived token is the same alpha over black instead of white.

1.3 Account hues (hue(forAccount: colorIndex % 8))
  0 #5C9EFA blue
  1 #B885F5 violet
  2 #F573B3 pink
  3 #40CCCC teal
  4 #858FFF indigo
  5 #66D1F5 cyan
  6 #FF949E rose
  7 #D6C29E sand
  They are kept away from the signal colours on purpose; never show an account by colour alone.

1.4 Type
Mac points are CSS px 1:1. Source: codenotchDark :179-184 and CapHeight in ClaudeTheme+Codenotch.swift:18-24.

  | token        | size and weight                 |
  |--------------|---------------------------------|
  | title        | 13.7 semibold (600)             |
  | chat         | 12.6 regular                    |
  | rowTitle     | 11.6 medium (500)               |
  | body         | 10.5 regular                    |
  | caption      | 9.5 regular                     |
  | mono         | 10.5 monospaced                 |
  | monoCaption  | 9.5 monospaced                  |
  | sectionTitle | = caption semibold              |
  | button       | = body semibold                 |
  | compact button | caption semibold              |

  Numbers use monospacedDigit (font-variant-numeric: tabular-nums).
  Fonts on Windows: "Segoe UI Variable Text","Segoe UI" for text and "Cascadia Mono",Consolas for mono.
  Segoe renders smaller than SF, and the Windows notch card already runs larger than the Mac card (.w-row 12px, .c-title 14px, .c-sub/.w-used 11px). So scale the Mac set by one factor, --ui-scale of about 1.15:
    title 15-16, chat 14, rowTitle 13, body 12, caption 11, mono 12.

1.5 Metrics (ClaudeControlTheme :82-100)
  corner 18.6            card corner; the Windows card uses 16
  padding 12
  blockSpacing 7.5
  lineGap 3.8
  barHeight 3.9
  hairline 0.94          use 1px
  statusRing 8.5, statusRingStroke 1.6
  rowCorner 10           rows, chips, banners, toast
  controlHeight 20       buttons; chips are 18

1.6 Motion (ClaudeMotion.swift:15-44)
  Needs-you breath: 7 half-cycles of 0.9 s ease-in-out, opacity .35 to 1, ending bright. It never loops. It restarts when the needs-you count changes (breathKey).
    CSS: animation: breathe .9s ease-in-out 7 alternate both
  Working arc: 3/4 of a circle, one turn per 1.4 s. Windows' notch deliberately uses steps() to spare the DWM compositor (notch.html:94-99); do the same: spin 1.4s steps(12) infinite.
  glide = spring(response .32, damping .86). Rows moving to new positions, sections folding. On the web, use a FLIP transition of about 320 ms, cubic-bezier(.2,.8,.2,1).
  quick = easeOut .12 s. Hover fills and small changes.
  Reduce Motion (prefers-reduced-motion, which is Windows' "Animation effects" switch): no breath, a still 3/4 arc, no transitions.
  Snapshot/static mode: timers stop and fields show their text.

==========================================================================================
2. THE WINDOWS NOTCH TODAY (what the fork UI builds on)
==========================================================================================
2.1 Window model
  - One Tauri window with label "notch" (tauri.conf.json; main.rs:37,46,286-292,294-360).
  - It is transparent, has no decorations, is always on top, skips the taskbar, has no shadow and is focusable:false.
  - Logical size: 360x650 on the upright (left/right) edges; 650x650 on the flat (top/bottom) edges.
  - It is placed flush with the chosen monitor edge. Config keys: notch_edge and the along ratio.
  - It is zoomed by the notch Size setting (0.8, 1, 1.25) through w.set_zoom. The page adds its own CSS zoom fallback (fitZoom, design widths 360/650, notch.html:1317-1334).
  - Click-through: the page reports its "hot" rectangles in physical px with set_hot(rects, expanded) (notch.html:1288-1298, main.rs:802-818).
  - Rust watches the cursor and toggles set_ignore_cursor_events. So anything interactive must be in a reported rectangle.
  - A folded notch reports only its wake rectangle (the rest pill plus a 34 px band).

2.2 Pill and rings (notch.html:40-120, 1057-1112)
  - #pill is 70 px deep (a column on left/right; a row with 12x18 padding on top/bottom). Rounded 20 px on the inner side, 1 px stroke in --edge, concave fillets of 38.7 px.
  - Each provider is a .cell: a .ringwrap (--ring 44px) plus a .pct label (15px, weight 600) under it. Gap inside the cell is 6 px; gap between cells 14 px; pill end padding 18 px.
  - The ring is three stacked SVGs in a 56-unit viewBox:
      hole: r22 filled --hole
      track: r25, stroke 5, --track
      reading: r25, stroke 5, coloured by tone()
          hard step: ample under 50%, watch at 50-70%, critical at 70% and above
          or a continuous ramp
      weekly ring (config weekly_ring off, inside or outside): r16 inside or r31 outside, stroke 2.4, track opacity .7, arc opacity .85
      activity: r19, stroke 2.5
          running: a 28% arc in INK, class arc-spin, 1.2 s steps(12)
          attention: a full ring in WATCH, class arc-pulse, 1.1 s steps(6)
  - A stale reading (more than 15 minutes old) dims the ring, reading and glyph to .55. The activity arc keeps its strength.
  - The glyph is the provider's mark, 26*k px.

2.3 Hover card (notch.html:100-116, 1167-1261)
  - #card: max-width 246, radius 16, padding 16, background --card (#0a0a0a dark, #fff light).
  - The tail #tail is an element of its own, 32x36, clip-path:
      right edge: M0 0C0 9 18.56 13.68 32 18C18.56 22.32 0 27 0 36Z
    Its tip sits at right:69px, 1 px under the pill. The shape is mirrored or rotated for the other edges.
  - The card is centred on the hovered cell and kept inside the window, the taskbar insets and an 8 px margin.
  - On flat edges the card sits 30 px clear of the pill.
  - Card content: head (mark plus title), window rows (label, reset, 4 px bar, "N% used"), notes.
  - Then for Claude only, the session list:
      stateSnap.sessions where state != idle
      at most 5 rows (SESSION_ROWS), then "and N more"
      each row is a 6 px dot plus the title
      dot colours: running INK, attention WATCH, done AMPLE, idle TRACK
      rows are not clickable ("a full list with jump-to-session is a follow-up")

2.4 Other page behaviour
  - Show on hover (folding): the pill is clipped to a 10x79 "rest" pill (#rest) and cells fade out in turn.
  - Handles: the settings orb and the move handle (notch.html:1463-1524).
  - Alt+drag slides the notch along its edge.
  - A plain click on a ring calls refreshRing, which invokes refresh_ring.
  - Right-click shows a native menu (show_notch_menu, src/notchmenu.rs).
  - Theme, edge, language, slots, weekly ring and colour transition all arrive as Tauri events.

2.5 Claude state and hooks on Windows
  - State: state.rs Session has id, title, state, started, total, last, attn, prompt, model, ppid, cwd. Snapshot has sessions and agg (running, attention or idle).
  - Hooks: codenotch-hook.exe POSTs to 127.0.0.1:<port>/event (server.rs:9-60).
  - hooks_install.rs merges "codenotch-hook" entries into ~/.claude/settings.json.
  - Multi-account Claude: claudeCells() (notch.html:975-992) splits usage.windows by the group field and "@slug" ids.

2.6 Token rule conflict (fork rule)
  - windows/codenotch/src/usage.rs:2-16,68,196-221 reads ~/.claude/.credentials.json and calls api.anthropic.com/api/oauth/usage.
  - The Claude card's "Sign in" button (claude_sign_in, notch.html:1149-1164) and the plan-from-credentials naming are part of this path.
  - The fork forbids it (CLAUDE.md "No token paths"). The UI must drop the button and the credentials-derived naming, and use engine readings instead.

2.7 Settings window (settings.html:1-140; src/settings_window.rs)
  - 680x520 inner size, frameless, over Mica (solid fallback with body.solid).
  - Sidebar #side 196 px with .row tabs (Accounts, Appearance, General; Quit at the bottom).
  - Pane: #head h1 22px/600, then #body scrolls.
  - Building blocks:
      .sec     section title, 13.5px/600
      .group   rounded 10, --group fill, 1 px --line border
      .item    40 px min height, justify space-between
      .item.cap  12 px --text2 caption row
      .switch  40x20 toggle, accent when on
      .seg     segmented control
      select
      .btn     6 px radius
      .link
      .acct    account rows
      #strip   error strip
      #toast
  - Tokens: --accent #0a7aff, --text #f2f2f4, --text2 rgba(235,235,245,.62), --text3 .38, plus light overrides.
  - Font: 13.5px "Segoe UI Variable Text".

==========================================================================================
3. NOTCH SURFACE ADDITIONS (to reproduce in notch.html)
==========================================================================================
3.1 Rings
  - One ring per Claude account. Ring id is "claude-acct-<12 hex of sha256(accountUuid)>".
  - Name: the Settings nickname, else the engine label (ClaudeRingNames.swift:9-18).
  - This replaces claudeCells()' group/@slug scheme. The engine sends a ring list with a reading per ring.
  - Weekly ring default is OUTSIDE (Fork.swift default registration). The Windows default is 'off', so set 'outside' for fresh installs.
  - Reading headline: window "session". Weekly: "weekly_all".

3.2 Activity arc states (Mac ProviderRing.swift:233-240 and ClaudeRingDecoration.swift:67-69)
  working   the spinning 1/4 arc in white. The Windows arc-spin is the same idea.
  waiting   a full ring in amber, pulsing .9 s. Windows arc-pulse.
  success   (NEW on Windows) a full ring in green, pulsing for 90 s after the ring's newest completion, then steady at opacity .85.
            The engine publishes freshSuccessUntil[ring]; ClaudeNotchState.settles() (ClaudeNotchEnvironment.swift:~170) is "now >= deadline".
  idle      nothing.
  A ring's state is its most urgent session: waiting over working over success (Codenotch ActivitySummary).

3.3 Badges (ClaudeRingDecoration.swift:104-152; ClaudeRingBadgeLayout.swift:33-113)
  Shown when the setting "Counts on the Claude rings" (claudeControl.ringBadges, default on) is on.

  Two badges:
    needsYou  count of sessions that need you, in Palette.watch
    review    count of finished, unreviewed sessions, in ample
  Working has no badge: the spinner already says it.

  Label: nothing for 0, "1".."9", then "9+".

  Count badge:
    - capsule 12 px tall, at least 12 wide, at most 18 wide, horizontal padding 3
    - text 9 px bold rounded (Segoe UI Variable Display, weight 700), black, tabular digits
    - knockout: a 1.5 px ring of the pill colour (#000 dark / --pill light) around the capsule, so it reads over the track and the weekly ring
    - no hit testing; never pulses
    - appears with scale .4 plus opacity, spring .3/.8 (off under reduced motion)

  Placement, in the ring's own 44x44 frame (origin top-left, y down):
    centre = (22 + r cos a, 22 + r sin a)
    r = 22 + 11 * (ringDiameter/44) = 33 px for a count badge
    r = 22 (on the ring itself) for a compact dot

  Angle a is clockwise from 3 o'clock:

    | edge   | needs-you        | review            |
    |--------|------------------|-------------------|
    | right  | 225° (up-left)   | 135° (down-left)  |
    | left   | 315° (up-right)  | 45° (down-right)  |
    | top    | 135° (down-left) | 45° (down-right)  |
    | bottom | 225° (up-left)   | 315° (up-right)   |

  In other words, the badges sit on the side facing the screen centre.
  Compact mode (the Mac camera strip only): a 7 px dot on the ring. Windows has no camera strip, so compact mode is never used.

  Windows care points:
    - .ringwrap and the pill's clip-path must not clip the badges. The open clip (--clip-open) already overhangs by 40/1 px; badges extend about 7 px beyond the ring box, which stays inside the 70 px pill on upright edges.
    - On the flat edges the 6 px gap to the .pct label means the badge centre at y≈45.3 (±6) can touch the label. Check it visually, or nudge the label.
  VoiceOver text on the cell, becoming aria-label/aria-description on Windows: "2 need you, 1 to review, 3 working, 1 failed" (ClaudeRingDecoration.swift:41-49).

3.4 Resting marks on the folded notch (ClaudeRestingMarks.swift; ClaudeRestingMarkLayout, ClaudeRingBadgeLayout.swift:118-177; ClaudeAttentionPolicy.restingMarks :282-292)
  Setting: "Dots on the folded notch" (claudeControl.restingMarks, default on).

  At most one of each mark, in this order:
    needsYou  an amber BAR 9 px long along the pill x 4 px (a bar, so it differs by shape and not only hue)
    review    a green dot, 4 px
    working   a white dot, 4 px

  Layout:
    - gap 3 px; the run is centred on the pill's centre along the edge
    - across: centred in the visible depth of the pill
    - fits only if run+2 <= pill length and 5 <= visible depth. The Windows #rest pill is 10x79 px, so it fits.
    - on a side edge the bar stands upright

  Motion:
    - the amber bar dims at once, then breathes to bright over 7 half-cycles of 0.8 s, opacity .3 to 1
    - it breathes again whenever the needs-you count changes
    - marks fade out 0.1 s when the notch opens; they fade back in 0.25 s after a 0.3 s delay once folded; changes to the set fade .25 s
  Only sessions on rings shown in the notch count (ClaudeAttentionPolicy.notchValues :240-254).
  Windows: draw them inside #rest, or as siblings positioned over it (notch.html:171-178). Use pointer-events:none and aria-hidden.

3.5 Hover-card session rows for Claude rings
  Rows come from ClaudeHostProjections.activityRow, ClaudeHostProjections.swift:325-381. Fields: name, detail, state, waitingFor, since, pid.

  | state   | row detail |
  |---------|------------|
  | busy    | "Waiting on <x>" or "3/7" plus the active task (48 chars max), or "<tool>…", or "Thinking…"; then "ctx 42%"; joined with " · " |
  | waiting | detail = "<hostApp> · <project>"; waitingFor = the input summary |
  | success | "Ready for review · <project> · N in background" |
  | idle    | "<hostApp> · <project>" |
  | failed turn | state idle, detail "Stopped · <reason>" (no amber) |

  Mac row look (TooltipCard.swift:980-1030):
    line 1: name, and on the right a StatusRing plus the state word in the state colour
            words: working (white), waiting (amber), complete (green), idle (secondary)
    line 2: detail (or waitingFor while waiting) in secondary ink, and the elapsed time on the right
    order: waiting, busy, success, idle; newest first within each
    as many rows as fit, then "and N more"
    a hairline separates the rows from the limit windows above

  Click (ClaudeBridge.swift:224-248; ClaudeAttentionPolicy.sessionClick :216-224), by setting "Clicking a session in the hover card":
    Smart (default)  opens the panel at the session if it needs you; otherwise jumps to its terminal (which marks it reviewed); if there is no terminal, opens the panel
    Panel
    Terminal
  Windows: replace renderCard's non-clickable rows (notch.html:1210-1219) with this layout; each row becomes a button. The card is already in the hot rects.

3.6 Ring click (ClaudeBridge.swift:252-280; ClaudePanelPolicy.ringClick)
  Setting "Clicking a Claude ring":
    "Opens its sessions" (openPanel, default)
      - closed panel: open the list filtered to that ring
      - same ring and same notch: close
      - another ring: switch to it
    "Refreshes its usage"
      - usage probe plus refetch (the current Windows behaviour, refreshRing)
  Windows: in the mouseup handler (notch.html:1432-1436), a Claude cell asks Rust to toggle the panel with the ring id and the ring's screen rect, instead of calling refreshRing. The right-click menu should also gain "Open sessions panel".

3.7 Hold open and pin
  - While the panel is open, the anchor notch is pinned open and hover cards are suppressed on every notch (ClaudePanelController setHoverCardsSuppressed; ClaudeNotchHold.setPanel).
    Windows: set folded=false and suppress showCard() while the panel is open; restore both on close.
  - While any shown session needs you, the notch is held unfolded if ClaudePanelPolicy.holdsNotchOpen(policy, needsYou, isFlushWithHardware, userHides, userAlwaysShows) says so:
      auto   holds only notches flush with the camera
      always holds
      never  does not
    Windows has no camera, so "auto" would never hold. Offer Never and Always only (default Never, since the resting marks show on the folded pill), or map Auto to Never. Drop the camera sentence from the copy.

3.8 Attention reactions (ClaudeAttentionPolicy.decide :91-117, merge :124-146, Burst :168-202; ClaudeAttentionReactions.swift)
  Per transition:
    - If the session's own terminal is focused: nothing at all.
    - needsInput (not failed): the "blocked" chime. Then auto-open the panel if autoOpen != never, no terminal is visible, nothing is full screen and the panel is closed. Otherwise peek, if peeks are on, the panel is closed and the ring is shown.
    - failed turn: the "finished" chime and a peek only. Never an auto-open.
    - readyForReview: the "finished" chime; auto-open only when autoOpen == needsInputOrDone; otherwise peek.
    - resolved: nothing.
  Transitions within 0.5 s form one burst:
    - one chime; the blocked chime wins
    - one auto-open or one peek; auto-open wins
    - needsInput outranks readyForReview; the newest of equals wins
  A peek click counts as the peek's for its duration plus 2 s (peekClickGrace).

  Windows mapping:
    chime            PlaySound (system alias) or a bundled WAV. Windows upstream has no sound or peek settings, so add "Play a sound" and "Open the notch when a session ends" plus a peek duration to the Claude Code pane.
    peek             unfold the notch and show the card for the peek duration
    terminalFocused  GetForegroundWindow, then its pid; check whether it is in the claude pid's ancestor chain (focus.rs chain_of/pid_hits_chain already do this)
    anyTerminalVisible  EnumWindows for visible, non-cloaked (DWMWA_CLOAKED) windows of terminal or editor processes: WindowsTerminal, OpenConsole/conhost, Code, Cursor, Windsurf, VSCodium, wezterm-gui, alacritty, pwsh or cmd host
    fullScreen       SHQueryUserNotificationState (QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE), or the foreground window rect equals the monitor rect

3.9 Dock badge
  "Needs-you count on the Dock icon" (dockBadge, default on). The label is the needs-you count, or nothing at 0 (ClaudeAttentionPolicy.dockBadgeLabel :270-273).
  Windows: the notch has skipTaskbar, so draw the count onto the tray icon (trayicon.rs) or use a taskbar overlay (ITaskbarList3::SetOverlayIcon). Copy becomes "Needs-you count on the tray icon".

==========================================================================================
4. SESSIONS PANEL WINDOW (placement, chrome, behaviour)
==========================================================================================
4.1 Geometry (ClaudePanelGeometry.swift). Port it verbatim to Rust or JS; it is pure.
  minimumHeight 220
  minimumWidth 200

  Width by edge and mode (:97-104):
    right/left: list 400, chat 440
    top/bottom and floating: list 440, chat 520

  Height cap (:107-109): list 680, chat 780. Also never more than the visible frame minus 2 x margin.
  Chrome: margin 8. Mac tailLength = NotchLayout.tailLength (28.2 pt), tailWidth = tailHeight (32.7 pt), corner = cardCorner (18.6).
  Windows equivalents: tail 32 (length) x 36 (width), corner 16, the same clip-path as #tail.

  ringPoint (:132-140): the ring centre along the notch (ringAlong from the window start), and across it tailTipInset in from the bezel.
    Windows: the tail tip is 69 CSS px from the screen edge on upright edges, times the Size zoom. On flat edges it is the far side of the pill plus about 1 px.
    The simplest source is the notch page itself: it sends the ring's rect in physical px, and Rust derives the anchor.

  beside(), used for right and left (:159-194):
    root  = ring.x - tail (right edge), clamped inside visible.maxX - margin; mirrored for left
    width = max(200, min(edgeWidth, root - (visible.minX + margin)))
    card y is centred on ring.y, clamped to [visible.minY + margin, visible.maxY - margin - h]
    window = card plus the tail strip on the notch side

  aboveOrBelow(), used for top and bottom (:200-231):
    x centred on ring.x and clamped
    top edge: card top = min(ring.y - tail, visible.maxY - margin); maxHeight = distance to the far edge; the card hangs below
    bottom edge: mirrored

  floating() (:235-244), used when the notch is hidden, there is no notch, or the ring is switched off:
    no tail; centred horizontally; top at visible.maxY - margin (just under the menu bar)
    Windows: top-centre of the pointer's monitor work area.

  tailOffset (:250-257): the tail's offset from the card centre, clamped to ±(cardLength/2 - corner - tailWidth/2), so the tail never runs into a rounded corner.
  "visibleFrame" on Windows is the monitor work area; the port already has work_insets and NOTCH_INSETS.
  The card is not scaled by the notch Size. Only the ring's position uses sizeScale.

4.2 Chrome (ClaudePanelChromeView.swift)
  - The card and tail are one outline (TooltipSilhouette): no seam, no window shadow.
  - The surface is Liquid Glass (dimmed) or a solid Palette.card #000. With Reduce Transparency: solid plus a 1 px ringTrack stroke.
  - Content is clipped to the card, rounded to the card corner.
  - Windows: a solid fill (--card #0a0a0a or #000 dark, white in light). Acrylic/Mica would fill the whole window rectangle and ignore the tail clip, so it cannot be used.
  - The tail direction follows the edge: the card sits to the left of a right-edge notch with the tail pointing right, and so on.

4.3 Window semantics (ClaudePanel.swift:15-43; ClaudePanelController.swift)
  Mac window: borderless, non-activating NSPanel, one level above the notch (statusBar+1), on all Spaces and over full-screen apps, not movable, no shadow. becomesKeyOnlyIfNeeded is true: clicking a row or button doesn't take key; a text field does.

  Opening (ClaudePanelController.open), by reason (OpenReason at :31):
    ringClick, hoverRow, peekClick, notification, hotKey and settings open it as key without activating the app, so the terminal stays frontmost.
    auto never takes key. It never replaces an already-open panel. It lands on the list, not the chat.
      Presentation, ClaudePanelPolicy.presentation: route .sessions(nil) with highlightedSessionID. The list unfolds that row's section, selects it and scrolls it to centre.
    notification (a banner click): the banner activated the app, so on close the front goes back to the last other app (returnsFrontOnClose).

  Closing:
    - Esc: in the chat, back to the list; in the list, close
    - the close button (✕); the same ring clicked again
    - a mouse-down outside, unless pinned (closesOnMouseDown: panel and notch clicks never close; other own windows and other apps do)
    - a successful jump to the terminal, unless pinned (state.onJumped)
    - a Space change, unless pinned
    - display unplugged
    - auto-close: an auto-opened panel closes 1 s after its cause is resolved, or max(peekDuration, 8 s) after it opened, whichever comes first, unless "engaged" (the pointer entered, or it became key) (ClaudePanelPolicy :autoCloseDeadline)

  Pin: "Keep open" in the header. Persisted as claudeControl.panelPinned.

  Re-anchoring: follows the notch 0.35 s after it moves (edge, size, nudge, rings, screens). The window animates 0.18 s on content height or width changes.
  Height: content reports its natural height, clamped to [220, cap] (ClaudePanelState.reportContentHeight :244-265). List and chat have different caps and widths; the route switch re-places the window.

  Editing keys: Cmd+A/C/V/X/Z and Shift+Cmd+Z are routed by the panel itself, because a non-activating key panel gets no menu equivalents.

  Hot key (ClaudeHotKey.swift; PanelHotKey): Off (default), ⌃⌥Space or ⌥⌘J (Carbon RegisterEventHotKey). Opens the panel for all accounts.

  Windows mapping, the hard part:
    - A second Tauri WebviewWindow labelled "panel": transparent, decorations false, alwaysOnTop, skipTaskbar, shadow false, resizable false, initially focus:false. It must stay above the notch window; re-assert HWND_TOPMOST after the notch's own topmost refresh (topmost.rs).
    - Non-activating: WS_EX_NOACTIVATE plus SW_SHOWNOACTIVATE for auto-open.
    - Key for clicks, the hot key or a text field: take foreground (SetForegroundWindow, legal while this app holds the click or hot-key input), after saving GetForegroundWindow(). On close, SetForegroundWindow(saved) so the terminal gets the keyboard back.
    - Outside clicks while not key: a WH_MOUSE_LL hook (no permission needed) or the cursor poll the notch already runs. When key, window blur suffices.
    - Space change: virtual desktop change (IVirtualDesktopManager). Optional; closing on a foreground change to another desktop is enough.
    - Hot key: RegisterHotKey through tauri-plugin-global-shortcut. Choices: Ctrl+Alt+Space, Ctrl+Alt+J (Win+Alt combos are mostly taken by the OS). Show an error when registration fails.
    - Resizing a transparent WebView2 window flickers. Resize instantly, not animated; grow the window before growing the content and shrink it after.

==========================================================================================
5. PANEL: LIST SCREEN
==========================================================================================
Outer layout (SessionsPanelContent.swift:98-195):
  header block: padding 12 on the sides and top, blockSpacing below
  a hairline in separator colour, inset 12 on each side
  the list: horizontal padding 4 (theme.padding - 8, so rows with their own 8 padding align text at 12), top 5.5, bottom 8
  the undo toast when present: padding 12
  The whole panel is one clock: elapsed labels refresh every 30 s.
  Everything scrolls in one ScrollView, with automatic scrollbars and bounce only when needed.
  While the pointer is over the list, row order is frozen (frozenOrder) so nothing moves under a click. Newcomers go to the end of their section.

5.1 Header (PanelHeader.swift). Snapshot: panel-every-state-520.png, top.
  Row 1: "Claude sessions" (title, primary, 1 line).
    Optional "Sealed" badge: caption semibold, accent text on accent at 16% capsule, padding 6x1.5.
    Right side, 2 apart: three icon buttons of 22x20, each 10.5 semibold, secondary ink, fill controlFill on hover or on, radius 6:
      pin / pin.fill     tooltip "Keep open" or "Keep open: on"
      gear (menu)        tooltip "Settings"
      xmark              tooltip "Close"
  Row 2: attention strip (shown when total > 0), 10 apart. Each item is a StatusRing (scale .85) plus text, caption medium, tabular digits:
    "N need you" (singular "1 needs you")  needsYou colour, half ring
    "N failed"                             critical, solid dot
    "N to review"                          review, full ring
    "N working"                            working (white), 3/4 arc
    "N idle"                               secondary, grey ring
    Only non-zero groups appear. "answerable" = needsInput - failed.
    Accessible text: "2 need you, 1 failed, 2 ready for review, 3 working, 4 idle".
  Row 3: ring filter chips, a flow layout with 5 spacing. Shown only when there is more than one tracked account and at least one has sessions.
    First chip "All <total>"; then one per account: "● <label> <count>".
    If needsYouCount > 0, append a 5 px amber dot plus the count in amber. The total is never tinted.
    Chip style: caption medium, padding h8, height 18, capsule.
      selected: fill controlFillHover, no border, label primary
      unselected: 1 px separator border, label secondary, fill controlFill on hover
    Labels are middle-truncated at 150. The count is secondary.
    Clicking a selected account chip goes back to All.
    Label collisions: AccountLabels.disambiguated appends " · <plan | email | folder>", for example "me@example.com · Max 20x".
  Gear menu (native menu):
    Picker "Open automatically": Never / When a session needs you / When one needs you or is done
    Toggle "Notify when a session needs you"
    Toggle "Notify when a session is done"
    ---
    "Mark all reviewed" (Ctrl+Shift+R; disabled with 0 to review)
    ---
    "Open Settings…"
    Windows: use a native Tauri Menu popup, like notchmenu.rs: CheckMenuItems as a radio group, plus checks.

5.2 Setup banners (SetupBanners.swift:36-109; copy in ConsentCopy :270-286). Stacked, blockSpacing apart, in the header block.

  (a) Consent card, shown when setup.needsHookConsent and not answered in this panel.
      Snapshots: panel-consent-520.png, settings-first-run-520.png.
      Box: padding 10, controlFill, radius 10.
      Header: terminal.fill icon (11, accent) plus "Turn on Claude Code control" (rowTitle semibold).
      Explanation (caption, secondary): "To show every session live and let you answer prompts from here, this app adds its hooks and a status-line wrapper to the settings.json of each folder Claude Code runs in. Nothing is written until you turn it on, and turning it off puts your status line back exactly."
      Scope line (caption, primary; Claude Parallel Profiles only): "Installs into ~/.claude and your VS Code workspaces' folders (3 now; new ones are set up automatically). Claude Parallel Profiles' account stores never get hooks."
      Files: monoCaption primary, head-truncated, at most 4, then "and N more".
        Example: "~/.claude-windows/5d1e0a7b3c21/settings.json (dotfiles · me@personal.dev)"
      If taking over: caption "Superpowered Vibe Notch's hooks are replaced, not doubled up, and the status line it wrapped is put back first." plus, when stores are cleaned, " Superpowered Vibe Notch also wrote into the account stores and ~/.claude-shared: its entries, its scripts and the settings.json files it created there are removed; nothing else is changed." followed by the cleanup file list (monoCaption, secondary).
      Keeps Vibe Notch: "Vibe Notch's hooks stay; remove them per account in Settings › Claude Code."
      Blocked: amber caption "Superpowered Vibe Notch is running and would write its hooks straight back. Quit it first." plus a "Quit it" button (tinted needsYou, compact).
      Buttons (right-aligned, 6 apart): "Not now" (secondary, regular) and "Turn on", or "Take over and turn on" (primary; disabled while blocked).
      Both buttons hide the card at once (didAnswerSetup).
        Not now  declineHooks: hookConsent=false, nothing written
        Turn on  grantHookConsent: writes hooks and the status-line wrapper into the listed files, with backups
  (b) Else, while Superpowered Vibe Notch is running: NoticeBanner.
      Icon exclamationmark.triangle.fill (needsYou).
      "Superpowered Vibe Notch is running" / "It installs its own hooks for the same sessions. Quit it to hand them over to this app."
      Button "Quit it" (tinted).
  (c) Scope notice, when setup.newInstallFolders is non-empty and consent is given. Snapshot: panel-scope-notice-520.
      Icon info.circle.fill (secondary).
      "Claude Code control now covers your VS Code workspaces"
      "This version puts its hooks and status line in 3 VS Code workspaces' folders too, and in new ones as they appear: Claude Parallel Profiles runs Claude Code there. Each settings.json has a backup beside it. Account stores never get hooks." (singular: "1 VS Code workspace's folder")
      Buttons stacked vertically: "OK" (acknowledgeScope) and "Turn off" (turnOffAfterScopeNotice), both secondary compact.
  (d) Socket error. Icon bolt.horizontal.circle.fill (critical).
      "Not receiving hook events" / "<error> Sessions still update from Claude Code's session files, without approvals."
      Example error: "The hook socket couldn't be opened (address in use)."
  (e) Control off (hooks disabled after consent). Icon pause.circle (secondary).
      "Claude Code control is off" / "Answer prompts where Claude Code runs (VS Code or the terminal). Sessions still show here and finish from their transcripts."
      Button "Turn on…" opens Settings.
      Otherwise, when some tracked accounts lack hooks: icon exclamationmark.circle.fill (needsYou).
      Title: "Hooks are missing in Work." / "…in Work and Side project." / "…in N accounts."
      Message: "Its|Their sessions still show here; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are back."
      Button "Settings…".
  NoticeBanner layout: icon 11 semibold in a 14 wide column, title body semibold primary, message caption secondary, one trailing action; padding 9, controlFill, radius 10.
  Windows copy: the Vibe Notch, Superpowered Vibe Notch and Vibe Island cases are Mac-only apps; drop them. Their Windows analogue is upstream Codenotch for Windows' own "codenotch-hook" entries in ~/.claude/settings.json (hooks_install.rs:22-33). Offer "Take over and turn on" for those, and remove them on take-over.

5.3 Sections (SessionList.swift; SessionSections.swift)
  Bucket order and titles (sectionTitle), with header ink:
    "Needs you"         needsYou
    "Ready for review"  review
    "Working"           white
    "Idle"              secondary
  Header: title (sectionTitle), count (caption medium, secondary, tabular), and a fold chevron (7.5 bold; tertiary, primary on hover; rotated -90° when folded).
  "Needs you" never folds and has no chevron.
  Header padding: h8, top 2 for the first and 11.5 otherwise, bottom 2; min height 18.
  "Ready for review", when unfolded, has a right-aligned "Mark all reviewed" button (quiet, compact). Tooltip: "Mark every session here reviewed (⌘⇧R). Undo for a few seconds after."
  Default folding: Idle folds when it has more than 3 sessions. Others fold only when the user folds them.
  A folded section is one summary line: caption secondary (primary on hover), padding 8x5, hover fill rowHover, radius 10.
    Text: "<t1>, <t2> and N more" (first 2 titles). A click unfolds.
  Ordering within a section:
    Needs you: answerable before failed; oldest wait first. Wait = activePermission.receivedAt, else lastActivity.
    Review: newest completion first.
    Working: longest running first (turnStartedAt ascending).
    Idle: most recent activity first.
    Ties by session id.
  Compact mode: when more than 8 rows are drawn (folded sections don't count), every row except "Needs you" rows goes to one line.
  Rows 2 apart (0 for the first in a section, and 0 in compact mode).

5.4 Row, regular form (SessionRow.swift:82-104). Snapshot: panel-every-state-520.png.
  Box: padding 8x7, radius 10.
    selected: fill rowSelection plus a 1 px rowSelectionStroke border
    hover: rowHover
  Click anywhere opens the chat. Buttons inside take their own clicks.
  Line 1:
    title (rowTitle, primary, 1 line, truncated at the tail)
    trailing slot: StatusTrail = StatusRing (8.5, stroke 1.6) plus elapsed (caption medium, tabular, in the state colour), 5 apart
  Under hover or selection, the trailing slot swaps (opacity, no reflow) for icon buttons:
    "checkmark.circle" in review colour, tooltip "Mark reviewed (⌘R)" (review rows), or "xmark.circle" "Dismiss (⌘R)" (failed rows)
    "arrow.up.forward.app" "<Show terminal|Show in editor> (⌘J)" when focusable
    "bubble.left" "Open chat (⏎)"

  StatusRing shapes (StatusRing.swift:24-44):
    working  3/4 arc spinning, white
    needs    half ring (top-right half, from 12 o'clock clockwise), amber, breathing
    error    solid dot, critical, inset 14%
    review   full ring, green
    idle     full ring, secondary
    All use round caps.

  Elapsed label (SessionRowContent.elapsed :141-154; UsageFormatter):
    needs    duration since waiting: "<1m", "45m", "2h 13m", "3d 5h"
    working  duration of the turn
    review   age: "just now", "Nm ago", "Nh ago", "Nd ago"
    idle     age of the last activity
  Line 2 (detail), SessionRowContent.detail :163-262. Rendered by SessionDetailView (SessionRow.swift:200-263).
    Permission, pending, with an active request: tool name (body semibold, amber), then the request in a code box (mono, primary, padding 6x4, controlFill, radius 6, selectable).
      The code box wraps fully when Allow is offered in the row; otherwise it is limited to 4 lines.
      Request text (PermissionPreview :400-421):
        Bash: the command
        else file_path, notebook_path or path shortened under ~
        else command, url, query or pattern
        else the first string field that isn't "description"
      Tool names are MCP-formatted.
    Permission seen only via a notification or the registry: "<Tool|Permission>" plus "waiting in the terminal".
    Question: "Asks" (semibold, amber) plus two spaces, then the question (primary), 2 lines. Fallback "A question for you".
    Plan: "Plan ready for approval" (primary).
    Elicitation or dialog: the reason's displayText, for example "Network access to registry.npmjs.org", "Figma needs you to pick a file", "Worker permission".
      Fallbacks: "Input requested" / "Waiting for you".
    Failed: the humanised error in critical, for example:
        "Rate limited · weekly limit resets Fri 9:00 AM"
      The window suffix appears only when a limit is exhausted (RateLimitReset :434-472); its phrase is "resets in 47m" within a day, else "resets <Wkday> <time>".
      Error names: Rate limited, Overloaded, Server error, Sign-in failed, Billing problem, Invalid request, Output limit reached, Turn failed.
    Working:
        "Compacting context…"
        "Waiting on <n workflows…>…"
        the active task's label
        the last tool (name plus input, secondary, 1 line)
        "Thinking…"
    Review: the last assistant message, else the last message, else "Finished"; primary, 2 lines, whitespace collapsed.
    Idle:
        a tool line
        "You: <message>"
        the last message
        "No messages yet"
      All secondary, 1 line.
  Line 3, the meta line (SessionMetaLine :281-337), top +1. Items are separated by a "·" (caption bold, tertiary), 6 apart:
    AccountTag: 5 px dot plus label (caption, secondary). Only when there are several accounts and no ring filter.
    TaskProgressBar: width 64, height 3.9, plus "3/7" (caption, secondary, tabular).
      Segmented when 12 or fewer tasks and each segment is at least 6.5 wide: done primary, in progress secondary, pending barTrack, 1.5 gap.
      Otherwise one continuous fill with an in-progress chip.
    ContextMeter: a 20 px bar plus "42% context".
      Colour: normal is a primary fill with secondary text; 80% and above amber; 90% and above critical.
      Tooltip "Context window 42% full".
    Project name (caption, secondary), when it differs from the title.
    "N background" with tooltip "N background task(s) still running".
    When space runs short: drop the project, then the word "context", then truncate the account label.
  Line 4, the action bar (RowActionBar.swift), top +3. Clicks are ignored until the request is "armed" (see 5.8).
    permission(always, needsReview):
      When Always is offered inline: the caption "Always: <description>" (secondary, 2 lines, right-aligned).
        Descriptions (PermissionSuggestionText, ChatApprovalBars.swift:17-68):
          "Don't ask again for Bash(npm run test:*) in this project (just you)"
          "Switch to accept-edits mode …"
          "Allow access to <dirs> …"
        Destinations: for this session / in this project (just you) / in this project (shared) / in all projects.
      When needsReview (more than 4 lines or 200 characters): "Too long to judge from here: review it whole first."
      Buttons (5 apart, right):
        "Deny" (secondary compact, "Deny (⌘⌫)")
        "Always" (secondary compact; only for a narrow rule: addRules/replaceRules to session or localSettings)
        "Allow" (primary compact, "Allow (⌘⏎)", 4 extra leading), or "Review…" (primary; opens the chat) when needsReview
    questionChips (a single single-select question with 1-4 options): one chip per option.
      Each chip: "<n>" (monoCaption, amber at 70%) plus the label, tinted needsYou compact.
      Tooltip "<description> (n)", or "Answer <label> (n)".
      Then "Other…" (quiet compact; opens the chat).
    answerInChat: "Answer…" (primary compact, "Answer in the chat (⌘⏎)").
    plan: "Review plan" (secondary, "Read the whole plan (⏎)") and "Approve" (primary, "Approve the plan and let Claude start (⌘⏎)").
    answerInTerminal: "Answer in the terminal" (caption, secondary), plus "Show terminal" (secondary compact) when focusable.
  Focus label: "Show in editor" when entrypoint == claude-vscode, else "Show terminal".
  VoiceOver sentence (SessionRowContent.accessibilityLabel :335-363): "title, state, detail, waiting 2m, 3 of 7 tasks done, now: X, account Work". Every answer is also exposed as an accessibility action.

5.5 Row, compact form (SessionRow.swift:108-143)
  One line, padding 8x4:
    title (body medium, primary)
    compact detail (caption, secondary; shown only if it fits whole):
      working: the detail text
      needs: the detail
      review/idle: the project
    CompactProgress when not hovered: task bar width 28, context "42%" alone, account dot (tooltip is the label)
    the trailing slot
  Snapshot: panel-busy-window-400.png (25 sessions).

5.6 Empty and ended states
  List empty (SessionList.swift:256-280): a scaled idle ring plus "No Claude sessions yet" (body medium) and "Start Claude Code in VS Code or a terminal. Sessions from every account show up here." (caption, secondary, centred), padding 28.
  Filtered to an empty account: "No sessions in this account" / "Choose All to see every account's sessions."
  Chat for a session that is gone (SessionsPanelContent.swift:77-94): back chevron, "Session ended", ✕, and "This session has ended or was cleared, so there is nothing more to show."

5.7 Mark all reviewed with undo (ClaudePanelState.swift:192-238; UndoReviewToast :365-390). Snapshot: panel-undo-520.
  Clicking marks every row of Ready for review. They leave the section at once (shown as reviewed) and commit after 5 s, or when the panel closes.
  The commit stamps the click time (BHV-9).
  Toast: a review ring, "Marked N reviewed" (body medium) and "Undo" (secondary compact, Ctrl+Z, tooltip "Put them back in Ready for review (⌘Z)"). Padding 10x7, controlFill, radius 10.

5.8 AnswerGate (AnswerGate.swift)
  A request can be answered only after it has been on screen for 0.35 s. It can be answered once; the answered id is remembered for 600 s.
  Requests already on screen when the panel opened count as armed.
  The engine also drops an answer whose toolUseId is no longer the pending one.
  Every button carries (sessionId, toolUseId).

5.9 Keyboard (ClaudeKeyRouter.swift:89-173; PanelKeys.swift)
  Windows mapping: Cmd becomes Ctrl, Option becomes Alt.

  | key                 | action |
  |---------------------|--------|
  | Up / Down           | move selection; the first press selects the first or last row; no wrap |
  | Enter               | open chat (in the composer: send). A bare Enter never approves anything. |
  | Ctrl+Enter          | primary action |
  | Ctrl+Alt+Enter      | Always allow (from the list only for an inline narrow rule on a short request; in the chat always) |
  | Ctrl+Backspace      | Deny (plan: Keep planning) |
  | 1-4                 | choose that option (list, questionChips) |
  | Ctrl+J              | jump to the terminal |
  | Ctrl+R              | mark reviewed / dismiss failure |
  | Ctrl+Shift+R        | mark all reviewed |
  | Ctrl+Z              | undo the mark-all |
  | Esc                 | back, then close |

  Primary action (Ctrl+Enter):
    permission: Allow (or open the chat when too long, from the list)
    plan: Approve plan
    question: open the chat (list only)
    none or terminal-only: Mark reviewed if reviewable

  While the composer is focused, plain keys and digits type; Ctrl combinations still route.
  A Ctrl combination with no command falls through to the field, so Ctrl+Backspace still deletes a word there.
  Windows specifics:
    - WebView2 browser accelerators collide: Ctrl+R/F5 reload, Ctrl+Shift+R, Ctrl+J downloads, Ctrl+P, Ctrl+F. Disable them with ICoreWebView2Settings3.AreBrowserAcceleratorKeysEnabled = false (through Tauri with_webview) and call preventDefault in keydown.
    - Ctrl+Alt equals AltGr on international layouts; it is harmless with Enter.

==========================================================================================
6. PANEL: CHAT SCREEN (route .session(id))
==========================================================================================
Structure (ChatView.swift:239-300): header, hairline, transcript (fills), hairline, bottom bar.
Width 440 (side edges) or 520. Height cap 780. Snapshots: chat-*.png.
Opening the chat marks the session reviewed (ChatView.swift:73). A session that finishes while its chat is open and the panel is shown is marked reviewed too (:99).
Chat history is loaded from the transcript file. Only the newest 150 items are drawn; "Show N earlier messages" (quiet compact, centred) adds pages. It is bottom-anchored and stays pinned to the bottom unless the reader scrolled up.

6.1 Header (ChatSessionHeader.swift). Padding h12, v9.
  Back chevron: icon button, "Back to sessions (Esc)", leading -6.
  Title: rowTitle semibold, 1 line.
  Subtitle: "● <account> · <subtitle>".
    The account tag appears only when there are several accounts.
    subtitle = the active task label (in primary ink) while working, else "<project> · N background" (secondary).
  Right side, 8 apart:
    Task summary button: capsule 18 tall, padding 6; TaskProgressBar width 30, "2/6", and a 7 pt chevron (rotated when open); fill when hovered or open.
      Tooltip "Now: <task>" or "Tasks".
    ContextMeter compact: "84%", coloured by level.
    "arrow.up.forward.app" icon: "<Show terminal|Show in editor> (⌘J)".
  Task board (open; snapshot chat-tasks-520):
    box padding 9, controlFill, radius 10
    "Tasks" and "2 of 6 done" (caption)
    rows 17 tall, 2 apart; scrolls past 8 rows (4 while an answer bar is showing)
    row marks: done = filled ring and dot, text struck through at 55% opacity; in progress = spinner, medium weight; pending = grey ring, secondary text
    The board closes when a request appears.

6.2 Transcript items (ChatView.swift:549-845), 12 apart, padding 12:
  user        right-aligned bubble (at least 48 leading space), markdown in the chat font, padding 11x7, radius 12, controlFillHover
  assistant   markdown in the chat font, full width, selectable; empty text draws nothing
  thinking    one italic secondary line cut at 90 characters plus "…" and a chevron; click expands
  toolCall
    header: status mark (7x7), then the tool name (body semibold; critical on error), then the summary (body, secondary, 1 line), then an expand chevron
    marks: running = spinner; waitingForApproval = amber half ring; success = grey ring; error or interrupted = critical dot
    summary (ToolCallSummary :717-756): subagent container "desc · N tools"; MCP args; for success the input's description / file name / first line of the command / pattern / query / url; the name is never repeated
    click toggles the result, except Edit, which always shows its diff
    Subagents: "+N earlier tool uses" plus the last 2
  image       right-aligned thumbnail, at most 240x240, radius 10; fallback "Image (<type>)"
  interrupted "Interrupted" in critical
  working indicator under the last item: spinner plus "Working…" or "Compacting context…" or "Waiting on …"
  loading     "Loading the conversation…" with a spinner
  empty       "No messages yet"

  Tool results (ToolResultViews.swift):
    CodeBox: controlFill, radius 8, optional mono header separated by a hairline
    File view: line numbers (tertiary, 26 wide), max 10 lines, "N more lines"
    Bash: stdout 15 lines, stderr 10 lines in critical, "No output", "Running in the background (<id>)", "exit N"
    Grep: files (10) / content (15) / "N files with matches" / "No matches"
    Glob: files (10)
    WebFetch: code plus 8 lines
    WebSearch: 5 results plus "and N more"
    Todo, Task, AskUserQuestion ("↳ answer"), ExitPlanMode (6 lines), MCP (server · tool plus key/value)
    Diffs: added in review colour at 12% fill with "+"; removed in critical at 12% with "−"; context secondary; mono caption; up to 12 changed lines then "…"
      Snapshot: chat-approval-520 (cookies.ts)
  Markdown (MarkdownRenderer.swift):
    headings: same size, bold for h1-h2 and semibold for others, 3 top padding
    paragraphs 8 apart
    lists: secondary bullets or numbers, 6 gap
    quotes: a separator-coloured bar plus secondary text
    code blocks: mono, primary, padding 9x7, controlFill, radius 8
    inline code: mono, no background
    links: accent
    rules: separator
    Windows: render with a bundled markdown parser and sanitise it. Transcript text is untrusted, so no raw HTML.

6.3 Bottom bar. Exactly one bar, in precedence order (ChatView.swift:286-376). Every bar uses ChatBar: padding 12, blockSpacing.
  1. A pending permission (by tool). It ignores clicks until armed.
     a) Other tools, ChatApprovalBar (ChatApprovalBars.swift:115-166). Snapshot: chat-approval-520.
        Title: amber half ring plus "<Tool> needs your permission" (body semibold).
        The request in a scroll box (max 132, mono, padding 8x6, controlFill, radius 8).
        When a suggestion exists: "Always allow: <description>." (caption, secondary).
        Buttons (regular): "Deny" (secondary), "Always allow" (secondary, only when a suggestion exists), "Allow" (primary).
     b) AskUserQuestion, ChatQuestionPanel. Snapshot: chat-question-other-520.
        Title: "Claude has a question" or "Claude has N questions".
        Questions scroll (max 300), 12 apart.
        Each question: header chip text (caption semibold, amber), the question (body medium), "Choose any" when multi-select.
        Options: radio (11 px; selected = 3.5 px ring stroke in primary) or checkbox (rounded 3, filled when checked).
          Title body medium, description caption secondary.
          Row padding 8x5, radius 8; selected fill rowSelection, hover rowHover.
        "Other": when chosen, a text field "Type your answer".
        Footer:
          "Pick an answer" / "Ready to send" / "k of N answered" / "Sent to Claude"
          "Show terminal" (secondary, when focusable)
          "Submit" or "Submit answers" (primary; disabled until every question has an answer, and after submit)
        Answer format (ChatQuestionForm.swift:144-186): {question text exactly as sent: label}. Multi-select labels join with ", " in option order, plus the Other text.
        Unparseable questions: the terminal-only bar with "Claude has a question" / "It can't be shown here. Answer it in the terminal."
     c) ExitPlanMode, ChatPlanApprovalBar. Snapshot: chat-plan-520.
        "Plan ready for approval"; the plan as markdown (max 280, padding 9, controlFill, radius 8).
        Footer: "Approving lets Claude start on it.", "Keep planning" (secondary) and "Approve plan" (primary).
        Keep planning denies with this reason:
          "The user reviewed the plan and wants to keep planning. Stay in plan mode and ask what to change before implementing."
  2. A dialog in the terminal (needs input, not an error). Snapshot: chat-terminal-only-520.
     Title: the reason's displayText.
     "Answer it in the terminal. Anything typed here would go straight into that dialog."
     "Show terminal" (primary when titled).
  3. No message route (replies can't be typed). Snapshot: chat-no-route-520.
     "Replies can be typed from here for sessions in tmux, iTerm2 and Terminal.", or "<engine reason>. Type in the terminal instead."
     "Show terminal" (secondary).
  4. Composer (ChatComposer, ChatApprovalBars.swift:246-297). Snapshots: chat-composer-520, chat-tasks-520.
     Field: "Reply to Claude", chat font, 1-5 lines. Enter sends; Shift+Enter or Alt+Enter adds a line.
       padding 9x6, radius 9, controlFill; a 1 px rowSelectionStroke border when focused; focused on appear.
     Send button: a 26 px circle with an arrow-up.
       enabled: primaryFill with onPrimary glyph
       empty: controlFill with a tertiary glyph, disabled
     Drafts are kept per session (state.drafts).
     Failure line in critical above the field (ChatComposerCopy :162-178):
       "Not sent: <reason>. Your message is kept."
       "Typed but not submitted: <reason>. Press Return in the terminal when it's safe."
       "Couldn't send to <Terminal>. Allow Automation for it in System Settings › Privacy & Security, or type in the terminal."
       "Couldn't reach the session's tmux pane. Type in the terminal instead."
  Windows copy: replace "tmux, iTerm2 and Terminal" and the Automation wording with whatever the Windows messenger can reach, for example "Windows Terminal and console windows". The composer is shown only when the engine reports a route.

==========================================================================================
7. SETTINGS: "CLAUDE CODE" PANE
==========================================================================================
Mac: a SwiftUI Form, .grouped style, in upstream Settings' sidebar (seam U11). ClaudeCodeSettingsHost.swift renders it dark with the notch accent.
The first launch opens Settings on this pane. The panel's gear, the consent banner and "Turn on…" all navigate here (ClaudeSettingsNavigation).
Windows: add a sidebar tab "Claude Code" to settings.html (a .row with a coloured .badge; Claude orange #D97757) and build the pane with the existing blocks:
  .sec for section titles, .group/.item/.item.cap for rows, .switch for Toggle, .seg for 2-4 option pickers, select for longer ones, .btn for buttons, .link for link buttons, #toast for "Copied".
Snapshots: settings-*-520.png; readable crops in crops2/.

Section order (SettingsPaneContent.swift:10-18, 34-53):

7.1 Consent card. Shown while hookConsent == nil or Vibe Notch is running.
  SettingsConsentCard :403-485: the same copy as 5.2a.
  Title is a headline; explanation callout secondary; scope callout primary; the file list is shown whole (not capped at 4).
  "Turn on" is emphasised (borderedProminent) and must NOT be the default button: a stray Enter must not edit every settings.json.
  If blocked: amber caption plus "Quit Superpowered Vibe Notch".
  Scope notice section (hookConsent == true with new folders): the same copy as 5.2c, plus a mono list of folders, for example "VS Code · dotfiles", "~/.claude-windows/c07a3f5e1d94". Buttons "Turn off" and "OK".

7.2 Accounts (AccountsSection.swift). Header "Accounts".
  Snapshots: settings-520 crops2 -0, settings-parallel-profiles, settings-unnamed-accounts.
  Empty: "No Claude Code accounts yet. Add the folder Claude Code uses, or create a new account."
  Per account row (:80-242):
    Line 1: a 9 px account dot (45% opacity when untracked) and the name (secondary when untracked, middle-truncated).
      Default caption: "Default", or with Parallel Profiles "In ~/.claude now" (tooltip: "Terminals outside VS Code run as this account now: Claude Parallel Profiles copies the focused VS Code window's account into ~/.claude.").
      Right side: "Ring in notch" (caption) and a small switch. It is disabled and off when untracked; tooltip "Needs Track sessions and hooks".
    Under it, indented 17:
      identity: "me@work.com · Max 20x", or "Not signed in"; with a single folder, "identity · folder"
      folder summary: "Runs in ~/.claude and 2 VS Code workspaces", or "Runs nowhere now"; then "Store (Claude Parallel Profiles): ~/.claude-me", or "Stores (…):\n  ~/a\n  ~/b"
      "Show folders" / "Hide folders" link
      Expanded folder list: title (a mono path head-truncated, or "VS Code · <project>"), role (Default / VS Code workspace / Folder / Folder · also a Claude Parallel Profiles account / Account store, tertiary), and state on the right:
        Hooks and live status line / Hooks installed / Hooks not installed / Hooks off / Not tracked / Checking… / Read only, never changed
      usage line (SettingsUsageLine :403-434):
        "5-hour 34% · weekly 41% · 4m ago" (", stale" when stale)
        or "Not checked while it isn't tracked" / "Not checked while its ring is off" / "No reading yet" / "Waiting for the first reading" / the status message / "Usage check failed: <m>" / "No limits reported"
    Status chips (caption medium, the ink at 14% capsule, padding 7x2):
      hook title (HookState :265-278):
        "Hooks in k of n folders" / "Hooks installed" (review) / "Hooks not installed" (amber) / "Hooks off" / "settings.json unreadable" (critical) / "Folder missing" / "No folder runs it now"
      "Live status line" (secondary)
      legacy "<App> hooks" (amber)
      "Vibe Island hooks" (secondary)
    Login conflict (amber): "Sessions here ran with different CLAUDE_CONFIG_DIR spellings, which Claude Code treats as separate logins. Start it the same way each time: Copy launch command." (VS Code variant: "…Start it from its VS Code window rather than by hand.")
    Launch guidance (VS Code-only accounts): "Runs in VS Code: pick it for a window from the Claude Parallel Profiles status bar item; that window's terminals run as it. For another terminal, focus one of its windows (the extension then copies it into ~/.claude) and run claude."
    Hook problem: amber, or critical when unreadable. For example:
      "Its sessions still show; answer their prompts where Claude Code runs (VS Code or the terminal) until the hooks are in."
      "Hooks are turned off (see Hooks below)."
      "Installing is off for this run (--no-install)."
    "Track sessions and hooks" switch. When off, the caption "Ring in notch needs Track sessions and hooks".
    Buttons (flow layout, 6 apart):
      "Rename…": an inline text field (placeholder = the default name) plus Save and Cancel; an empty name reverts to the default
      "Install hooks" / "Reinstall hooks": disabled without consent, hooks or install permission, while busy, or when the folder is missing
      "Remove <legacy> hooks"
      "Copy launch command": changes to "Copied" for 1.5 s; the tooltip is the command
      "Reveal in Finder"
      "Forget…": confirms inline with "Forget and remove hooks" (destructive) and Cancel, plus a caption:
        "This app's hooks are removed from its settings.json and it stops being tracked. The folder and its sessions are left alone."
        or the multi-folder variant.
  Suggestion rows: "Found ~/.claude-old" plus the reason (for example "Named like a backup copy."), with "Dismiss" and "Add".
  Unsigned folders: "Not signed in" plus "<list>: Claude Code runs here, but nobody is signed in yet. It gets a ring once someone signs in with /login." (VS Code variant too).
  Footer buttons: "Add existing folder…" (folder picker) and "New account…".
  New account flow (NewAccountForm :362-469):
    Parallel Profiles guidance: "Add an account in VS Code" with the NewAccountCopy text; buttons "Create a folder for the terminal…" and "Done".
    Naming: field "Name, for example work", Cancel, Create. Caption "Creates ~/.claude-<slug>, a separate Claude Code config folder you sign in to once."; an error shows in critical.
    Created: "Created ~/.claude-x" / "Run this in a terminal, then /login. It's on your clipboard." plus the command in mono, "Copy again" and "Done".
  Section footer caption (Parallel Profiles variant): "Ring in notch draws the account's usage ring. Track sessions and hooks lists its sessions here; switching it off also removes this app's hooks from that account's VS Code workspaces and folders, and its ring and usage checks with them. ~/.claude keeps them while another account is tracked, …"
  Windows copy:
    "Reveal in Finder" becomes "Show in Explorer"
    paths display as %USERPROFILE%\.claude-x or ~\.claude-x
    launch command (AccountModels.swift:148-151) becomes PowerShell $env:CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work'; claude, or cmd set "CLAUDE_CONFIG_DIR=…" && claude
    the folder picker is a native Windows dialog (tauri-plugin-dialog)

7.3 Hooks and status line (:108-188)
  If hookConsent == false:
    "Claude Code control is off"
    caption: "Nothing is written to any settings.json. Sessions still show from Claude Code's session files, without approvals or "done"."
    "Turn on…" re-opens the full consent card inline. It never writes blind.
  Switch "Hooks in tracked accounts". Caption = hooksSummary (SettingsPaneModel.swift:102-124), amber when not all are installed:
    "Turn on Claude Code control first." / "Off: no account has this app's hooks." / "Installed in 2 of 4 folders of 2 tracked accounts." / "Installed in all N tracked accounts." / "No tracked accounts."
    Disabled without consent, without install permission, or while busy.
  Switch "Live status line data": "Wraps each account's status line to read limits and context live. Turning it off restores your status line exactly."
  "Socket": mono path, middle-truncated, selectable. Windows: show the hook endpoint (named pipe or 127.0.0.1:<port>).
  "Claude Code": "Version 2.1.97" or "Not found yet", "Find automatically" (when a path was chosen) and "Choose…". Caption "Hooks are written for <path>." or "…for the oldest claude found, so every version reads them."
  Notice caption: "Last change: settings.json in <folder> (VS Code · checkout-web). The previous version is kept as <…>.bak."

7.4 Usage (:192-232)
  Picker "Check usage every": Off / 5 / 10 / 15 / 30 min. Use .seg or select.
    Caption UsageCheckCopy :490-495: "Every N min, only when nothing fresher has arrived, this app runs Claude Code's own usage check in each signed-in account (Claude Code may update its own files there); this app never reads your login token. To find claude it may ask your login shell once."
    Off: "Off: readings come only from live status lines and Claude Code's own cache."
  Switch "Also read Claude Desktop's cached usage" / "Claude Desktop keeps the limits it last saw on disk; no token is involved."
  One line per tracked account: a 7 px dot, the name, and the usage line on the right.
  "Refresh now", with a spinner while refreshing.

7.5 Cloud (CloudSection.swift; copy in CloudSettingsCopy :172-270)
  Snapshots: settings-cloud-{signed-out, signed-in, overridden, no-website}-520.
  "Website" (read-only): the host without https://, or "None". Caption when overridden: "Set by AGENTNOTCH_WEB_URL for this run."
  Signed out:
    "Not signed in" (or "Signing in…" with a spinner)
    caption "Opens Google's sign-in in your browser. The website sign-in is kept in a file only your user can read." / "Finish signing in in your browser." / "This build has no website, so it can't sign in or sync."
    an error in critical
    "Sign in with Google": disabled with no website or while signing in
  Signed in:
    Confirm row: "Signed in as <email>" / "The website sign-in is kept in a file only your user can read.", "Sign out…", then Cancel and "Sign out" (destructive).
    Switch "Sync sessions and usage": "Sends project folder names, session titles, models, times, token counts, cost, and usage limits for each signed-in Claude account (Claude Desktop's readings too | Claude Desktop's too, once reading its cache is on). Never file paths, prompts or your Claude login."
    Switch "Summarise finished sessions with Claude": disabled unless sync is on; shown on only when both are on. Long caption (summariesDetail) plus a note:
      "Needs sync." / "Not in this run: it can't start Claude Code." (amber) / "N sessions summarised on this Mac."
    Status row:
      "Sync is off" / "Nothing is sent while it is off."
      "Syncing…" / "Last sync failed" (error in critical) / "Last synced 3m ago" / "Not synced yet"
      detail "1 session and 4 usage readings to send."
      "Sync now": disabled when off or syncing
    Dashboard: "Your sessions and usage from every Mac and account you sync. Share an account there with a code: everyone in its pool sees what it was used for." plus "Remove summaries or delete synced data in the website's Settings." ("Settings" is a link).
      Buttons "Open dashboard" and "Share accounts…" (the pools page), disabled until a dashboard URL is known.
  Consent rules the UI must enforce: both switches are off after every sign-in, sign-out and website change; summaries ride on sync.
  Windows: the browser step opens the system browser and returns through the agentnotch:// protocol, registered by the installer (tauri-plugin-deep-link) or a loopback redirect. Change "every Mac" to "every computer".

7.6 Sessions and attention (:236-286)
  Picker "Open the sessions panel": Never / When a session needs you / When one needs you or is done.
    Caption is the policy detail plus " Never over a full-screen app.":
      "The rings and the chime still tell you."
      "When a session needs you, unless you're already in its terminal."
      "Also when a session is done, unless you're already in its terminal."
  Picker "Keep the notch open while a session needs you": Auto / Always / Never.
    Caption: "Auto keeps it open only beside the camera, where the folded notch can't show marks. There, finished and working sessions show only when you hover (or by their chime and peek)." Windows: see 3.7 and adapt.
  Switch "Counts on the Claude rings".
  Switch "Dots on the folded notch" / "Not beside the camera, where the folded notch is the camera housing itself." (drop the camera clause on Windows)
  Switch "Needs-you count on the Dock icon". Windows: the tray icon.
  Picker "Clicking a Claude ring": "Opens its sessions" / "Refreshes its usage".
  Picker "Clicking a session in the hover card": "Smart" / "Opens the panel" / "Shows the terminal". Caption "Smart opens the panel when the session needs you, and its terminal otherwise."
  Picker "Panel shortcut": Off / ⌃⌥Space / ⌥⌘J. Windows: Off / Ctrl+Alt+Space / Ctrl+Alt+J.
  Button "Open the sessions panel".

7.7 Notifications (:290-311)
  "Banner when a session needs you" and "Banner when a session is done".
  "macOS permission": "Allowed", or "Off in System Settings" (amber when banners are on) plus "Open…". Windows: "Windows notifications", read from ToastNotifier.Setting; ms-settings:notifications opens the settings page.
  Caption "Banners are silent: sounds and the notch's peek follow this app's Notifications settings." plus "Sounds and peek…". Windows has no such pane upstream, so host the sound and peek settings here.
  Banner content (NotificationService.swift):
    "<title> needs you" (subtitle: account; body e.g. "Approve Bash" / "Question · <header>" / "Plan ready for approval")
    "Done: <title>" ("Ready for review · <project>")
    "<title> stopped" ("Overloaded · retry in its terminal")
    limit: "Work: 3 sessions hit the limit · resets 14:05"
    Actions: "Open" and "Mark Reviewed" (review only).
    Windows: WinRT toasts with buttons, activated through the protocol or an AUMID.

7.8 Advanced
  "Session state" / "A plain-text line per session for bug reports: its title, state and tasks, and the start of a finished reply. Look it over before sharing it." with "Copy".
  "Review queue" / "Marks every finished session reviewed." with "Reset…", then Cancel and "Mark all reviewed" (destructive).
  The sealed badge "Sealed" appears in sealed runs.

7.9 General pane (updates, fork)
  "Check now" and "Install updates automatically". Source builds: the switch is disabled and "Check now" says where to download.
  Windows: the upstream General pane has "Updates" with "Check for updates" (updater.rs, tauri-plugin-updater). Its endpoint must point at the fork's releases (tauri.conf.json currently has vinzdg's endpoint and a placeholder pubkey).

==========================================================================================
8. EDGE MAPPING: MAC TO WINDOWS NOTCH
==========================================================================================
  | Mac edge | Windows notch | Panel placement | Badge angles | Resting marks | Width list/chat |
  |----------|---------------|-----------------|--------------|---------------|-----------------|
  | right | body[data-edge=right]: upright pill, window flush right | card to the left of the notch; tail points right at the ring's centre y; card centred on the ring, clamped to the work area ±8 | needs 225°, review 135° | bar upright | 400/440 |
  | left | mirrored | card to the right, tail left | needs 315°, review 45° | bar upright | 400/440 |
  | top (menu-bar band) | flat pill hanging from the top edge | card below; top = pill bottom + gap (Windows card uses 30 px) minus the tail; centred on ring x; tail up | needs 135°, review 45° | bar horizontal | 440/520 |
  | top beside the camera | n/a (no hardware notch); isFlushWithHardware is always false | | | | |
  | bottom | flat pill at the bottom, usually over the taskbar | card above; clamp with work_insets so it never sits under the taskbar | needs 225°, review 315° | bar horizontal | 440/520 |
  | notch hidden or ring off | Show: Hide | floating: top-centre of the pointer monitor's work area, no tail | | | 440/520 |

  Along-edge offset: ringAlong = the ring centre from the window start. On Windows, get it from the page: the rect of .cell[data-p=ring] .ringwrap times devicePixelRatio, plus the window position.
  Show-on-hover (folded) on Windows is the Mac's folded state: resting marks show there. The panel open or a hold (Always) forces an unfold.
  Alt+drag nudge and the move handle change the anchor, so re-place the panel after the notch_edge event or a placement (Mac re-anchors 0.35 s after the move).

==========================================================================================
9. WHAT IS HARD / RISKS (UI-specific)
==========================================================================================
  1. The panel must be a second top-level window; the 360x650 notch window can't hold 400-520 px cards and its click-through hot-rect model would fight a big interactive surface.
     - Keep it above the notch (topmost ordering).
     - Convert coordinates correctly across per-monitor DPI and the notch Size zoom.
     - The notch window re-asserts TOPMOST (topmost.rs); the panel must too.
  2. Non-activating behaviour: see 4.3. WebView2 focus inside a WS_EX_NOACTIVATE window, taking focus only for text fields or a click-open, giving the foreground back on close. Getting this wrong steals keystrokes from the terminal.
  3. Outside-click dismissal while not key needs a WH_MOUSE_LL hook or a cursor/button poll.
  4. Window resizing per content height (220-680/780) and per width switch (list to chat) without flicker on transparent WebView2.
  5. Browser accelerator keys in WebView2 (Ctrl+R, Ctrl+J, Ctrl+Shift+R) collide with panel shortcuts; disable them.
  6. Compositor cost: the upstream notch uses steps() animations; the panel's spinners and breaths must too. No infinite smooth animations; the breaths are finite (7).
  7. Row reordering animation and frozen order under the pointer (SessionsPanelContent.swift:219-221, 184-194) need FLIP.
  8. The AnswerGate (0.35 s arm, answer once) must be reproduced exactly, with the engine re-checking toolUseId.
  9. The markdown, diff and tool-result renderers are a sizeable port (ToolResultViews.swift is 685 lines). All text is untrusted: escape it, and have no HTML passthrough.
  10. Badges on flat edges may overlap the .pct label; resting marks must go into #rest while it is clipped and fading.
  11. Mac-only concepts to drop or adapt:
      Vibe Notch, Superpowered Vibe Notch and Vibe Island (use upstream Windows codenotch-hook take-over instead)
      the camera strip, the Dock, Spaces, Automation permission
      tmux/iTerm2/Terminal wording
      Finder
      ⌘/⌥ glyphs in tooltips (the Mac tooltips embed "(⌘⏎)" and similar; rewrite them as "(Ctrl+Enter)")
  12. The upstream Windows Claude path reads .credentials.json and calls the OAuth usage API (usage.rs). This violates the fork's token-free rule. Remove it together with the card's "Sign in" button and credentials-based naming.
  13. Upstream Windows has no chime, peek or toast infrastructure; these must be built (PlaySound, a timed unfold plus card, WinRT toasts with actions).
  14. Light theme: Windows supports Light/Dark/System. The Mac panel is dark on the solid surface and follows appearance only on glass. Provide both token sets (1.1-1.2) and use the notch's resolved theme.

==========================================================================================
10. SNAPSHOT INVENTORY (in the snapshots folder; each at -400 and -520)
==========================================================================================
  panel-every-state
      every row kind: plan (Review plan/Approve), question chips (1 Recharts, 2 Chart.js, 3 ECharts, Other…),
      terminal-only dialogs (Network access…, Figma needs you to pick a file, Worker permission) with Show terminal,
      Bash permission (code box, 3/7 tasks, Always caption, Deny/Always/Allow),
      Answer…, rate-limited failure, compact review/working rows with task and context meters,
      "Mark all reviewed", folded Idle summary, account chips "All 20 ●7 · Personal 10 ●4 · Work 10 ●3"
  panel-needs-you      needs-you focus
  panel-regular-rows   9 sessions, non-compact rows, sections unfolded
  panel-busy-full      a long list, full height
  panel-busy-window    25 sessions, compact rows, 4 accounts, chips wrap, the list scrolls
  panel-keyboard-folded  a selected row (outline) with hover actions (open terminal, chat) and Working/Idle folded
  panel-filtered       the Team chip selected; no account tags on rows
  panel-undo           the "Marked 2 reviewed · Undo" toast
  panel-banners        the Vibe Notch running banner (Quit it), the socket error, "Hooks are missing in Work." (Settings…)
  panel-consent        the consent card with take-over copy and cleanup list; the empty list below it
  panel-scope-notice   the scope notice with OK / Turn off
  panel-empty          header plus empty state only
  chat-approval        header with tasks and context; transcript with thinking, Read, markdown list, Edit diff, Bash; the permission bar
  chat-plan            the plan approval bar
  chat-question-other  the question panel with Other text and Submit
  chat-tasks           the open task board and a working indicator; composer placeholder, send disabled
  chat-composer        a draft typed, send enabled
  chat-terminal-only   the dialog-in-terminal bar
  chat-no-route        the "Replies can be typed from here…" bar
  settings             the full pane
  settings-first-run   the consent card at the top
  settings-parallel-profiles  folders expanded, the new-account guidance
  settings-unnamed-accounts   engine-named accounts ("Claude Company", "Claude Personal")
  settings-scope-notice
  settings-cloud-signed-out, -signed-in, -overridden, -no-website

Not rendered (they need a sealed app launch): notch-<edge>-open/card/folded, ring-badges, ring-settle and panel-chrome-<edge>. Their content is described in sections 3 and 4 from ClaudeAppSnapshots+Notch.swift and +Panel.swift.
