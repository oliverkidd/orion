# Spotify now-playing in the footer

## Goal

A small readout in the footer bar, just left of the right-edge `8 agents · 44 MB`, showing what Spotify is playing with three clickable buttons:

```
… hints …                ♪ Midnight City · M83  ⏮ ⏸ ⏭   8 agents · 44 MB
```

- Read and control the local Spotify desktop app over AppleScript (`osascript`). No Spotify login, no Web API, no developer app.
- No search, no keyboard shortcuts. The Mac's media keys (F7–F9) already control Spotify system-wide; orion only shows state and offers mouse buttons.
- Hidden entirely when Spotify is not running, nothing is loaded, the platform isn't macOS, or the setting is off.

All paths below are relative to the repo root; `T = crates/orion-tui/src`.

## Behaviour

- **Playing:** `♪ <title> · <artist>  ⏮ ⏸ ⏭`. The title is in `th.text` and the artist in `th.dim`.
- **Paused:** the same, with `▶` in place of `⏸` and the title dimmed too.
- **Stopped, not running, or nothing loaded:** nothing drawn, no width reserved.
- **Width:**
  - The readout takes at most a third of the bar's width.
  - It shortens the artist first, then the title, using `truncate` with `…`.
  - It drops the artist entirely before shrinking the title below 12 chars.
  - Below `♪ ` + 8 title chars + the three buttons, it is hidden.
- **Hover/click:**
  - Each button underlines under the pointer, like the other footer buttons.
  - A click sends the command, then immediately re-polls so the glyph flips without waiting a second.
  - Clicking the title text does nothing. It is not a hit target.
- **Polling:**
  - Once a second while Spotify was last seen running.
  - Every 5 s while it isn't, so a closed Spotify costs almost nothing.
  - Never two `osascript` calls in flight at once.
- **Permission:**
  - The first call triggers macOS's "<terminal> wants to control Spotify" Automation prompt.
  - If the user denies it, AppleScript error `-1743` comes back.
  - On `-1743`, stop polling for the rest of the run and flash once: `Spotify: allow orion's terminal under System Settings → Privacy & Security → Automation`.
  - Must never launch Spotify. Every script guards with `application "Spotify" is running`.

## AppleScript

The poll uses one script whose output is a single tab-separated line, or empty when there is nothing to show:

```applescript
if application "Spotify" is running then
  tell application "Spotify"
    if player state is stopped then return ""
    set t to current track
    return (player state as text) & tab & (name of t) & tab & (artist of t)
  end tell
end if
return ""
```

Commands are each a guarded one-liner (`if application "Spotify" is running then tell application "Spotify" to <cmd>`), where `<cmd>` is one of:
- `previous track` (Spotify restarts the track when more than ~3 s in, as it does from the media key)
- `playpause`
- `next track`

Pass each script with `osascript -e <script>`, with `stdin(Stdio::null())` and stderr piped. Copy the pattern in `T/clipboard_image.rs:180-202` (`quiet` + error mapping on stderr).

## Steps

### 1. `T/spotify.rs`: new module (register it in `T/lib.rs`)

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowPlaying { pub playing: bool, pub title: String, pub artist: String }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button { Previous, PlayPause, Next }

pub enum Answer { Track(Option<NowPlaying>), Denied }

pub fn interval() -> Option<Duration>          // ORION_SPOTIFY_POLL_SECS; 0 = off; default 1 s; None off-macOS
pub fn spawn_poll(tx: UnboundedSender<Answer>)  // tokio::spawn + tokio::process::Command, 3 s timeout
pub fn spawn_command(b: Button, tx: UnboundedSender<Answer>) // run the command, then a poll, send the poll's answer
fn parse(stdout: &str) -> Option<NowPlaying>    // "playing\tTitle\tArtist" → Some; "" / malformed → None
fn denied(stderr: &str) -> bool                 // contains "-1743"
pub fn readout(np: &NowPlaying, max: usize) -> Option<(Vec<Span>, [Range<u16>; 3])> // spans + button column offsets, None when it can't fit
```

- Model `interval()` on `T/update_check.rs:31`.
- On non-macOS (`#[cfg(not(target_os = "macos"))]`), `interval()` returns `None` and nothing else runs.
- A timeout or any other error counts as `Track(None)`. It must never spam flashes.

### 2. Env var: `crates/orion-core/src/env.rs`

Add this next to `UPDATE_CHECK_SECS` (`env.rs:45`):

```rust
/// Cadence in seconds of the footer's Spotify poll; `0` turns it off, as the
/// e2e tests do so their footers never depend on what's playing.
pub const SPOTIFY_POLL_SECS: &str = "ORION_SPOTIFY_POLL_SECS";
```

Set `ORION_SPOTIFY_POLL_SECS=0` wherever the e2e harness already sets `ORION_UPDATE_CHECK_SECS=0`. Find it with `grep -rn UPDATE_CHECK_SECS crates/ scripts/`.

### 3. App state: `T/app.rs`

- Add `pub spotify: Option<spotify::NowPlaying>`, `pub spotify_in_flight: bool`, `pub spotify_denied: bool` and `pub spotify_enabled: bool` near `update_available` (`app.rs:3903`). Initialise them in `App::new`.
- Add `HitTarget::FooterSpotify(spotify::Button)` alongside the footer variants (`app.rs:201-212`).

### 4. Event loop: `T/event_loop.rs`

- Next to the update-check channel (`:394-396`), create `let (spotify_tx, mut spotify_rx) = unbounded_channel::<spotify::Answer>();` and `let mut next_spotify = Instant::now();`.
- Add a `select!` branch at `sleep_until(next_spotify), if spotify::interval().is_some() && app.spotify_enabled && !app.spotify_denied`:
  - If `!app.spotify_in_flight`, set it and call `spotify::spawn_poll(spotify_tx.clone())`.
  - Set `next_spotify = now + (if app.spotify.is_some() { interval } else { 5 s })`.
- Add a `select!` branch for `answer = spotify_rx.recv()`:
  - Clear `spotify_in_flight`.
  - `Track(t)`: set `app.dirty |= app.spotify != t; app.spotify = t;`
  - `Denied`: set `spotify_denied = true; app.spotify = None;` and flash the Automation message once.
- In `handle_mouse`'s footer arms (`:12382-12391`), make `Some(HitTarget::FooterSpotify(b))` call `spotify::spawn_command(b, tx)`. The sender must be reachable there: store a clone on `App` (e.g. `app.spotify_tx: Option<UnboundedSender<Answer>>`, set at startup). This mirrors how `app.git_sync.tx` is stored.
  - Optimistic flip: for `PlayPause`, toggle `app.spotify.playing` and mark dirty straight away.
- Add `HitTarget::FooterSpotify(_)` to the button-like `matches!` list in `update_pointer` (`:11570-11595`) so it underlines on hover.
- Add it to the exhaustive no-focus-change arm in `T/event_loop/focus_walk.rs` (~`:125-142`).
- In `apply_config` (~`:8490`), add `app.spotify_enabled = cfg.spotify;`. When it turns off, clear `app.spotify`.

### 5. Footer drawing: `T/ui/footer.rs`

In `draw_footer_bar` (`:500`), after computing `usage`/`right_w` (`:527-536`):
- Compute `spot_max = area.width / 3`, then call `spotify::readout(np, spot_max)`.
- Reserve its width plus 2 spaces from `left`, so the hints clip before it does.
- Draw its spans in a `Rect` immediately left of the usage rect.
- Push one hit rect per button onto `app.hits`, each 1 cell wide plus its trailing space, as `HitTarget::FooterSpotify(Button::…)`.
- Underline the button whose target equals `app.hover_crumb`, the same way the usage readout does (`:659-667`).

### 6. Setting: `T/config.rs`

Follow the checklist used for `animations`:
- Add `pub spotify: bool` to `Config` (`:1349`), defaulting to `true` (`:1970`).
- Add `SettingKind::Spotify` (`:503`) and an `added_on()` arm dated `2026-10-06`.
- Add a `SettingSpec` row in the Appearance tab next to Animations (`:911`):
  - label `Spotify in footer`
  - hint `Show what Spotify is playing, with ⏮ ⏸ ⏭ buttons (macOS)`
- Add a value arm (`on_off(self.spotify)`, ~`:3339`) and a toggle arm (~`:3503`).

### 7. Docs

- Add a line to `docs/configuration.md` for the setting.
- Add a line to `docs/configuration.md` (or wherever env vars are listed) for `ORION_SPOTIFY_POLL_SECS`.

## Tests

Unit tests in `T/spotify.rs`:
- `parse` handles playing, paused, empty, a title containing `·`, and malformed lines (fewer than 3 fields → `None`).
- `denied` matches `execution error: Not authorised to send Apple events to Spotify. (-1743)`.
- `readout`:
  - Fits: the button ranges point at `⏮`, `⏸`/`▶` and `⏭`.
  - Shrinking `max`: the artist goes first, and below the minimum it returns `None`.

Footer tests in `T/event_loop.rs`'s test module, modelled on `clicking_the_footer_readout_opens_the_memory_modal` (~`:19240`):
- With `app.spotify = Some(..)`, the footer shows `♪ Title · Artist` left of the usage readout on a 120-wide `TestBackend`.
- `hit_rect(&HitTarget::FooterSpotify(Button::PlayPause))` covers the `⏸` cell.
- A `Moved` event over it sets `hover_crumb`, and the cell is UNDERLINED after a redraw.
- With `app.spotify = None`, no `FooterSpotify` hit rects exist and the hints use the full width.
- At 60 wide, the readout shortens or hides, and the usage readout is still intact.
- Settings toggle test modelled on `animations_default_on_toggle_and_persist` (`config.rs:5006`).

## Verification

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p orion-tui spotify
cargo test -p orion-tui footer
cargo test --workspace
```

Manual check, with Spotify open and a track playing:
1. `cargo run`. The track appears in the footer within ~1 s. Allow the Automation prompt if asked.
2. Press F8. The footer flips `⏸`/`▶` within a second.
3. Click `⏭`. The title changes almost immediately.
4. Quit Spotify. The readout disappears within ~5 s, and Spotify does not relaunch.
5. Toggle Settings → Appearance → Spotify in footer off. The readout disappears and polling stops (check with `ps aux | grep osascript`).
