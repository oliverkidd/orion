//! Kitty keyboard protocol negotiation, tmux-style: the child talks to a
//! virtual terminal (the DAEMON's ring + client-side vt100 parser, or the
//! TUI's own parser behind the BUILT-IN EDITOR), so nobody would ever
//! answer its `CSI ? u` support query. We scan the output stream here,
//! answer queries ourselves, and track the child's flag stack so whoever
//! encodes keys for it knows how.
//!
//! Also answers DA1 (`CSI c`) — the common detection recipe is "send the
//! kitty query then DA1, protocol is supported iff the kitty reply arrives
//! before the DA1 reply", which needs a DA1 reply to terminate promptly.
//!
//! And the DSR queries: device status (`CSI 5 n`) is always OK, while a
//! cursor position report (`CSI 6 n`) needs a screen, so the scanner only
//! marks where in the chunk it was asked and the caller answers it off its
//! own screen (the daemon's `pty::cursor`, the editor modal's parser).
//! Replies stay in query order either way. A child that waits for these
//! answers before it draws — Microsoft Edit asks for DA1 and a cursor
//! report at startup — would otherwise hang on a blank screen.
//!
//! And the REPLAYED MODES: bracketed paste (`CSI ? 2004 h` / `l`), the
//! alternate screen (`?1049`, `?1047`, `?47`), mouse tracking (`?1000`,
//! `?1002`, `?1003`) and its encoding (`?1005`, `?1006`, `?1015`). A client
//! reads each off its own parser — to paste the way a terminal would, and
//! to hand the wheel to a program that asked for the mouse rather than
//! scroll its own history — but a child sets them once, claude as it opens
//! its fullscreen screen, and a ring that has wrapped since no longer holds
//! the sets, so the replay has to restore them (`PtySession::snapshot`).
//! Lose them and a long session's pane no longer scrolls: the wheel goes
//! to a history a fullscreen program never wrote into.

const ESC: u8 = 0x1b;

/// Max nesting the spec suggests implementations may cap the stack at.
const MAX_STACK: usize = 32;
/// A real kitty sequence has short params; anything longer is not for us.
const MAX_PARAMS: usize = 16;

/// What the pump should do in response to scanned output.
#[derive(Debug, Default, PartialEq)]
pub struct ScanActions {
    /// Replies owed to the child's stdin, in the order it asked — detection
    /// recipes key on which reply arrives first.
    pub replies: Vec<Reply>,
    /// Set when the effective flags changed; broadcast to attached clients.
    pub flags_changed: Option<u8>,
}

/// A reply to one query; adjacent byte replies are merged.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Known from the scanner alone: kitty flags, DA1, device status.
    Bytes(Vec<u8>),
    /// A cursor position report on the screen as it stood `at` bytes into
    /// the fed chunk, just past the query. The pump fills it in.
    CursorPosition { at: usize },
}

impl ScanActions {
    fn reply_bytes(&mut self, bytes: &[u8]) {
        match self.replies.last_mut() {
            Some(Reply::Bytes(pending)) => pending.extend_from_slice(bytes),
            _ => self.replies.push(Reply::Bytes(bytes.to_vec())),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    Ground,
    Esc,
    /// Inside a CSI; params (0x30..=0x3F) collect, intermediates (0x20..=0x2F)
    /// or overflow poison the sequence (we only relay interesting finals).
    Csi {
        poisoned: bool,
    },
}

pub struct KittyScanner {
    state: State,
    params: Vec<u8>,
    stack: Vec<u8>,
    bracketed_paste: bool,
    /// The alternate screen mode set, if one is: 1049, 1047 or 47.
    alt_screen: Option<u16>,
    /// The mouse tracking mode set, if one is: 1000, 1002 or 1003.
    mouse_tracking: Option<u16>,
    /// The mouse encoding set, if one is: 1005, 1006 or 1015.
    mouse_encoding: Option<u16>,
}

impl Default for KittyScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl KittyScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: Vec::new(),
            stack: Vec::new(),
            bracketed_paste: false,
            alt_screen: None,
            mouse_tracking: None,
            mouse_encoding: None,
        }
    }

    /// Effective flags: top of the stack, or 0 (legacy) when empty.
    pub fn flags(&self) -> u8 {
        self.stack.last().copied().unwrap_or(0)
    }

    /// Has the child turned bracketed paste on (`CSI ? 2004 h`)?
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// The REPLAYED MODES the child has on, as the sets that turn them on:
    /// the alternate screen first, so what the replay draws after lands on
    /// it, then the mouse and its encoding, then bracketed paste. Empty
    /// when none is on.
    pub fn replay_modes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for mode in [self.alt_screen, self.mouse_tracking, self.mouse_encoding]
            .into_iter()
            .flatten()
        {
            out.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        }
        if self.bracketed_paste {
            out.extend_from_slice(b"\x1b[?2004h");
        }
        out
    }

    /// One DECSET (`on`) or DECRST of private mode `mode`, for the
    /// REPLAYED MODES. Setting one of a family replaces the family's last;
    /// resetting clears it when it is the one set.
    fn set_mode(&mut self, mode: u16, on: bool) {
        let slot = match mode {
            2004 => {
                self.bracketed_paste = on;
                return;
            }
            47 | 1047 | 1049 => &mut self.alt_screen,
            1000 | 1002 | 1003 => &mut self.mouse_tracking,
            1005 | 1006 | 1015 => &mut self.mouse_encoding,
            _ => return,
        };
        if on {
            *slot = Some(mode);
        } else if *slot == Some(mode) {
            *slot = None;
        }
    }

    /// Scan a chunk of child output. Sequences split across chunks are fine —
    /// the state machine persists between calls.
    pub fn feed(&mut self, data: &[u8]) -> ScanActions {
        let mut actions = ScanActions::default();
        let before = self.flags();
        for (i, &b) in data.iter().enumerate() {
            self.step(b, i + 1, &mut actions);
        }
        let after = self.flags();
        if after != before {
            actions.flags_changed = Some(after);
        }
        actions
    }

    /// `end` is the offset in the fed chunk just past `b`.
    fn step(&mut self, b: u8, end: usize, actions: &mut ScanActions) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => {
                if b == b'[' {
                    self.params.clear();
                    self.state = State::Csi { poisoned: false };
                } else {
                    // RIS (`ESC c`) puts every mode back to its default.
                    if b == b'c' {
                        self.bracketed_paste = false;
                        self.alt_screen = None;
                        self.mouse_tracking = None;
                        self.mouse_encoding = None;
                    }
                    // Includes ESC ESC; any other escape kind is not a CSI.
                    self.state = if b == ESC { State::Esc } else { State::Ground };
                }
            }
            State::Csi { poisoned } => match b {
                0x30..=0x3F => {
                    if self.params.len() < MAX_PARAMS {
                        self.params.push(b);
                    } else {
                        self.state = State::Csi { poisoned: true };
                    }
                }
                0x20..=0x2F => self.state = State::Csi { poisoned: true },
                0x40..=0x7E => {
                    if !poisoned {
                        self.dispatch(b, end, actions);
                    }
                    self.state = State::Ground;
                }
                // Cancelled / malformed sequence.
                _ => self.state = State::Ground,
            },
        }
    }

    fn dispatch(&mut self, final_byte: u8, end: usize, actions: &mut ScanActions) {
        let params = std::mem::take(&mut self.params);
        match final_byte {
            b'u' => match params.split_first() {
                // CSI ? u — "do you speak kitty?" Reply with current flags.
                Some((b'?', [])) => {
                    actions.reply_bytes(format!("\x1b[?{}u", self.flags()).as_bytes());
                }
                // CSI > flags u — push (flags default 0).
                Some((b'>', rest)) => {
                    let flags = parse_num(rest).unwrap_or(0) as u8;
                    if self.stack.len() < MAX_STACK {
                        self.stack.push(flags);
                    } else {
                        // Spec: at the cap, the oldest entry is evicted.
                        self.stack.remove(0);
                        self.stack.push(flags);
                    }
                }
                // CSI < n u — pop n (default 1).
                Some((b'<', rest)) => {
                    let n = parse_num(rest).unwrap_or(1).max(1) as usize;
                    for _ in 0..n {
                        if self.stack.pop().is_none() {
                            break;
                        }
                    }
                }
                // CSI = flags ; mode u — modify the current entry in place.
                Some((b'=', rest)) => {
                    let mut it = rest.split(|&b| b == b';');
                    let flags = it.next().and_then(parse_num).unwrap_or(0) as u8;
                    let mode = it.next().and_then(parse_num).unwrap_or(1);
                    if self.stack.is_empty() {
                        self.stack.push(0);
                    }
                    let top = self.stack.last_mut().expect("just ensured non-empty");
                    match mode {
                        1 => *top = flags,
                        2 => *top |= flags,
                        3 => *top &= !flags,
                        _ => {}
                    }
                }
                // Bare CSI u = SCO restore-cursor; not ours.
                _ => {}
            },
            // DECSET / DECRST, any number of modes at once
            // (`CSI ? 1004 ; 2004 h`): the REPLAYED MODES are tracked.
            b'h' | b'l' => {
                if let Some((b'?', modes)) = params.split_first() {
                    for mode in modes.split(|&b| b == b';').filter_map(parse_num) {
                        if let Ok(mode) = u16::try_from(mode) {
                            self.set_mode(mode, final_byte == b'h');
                        }
                    }
                }
            }
            // DA1 (CSI c / CSI 0 c): claim VT102 so detection loops terminate.
            b'c' if params.is_empty() || params == *b"0" => {
                actions.reply_bytes(b"\x1b[?6c");
            }
            // DSR 5 (device status): always OK.
            b'n' if params == *b"5" => actions.reply_bytes(b"\x1b[0n"),
            // DSR 6 (cursor position): the pump reads it off a screen.
            b'n' if params == *b"6" => actions.replies.push(Reply::CursorPosition { at: end }),
            _ => {}
        }
    }
}

fn parse_num(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 9 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(reply: &[u8]) -> Vec<Reply> {
        vec![Reply::Bytes(reply.to_vec())]
    }

    #[test]
    fn query_gets_reply_with_current_flags() {
        let mut s = KittyScanner::new();
        let a = s.feed(b"\x1b[?u");
        assert_eq!(a.replies, bytes(b"\x1b[?0u"));
        s.feed(b"\x1b[>5u");
        let a = s.feed(b"\x1b[?u");
        assert_eq!(a.replies, bytes(b"\x1b[?5u"));
    }

    #[test]
    fn push_pop_track_flags() {
        let mut s = KittyScanner::new();
        let a = s.feed(b"\x1b[>1u");
        assert_eq!(a.flags_changed, Some(1));
        assert_eq!(s.flags(), 1);
        let a = s.feed(b"\x1b[>15u");
        assert_eq!(a.flags_changed, Some(15));
        let a = s.feed(b"\x1b[<u");
        assert_eq!(a.flags_changed, Some(1));
        let a = s.feed(b"\x1b[<5u");
        assert_eq!(a.flags_changed, Some(0));
        assert_eq!(s.flags(), 0);
    }

    #[test]
    fn set_mode_modifies_in_place() {
        let mut s = KittyScanner::new();
        s.feed(b"\x1b[=5;1u");
        assert_eq!(s.flags(), 5);
        s.feed(b"\x1b[=2;2u");
        assert_eq!(s.flags(), 7);
        s.feed(b"\x1b[=1;3u");
        assert_eq!(s.flags(), 6);
    }

    #[test]
    fn sequences_split_across_chunks() {
        let mut s = KittyScanner::new();
        assert_eq!(s.feed(b"hello \x1b"), ScanActions::default());
        assert_eq!(s.feed(b"[>"), ScanActions::default());
        let a = s.feed(b"1u world");
        assert_eq!(a.flags_changed, Some(1));
    }

    #[test]
    fn unrelated_sequences_ignored() {
        let mut s = KittyScanner::new();
        // Colors, cursor moves, bare CSI u (SCO restore cursor), long params.
        let a = s.feed(b"\x1b[31mred\x1b[H\x1b[u\x1b[12345678901234567890u");
        assert_eq!(a, ScanActions::default());
        assert_eq!(s.flags(), 0);
    }

    /// The alternate screen, the mouse and its encoding are tracked as
    /// claude sets them on opening its fullscreen screen, and replay as the
    /// sets that turn them on, the screen first.
    #[test]
    fn replay_modes_follow_the_screen_and_the_mouse() {
        let mut s = KittyScanner::new();
        assert!(
            s.replay_modes().is_empty(),
            "nothing on until the child asks"
        );
        s.feed(b"\x1b[?1049h\x1b[?10");
        s.feed(b"00h\x1b[?1006h\x1b[?2004h frame");
        assert_eq!(
            s.replay_modes(),
            b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h"
        );
        // A later tracking mode replaces the first; several at once count.
        s.feed(b"\x1b[?1002;1015h");
        assert_eq!(
            s.replay_modes(),
            b"\x1b[?1049h\x1b[?1002h\x1b[?1015h\x1b[?2004h"
        );
        // Resetting a mode that is not the one set changes nothing.
        s.feed(b"\x1b[?1000l");
        assert!(s.replay_modes().starts_with(b"\x1b[?1049h\x1b[?1002h"));
        // Leaving the screen and the mouse, as claude does on exit.
        s.feed(b"\x1b[?1002l\x1b[?1015l\x1b[?1049l");
        assert_eq!(s.replay_modes(), b"\x1b[?2004h");
        // RIS drops every mode.
        s.feed(b"\x1b[?1049h\x1b[?1003h\x1bc");
        assert!(s.replay_modes().is_empty());
    }

    #[test]
    fn bracketed_paste_follows_decset_2004() {
        let mut s = KittyScanner::new();
        assert!(!s.bracketed_paste(), "off until the child asks");
        // Split across chunks, alongside other modes, as claude sets it.
        s.feed(b"\x1b[?25l\x1b[?10");
        s.feed(b"04;2004h prompt");
        assert!(s.bracketed_paste());
        // A mode that is not 2004, or the ANSI (non-`?`) form, changes nothing.
        s.feed(b"\x1b[?1004l\x1b[2004l\x1b[?20045l");
        assert!(s.bracketed_paste());
        // A shell running a command turns it off before the command reads.
        s.feed(b"\x1b[?2004l");
        assert!(!s.bracketed_paste());
        s.feed(b"\x1b[?2004h");
        // RIS (`reset`) drops it with every other mode.
        s.feed(b"\x1bc");
        assert!(!s.bracketed_paste());
    }

    #[test]
    fn da1_gets_vt102_reply() {
        let mut s = KittyScanner::new();
        assert_eq!(s.feed(b"\x1b[c").replies, bytes(b"\x1b[?6c"));
        assert_eq!(s.feed(b"\x1b[0c").replies, bytes(b"\x1b[?6c"));
        // DA2 / DA-with-args are not answered.
        assert_eq!(s.feed(b"\x1b[>c"), ScanActions::default());
    }

    #[test]
    fn device_status_is_ok() {
        let mut s = KittyScanner::new();
        assert_eq!(s.feed(b"\x1b[5n").replies, bytes(b"\x1b[0n"));
        // DEC's private form is not answered.
        assert_eq!(s.feed(b"\x1b[?6n"), ScanActions::default());
    }

    #[test]
    fn cursor_position_is_left_to_the_pump_in_query_order() {
        let mut s = KittyScanner::new();
        // `at` points just past the query's final byte, and the report keeps
        // its place between the byte replies around it.
        let a = s.feed(b"\x1b[?u\x1b[cab\x1b[6ncd\x1b[c");
        assert_eq!(
            a.replies,
            [
                Reply::Bytes(b"\x1b[?0u\x1b[?6c".to_vec()),
                Reply::CursorPosition { at: 13 },
                Reply::Bytes(b"\x1b[?6c".to_vec()),
            ]
        );
        // Split across chunks, `at` is into the chunk that finished it.
        assert_eq!(s.feed(b"\x1b["), ScanActions::default());
        assert_eq!(s.feed(b"6n").replies, [Reply::CursorPosition { at: 2 }]);
    }
}
