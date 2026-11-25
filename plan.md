# FMX UI improvement plan

A plan to make FMX feel clean, calm and fast. It is based on building the
debug app (v0.5.1, branch `fix/office-reader-text`) and capturing 27
screens with `FMX_SCREENSHOT` (Light, Dark, Hebrew/RTL, German, small window,
first run, Settings, editor, queue, dialogs). The source and
`packaging/audit-ui.py` were also checked.

The current app is a solid base. It uses an Adwaita-like palette, has good
contrast tests, correct RTL mirroring, sensible step cards and a quiet
Settings dialog. Most problems are about **density, consistency and
hierarchy**, not broken features. The aim is fewer things on screen, one
visual language, and the main action always in view.

---

## 1. Bugs found (fix first)

> **Status:** Phase 0 is done on branch `fix/ui-bug-sweep`. B1–B3 and B5–B14 are
> fixed. B4 was a false positive. B1's real cause was different from the guess
> in the table (see the note below it). Still open: German mixes informal "du"
> (about 9 strings) with formal "Sie" (about 34). That needs a German copy pass.

| # | Where | What happens | Likely fix |
|---|---|---|---|
| B1 | Main window, **light theme** | The sidebar/content and content/queue separators render almost black (`#2C2C2F`, the *text* color), not the border color (`#DEDEE0`). Dark theme is correct (`#444448`). This makes a harsh frame around the content. | The panel separator is painted with `widgets.hovered.fg_stroke` (egui `panel.rs:930`). Draw the separator yourself with `p.border`, or set panels to `show_separator_line(false)` and use the frame stroke. Check it with a real mouse too. |
| B2 | Screen recorder / Screenshot, action bar | Says *"1 job will be added to the queue and saved **next to the original files**"*. A recorder has no original files. | Use a capture-specific `ready_summary()` text: "Saves to ~/Videos" (show the real folder). |
| B3 | Screen recorder, step 1 | Step 1 "What gets recorded" shows a green ✓ (done) before the user did anything, and says "press Stop **in the queue**". The queue is hidden while the editor is open. | Make it an info card (no number or check). Show the Stop control in the action bar or a floating "Recording…" pill while recording. |
| B4 | Screen recorder, "Output format" row | ~~The subtitle "Stays playable even if recording stops unexpectedly" is clipped by the row below.~~ **False positive:** the row was at the edge of the scroll area. | `pref_row` must grow to fit the subtitle when it sits beside a segmented control. Add a layout test like `editor_panels_stay_within_available_space`. |
| B5 | Empty queue bar at narrow widths and in German | "More" wraps to a second line (900 px). In German the whole message wraps into a two-line paragraph. A 1-line bar becomes 3 lines. | Truncate the hint with an ellipsis, or hide it below about 900 px. Keep "More" on the trailing edge. |
| B6 | Sidebar search | The clear "×" is always visible, even when the field is empty, and it sits outside the field. In German the placeholder is cut off ("Werkzeuge such…"). | Put the clear button inside the field, shown only when there is text. Let the field use the full sidebar width. |
| B7 | Queue, waiting jobs | **Stop all** is enabled and red when nothing is running (only waiting jobs). | Disable it, or turn it into "Clear queue" when nothing is running. |
| B8 | Queue rows | **Cancel** (running job) and **Remove** (finished job) both use the same `X` icon. One is destructive and one is not. | Use `STOP`/`X_CIRCLE` for cancel and `X` or `TRASH` for remove. Add tooltips. |
| B9 | Branding | The window title, sidebar title and About page say "fmx". The welcome screen, Settings help and README say "FMX". There are 23 lowercase "fmx" strings in `locales/en.json`. | Choose **FMX** everywhere visible (`app.name`, window title, About page, strings). Keep `fmx` only for file and command names. |
| B10 | Copy casing | "When Finished" and "Core Programs" use Title Case. Every other heading is sentence case. | Change them to "When finished" and "Core programs" (all 6 locales). |
| B11 | Programs page | Two explanations say the same thing with different verbs: "click **Rescan**" and "press **Scan again**". The refresh button has no label. | Use one sentence and one verb that match the button's tooltip. |
| B12 | "When finished → Then" row | The label "Then" and the hint "Until fmx is closed; it starts as Do nothing." are hard to understand. | Use the label "After the queue finishes" and the hint "Resets to Do nothing next time FMX starts." |
| B13 | First run (debug build) | The right-click menu is installed for Nautilus, Nemo and KDE without asking. | Ask on the welcome screen's last step ("Add FMX to your file manager's right-click menu?") or make it opt-in in Settings. |
| B14 | Image preset dialog | "Max width" uses a grey checkbox labelled "Original". An unchecked box called "Original" is confusing. | Use a switch "Limit width" with a number field that appears when it is on, or a dropdown: *Original / 3840 / 1920 / 1280 / Custom*. |

> **B1 note:** the separators were fine. There is a 1 px seam between panels
> that showed eframe's default near-black `clear_color`. `FmxApp::clear_color`
> now returns the panel fill.

---

## 2. Experience problems (design debt)

### 2.1 Home and tool grid
- **Cards are big and nearly empty.** Each card is about 265×120 px and shows only a name such as "MP4". Only 6 tools fit above the fold. The next group ("Edit and combine") falls below the fold, so users don't see what else exists.
- **A "⋮" on every card** adds visual noise (18 dots on one page) for actions that few people use (favorite, preset).
- **Icon colors carry no meaning.** MP4 is blue, MKV purple, WebM green, AVI orange, but they all use the same file glyph. Glyphs also vary within a category without a rule: MP3 uses a ♪ note, M4A/AAC/OGG a file glyph, FLAC/WAV a waveform. Screen recorder and Sound recorder share the same red ● icon.
- **The page header repeats the sidebar.** A 40 px colored tile plus a heading plus a description show the same category that is already highlighted on the left.

### 2.2 Task editor
- **The main settings are below the fold.** At the default window size, step 2 "Choose your settings" is hidden for *every* tool (MP4, JPG, Edit tags, Trim). The user sees the file list and a big button but not what they are about to do.
- **Tool descriptions are long and refer to other parts of the UI.** MP4: *"Open 'Output settings' for codec, hardware encoding… and 'Edit' to clip, crop…"* That is 2–3 lines of hard-to-read centered text.
- **Duplicate add controls.** The "Add files…" button and the clickable drop zone do the same thing. The empty drop zone takes about 210 px.
- **File rows are inconsistent.** Video rows show 👁 preview, ⓘ info and ✕ remove. Image rows show only ✕, and there is no thumbnail even for a JPG. Audio rows show ⓘ and ✕. "Files selected: 1" compares with "Files selected: 2 · 0:07 in total".
- **The action bar is crowded.** A two-line status ("Ready: 2 jobs will be added to the queue and saved next to the original files.") competes with "📁 Change…" (change *what*?) and the primary button.
- **The disabled primary button is unreadable.** White text on pale blue (around 1.5:1 contrast). It looks broken rather than disabled.
- **The header actions are mysterious.** A bare ⚡ (presets) and ⋮ (a menu with one item, "Save as preset…") sit side by side.
- **The queue disappears in the editor.** You cannot see your running jobs while you set up the next one.

### 2.3 Small windows
- The sidebar stays 230 px wide at any width. At 800×560 the editor shows **one file row** between the header and the action bar.
- There is no compact mode, and the sidebar doesn't collapse.

### 2.4 Dialogs and modals
- Three dialog styles: Settings (centered title, close ✕), Image preset (left title, Cancel/Save), and Welcome (small left label "Welcome to FMX" and a plain-text "Skip guide").
- Two segmented-control styles: Settings → Appearance (white pill on grey) and Collage tabs (blue-tinted pill).
- The welcome guide is 4 text-heavy steps with thin outline icons and "Step 1 of 4" text. The default focus ring on "Next" shows a double outline.

### 2.5 Queue
- Rows show no output name or location, and no thumbnail.
- There is no overall progress (for example, "2 of 5 · about 1 min left") in the header or in the window title or taskbar.
- "⋮ More" is an icon plus a label, a pattern used nowhere else.

### 2.6 Window chrome
- The client-side title bar says "fmx" right above a sidebar header that also says "fmx". That uses 55 px of vertical space for a duplicated label.

---

## 3. Design direction: "calm, compact, confident"

1. **One accent, used rarely.** Blue only for the primary action, selection and focus. Category color shows *only* in the category's icon tile, not on every format card.
2. **Content first.** What the user is about to do (files + key settings + Go) fits in one screen at 1280×720 without scrolling.
3. **Progressive disclosure.** Basics are visible. Advanced options go in collapsibles, and rare actions go in menus or right-click.
4. **One component per job.** One dialog template, one segmented control, one row type, one icon size scale.
5. **Tokens, not literals.** There are 140+ literal `add_space`/size values today. Move them into named spacing and type tokens in `theme.rs`.

### Proposed tokens (`src/theme.rs`)
```rust
pub mod space { pub const XS: f32 = 4.0; pub const S: f32 = 8.0; pub const M: f32 = 12.0;
                pub const L: f32 = 16.0; pub const XL: f32 = 24.0; pub const XXL: f32 = 32.0; }
pub mod text  { pub const CAPTION: f32 = 12.0; pub const BODY: f32 = 14.0; pub const LABEL: f32 = 14.0;
                pub const TITLE: f32 = 17.0; pub const HEADING: f32 = 22.0; }
pub mod icon  { pub const S: f32 = 16.0; pub const M: f32 = 20.0; pub const TILE_S: f32 = 32.0; pub const TILE_L: f32 = 48.0; }
```
Add a `p.accent_disabled` color and a `p.on_accent_disabled` color that pass 3:1 contrast, and extend `text_meets_contrast_guidelines` to check them.

---

## 4. Roadmap

Each phase can ship on its own. Within a phase, items are listed in order.

### Phase 0: Bug sweep (about 1–2 days, low risk)
B1–B14 above. Add a test for each layout bug (B4, B5, B6) next to the existing
`*_stay_within_*` tests in `app.rs`.

### Phase 1: Foundations (about 2–3 days)
- [ ] Add the `space`/`text`/`icon` tokens. Replace literals in `app.rs`, `widgets.rs` and `tag_editor.rs` (mechanical).
- [ ] Add disabled-primary colors (fixes the unreadable "Add to queue").
- [ ] Make one dialog helper, `widgets::dialog(title, body, actions)`: centered title, close ✕, footer actions on the trailing side. Port Settings, Image preset, Save preset, Media info and Welcome to it.
- [ ] Use one `segmented()` style everywhere. Port the collage tabs to it.
- [ ] Use one row primitive for file rows, queue rows and pref rows (leading tile, title and subtitle, trailing actions), with consistent hover-reveal actions.
- [ ] Extend `audit-ui.py` to flag new `add_space(<literal>)`, `Color32::from_rgb` outside `theme.rs`/`art.rs`, and Title Case headings in `en.json`.

### Phase 2: Home and tool grid (about 3 days)
- [ ] **Compact format cards:** about 160×88 px with a 32 px tile, the name, and a one-line weak subtitle ("Most compatible", "Smaller files", "Lossless"). This fits about 12–15 tools above the fold.
- [ ] Remove the per-card ⋮. Show a ☆ favorite toggle on hover or focus, and put the other actions in the right-click menu.
- [ ] **Icon system:** each category has one hue (Video blue, Audio pink, Image green, Docs orange…). Glyphs say *what kind of thing* it is (container/codec/lossless/edit/capture) and follow one rule. Give Screen recorder and Sound recorder different glyphs (monitor vs. microphone).
- [ ] **Slimmer header:** category name plus description on one line, without the 40 px tile because the sidebar already shows it. Show the section heading counts ("Convert video · 6").
- [ ] Add a **"Recent"** row at the top of each category (the last 3 tools used). Most people repeat the same conversion.
- [ ] Put the search clear button inside the field (B6). Search keeps its `Ctrl+F` hint as a visible placeholder suffix.

### Phase 3: Task editor (about 4–5 days, the biggest win)
- [ ] **Two-pane layout at ≥ 1100 px:** files on the start side, settings on the end side, both visible at once. Below 1100 px, keep stacked steps but **collapse the file list to a summary chip** ("2 files · 0:07 ▾") once files are added, so step 2 is visible.
- [ ] **Short descriptions:** at most one line ("Plays everywhere. Good default for sharing."). Move the long text into a ⓘ tooltip on the title.
- [ ] **Empty state:** one big drop zone with the "Add files…" and "Add folder…" buttons *inside* it. Remove the duplicate button row.
- [ ] **File rows:** real thumbnails for images and video (there is already a preview pipeline in `preview.rs`), the same action set for every type, and actions shown on hover or focus.
- [ ] **Action bar:** `[ 📁 ~/Videos ▾ ]   2 files · ≈ 15 MB          [ Add to queue ]`. The folder becomes a labelled path button, and the status is one short line. Give "Start now" its own split-button option.
- [ ] **Header:** replace ⚡ + ⋮ with one labelled "Presets ▾" menu button (apply / save current / manage).
- [ ] **Mini queue:** while the editor is open and jobs are running, show a slim progress pill in the action bar ("2 running · 42%"). Clicking it opens the queue.

### Phase 4: Queue (about 2 days)
- [ ] Add an overall progress line in the header: "3 of 5 done · about 1 min left", and show the same progress in the window title and taskbar where the platform supports it.
- [ ] Rows show source → output name, a thumbnail, and on completion **Open** / **Show in folder** as visible buttons (not icon-only).
- [ ] Give finished, failed and cancelled jobs a subtle status stripe or color. Failed rows get "Show details" with the ffmpeg error text and Retry.
- [ ] Replace the "⋮ More" pattern with a plain ⋮ menu button: Save list, Load list, Clear finished.
- [ ] Fix B7/B8 here if they were not done in Phase 0.

### Phase 5: Responsive and chrome (about 2–3 days)
- [ ] **Breakpoints:** ≥ 1100 px full sidebar. 760–1100 px, the sidebar collapses to a 64 px icon rail with tooltips. Below 760 px, the sidebar becomes an overlay that a ☰ button opens.
- [ ] Remove the duplicated title: set the window title to the current page ("Convert to MP4 — FMX"). Where the platform allows it (macOS `fullsize_content_view`, Windows), draw the sidebar header into the title bar area.
- [ ] Add tests in the style of `settings_dialog_switches_navigation_between_narrow_and_wide_windows` for each breakpoint, and add `FMX_SCREENSHOT_SIZE` presets to CI as golden images (optional).

### Phase 6: First run and polish (about 2 days)
- [ ] **Welcome:** 3 steps, each with a filled illustration that uses the category hues, page dots instead of "Step 1 of 4", and a primary "Get started". The last step offers the right-click menu (B13) and the theme choice.
- [ ] **Empty home after the welcome:** a big drop target on the home page ("Drop any file — FMX suggests what to do"). This uses the existing `accepts()` logic to suggest matching tools.
- [ ] **Micro-interactions:** card hover lifts 1 px with a shadow, the progress bar is animated with a stripe while it is indeterminate, and a success toast has an "Open folder" action. All of them respect `reduce_motion`.
- [ ] **Keyboard:** visible focus on cards, arrow-key navigation in the grid, `Enter` opens a tool, `Ctrl+Enter` = Add to queue, `Esc` = back. Show the shortcuts in tooltips.
- [ ] **Copy pass:** every string in `en.json` is sentence case and uses "FMX", with one verb per action (Add / Remove / Clear / Rescan). Then sync the other 5 locales and run `audit-ui.py`.

---

## 5. How to check each change

```bash
cargo build
S=/tmp/fmx-shots; mkdir -p $S
# Isolated profile so real settings are untouched
export XDG_DATA_HOME=$S/data XDG_CONFIG_HOME=$S/config FMX_LANG=en
FMX_SCREENSHOT=$S/home.png        FMX_SCREENSHOT_SIZE=1280x800 target/debug/fmx
FMX_SCREENSHOT=$S/editor.png      FMX_SCREENSHOT_SIZE=1280x800 FMX_SCREENSHOT_TOOL=mp4 target/debug/fmx some.mp4
FMX_SCREENSHOT=$S/small.png       FMX_SCREENSHOT_SIZE=800x560  FMX_SCREENSHOT_TOOL=mp4 target/debug/fmx some.mp4
FMX_SCREENSHOT=$S/rtl.png         FMX_SCREENSHOT_SIZE=1280x800 FMX_LANG=he target/debug/fmx
FMX_SCREENSHOT=$S/queue.png       FMX_SCREENSHOT_SIZE=1280x800 FMX_SCREENSHOT_TOOL=mp4 FMX_SCREENSHOT_ENQUEUE=1 FMX_SCREENSHOT_RUNNING=1 target/debug/fmx some.mp4
cargo test && python3 packaging/audit-ui.py
```

**Done means:** at 1280×720 the editor shows files, key settings and the primary
action without scrolling for MP4, JPG, Trim and Edit tags. Light and dark look
the same apart from color. Hebrew mirrors correctly. German strings don't wrap
the queue bar. `audit-ui.py` reports no new literal spacing or color.

---

## 6. Suggested order

1. Phase 0 (bugs). Quick wins that are visible right away.
2. Phase 1 (tokens and components). This makes everything after it faster and consistent.
3. Phase 3 (editor). This has the largest effect on everyday use.
4. Phase 2 (grid), then Phases 4, 5 and 6.

Update the README screenshots (`assets/screenshot*.png`) and the website hero after Phase 3.
