//! The addresses a RUN COMMAND prints for itself — `http://localhost:3000`
//! — read off its output as it streams, for the run line's `:3000` and
//! ⌘O's **Open app**.
//!
//! Startup lines scroll off a screen within seconds, so what was seen is
//! kept: a sticky set per run, in the order the ports first appeared. A
//! run started again is a new PTY, and a new scanner. Colour and other
//! escapes are stripped before matching — Vite bolds the port inside its
//! URL — and text is carried across chunks, so an address split by a read
//! boundary is still read whole.

use super::ESC;

/// The hosts that mean this machine, as a dev server prints them.
const HOSTS: [&[u8]; 3] = [b"localhost", b"127.0.0.1", b"0.0.0.0"];
const SCHEME: &[u8] = b"http://";
/// The most ports one run keeps: a stack prints a handful at most.
const MAX_PORTS: usize = 8;
/// Text carried from one chunk into the next: more than the longest
/// address (`http://127.0.0.1:65535`, 22 bytes), so one cut by a chunk
/// boundary is whole the next time round.
const CARRY: usize = 32;

/// Where the stripper is inside an escape sequence.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// Just read ESC.
    Esc,
    /// In a CSI (`ESC [`), until its final byte.
    Csi,
    /// In a string — OSC, DCS, APC, PM, SOS — until BEL or ST.
    Str,
    /// Read ESC inside a string: `\` ends it (ST).
    StrEsc,
}

/// Tracks the localhost ports one run has printed.
#[derive(Debug, Default)]
pub struct PortScanner {
    ports: Vec<u16>,
    /// Printable text not yet ruled out, escapes stripped.
    text: Vec<u8>,
    escape: Escape,
}

impl PortScanner {
    /// The ports seen so far, first seen first.
    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// Scan a chunk of output: true when it showed a port not seen before.
    pub fn feed(&mut self, data: &[u8]) -> bool {
        for &b in data {
            self.strip(b);
        }
        let grew = self.scan();
        if self.text.len() > CARRY {
            self.text.drain(..self.text.len() - CARRY);
        }
        grew
    }

    /// One byte through the escape stripper: printable text lands in
    /// `text`, a control byte as the space that ends an address.
    fn strip(&mut self, b: u8) {
        self.escape = match (self.escape, b) {
            (Escape::None, ESC) => Escape::Esc,
            (Escape::None, b) => {
                self.text.push(if b < 0x20 || b == 0x7f { b' ' } else { b });
                Escape::None
            }
            (Escape::Esc, b'[') => Escape::Csi,
            (Escape::Esc, b']' | b'P' | b'X' | b'^' | b'_') => Escape::Str,
            (Escape::Esc, _) => Escape::None,
            (Escape::Csi, 0x40..=0x7e) => Escape::None,
            (Escape::Csi, _) => Escape::Csi,
            (Escape::Str, super::BEL) => Escape::None,
            (Escape::Str | Escape::StrEsc, ESC) => Escape::StrEsc,
            (Escape::StrEsc, b'\\') => Escape::None,
            (Escape::Str | Escape::StrEsc, _) => Escape::Str,
        };
    }

    /// Every complete address in `text`; one whose port runs to the end
    /// of the text may have more digits coming, and waits for them.
    fn scan(&mut self) -> bool {
        let mut grew = false;
        let mut from = 0;
        while let Some(at) = find(&self.text[from..], SCHEME) {
            from += at + SCHEME.len();
            let rest = &self.text[from..];
            let Some(host) = HOSTS.iter().find(|h| rest.starts_with(h)) else {
                continue;
            };
            let Some(digits) = rest[host.len()..].strip_prefix(b":") else {
                continue;
            };
            let n = digits.iter().take_while(|b| b.is_ascii_digit()).count();
            if n == 0 || n == digits.len() {
                continue;
            }
            let port = std::str::from_utf8(&digits[..n])
                .ok()
                .and_then(|d| d.parse::<u16>().ok())
                .filter(|&p| p != 0);
            if let Some(port) = port {
                if !self.ports.contains(&port) && self.ports.len() < MAX_PORTS {
                    self.ports.push(port);
                    grew = true;
                }
            }
        }
        grew
    }
}

/// Where `needle` first starts in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(chunks: &[&[u8]]) -> Vec<u16> {
        let mut scanner = PortScanner::default();
        for chunk in chunks {
            scanner.feed(chunk);
        }
        scanner.ports().to_vec()
    }

    /// The three hosts that mean this machine, as Next.js, Vite and a
    /// server bound to every interface print them; a LAN address, another
    /// host or https is not the app on this machine's localhost.
    #[test]
    fn the_three_local_hosts_are_read_and_nothing_else() {
        let out = b"  - Local:        http://localhost:3000\r\n\
            \x20 - Network:      http://192.168.1.5:3000\r\n\
            listening on http://0.0.0.0:8080/\n\
            api at http://127.0.0.1:4000/graphql\n\
            docs http://example.com:9000 and https://localhost:9443\n";
        assert_eq!(scanned(&[out]), vec![3000, 8080, 4000]);
    }

    /// The set is sticky and ordered by first sighting: a port printed
    /// again is not new, and `feed` says when one is.
    #[test]
    fn ports_are_kept_in_the_order_first_seen() {
        let mut scanner = PortScanner::default();
        assert!(scanner.feed(b"http://localhost:5173/\n"));
        assert!(!scanner.feed(b"ready again at http://localhost:5173/\n"));
        assert!(!scanner.feed(b"compiled in 12ms\n"));
        assert!(scanner.feed(b"http://localhost:3000\n"));
        assert_eq!(scanner.ports(), &[5173, 3000]);
    }

    /// Vite bolds the port inside its URL and colours the rest; an
    /// address split across two reads is read whole, and a port is only
    /// taken once its digits have ended.
    #[test]
    fn escapes_and_chunk_boundaries_do_not_hide_a_port() {
        let vite = b"\x1b[32m\x1b[1m  \xe2\x9e\x9c\x1b[22m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\n";
        assert_eq!(scanned(&[vite]), vec![5173]);
        assert_eq!(
            scanned(&[b"ready - http://local", b"host:30", b"00\n"]),
            vec![3000]
        );
        let mut scanner = PortScanner::default();
        assert!(
            !scanner.feed(b"http://localhost:30"),
            "more digits may come"
        );
        assert!(scanner.feed(b"01 started\n"));
        assert_eq!(scanner.ports(), &[3001]);
        // A hyperlink's target is skipped; its visible text is what counts.
        let link = b"\x1b]8;;http://localhost:1111\x1b\\http://localhost:2222\x1b]8;;\x07\n";
        assert_eq!(scanned(&[link]), vec![2222]);
    }

    /// Nonsense after the colon is no port: none at all, zero, or one
    /// past the top of the range.
    #[test]
    fn only_a_real_port_is_kept() {
        let out = b"http://localhost:/ http://localhost:0/ http://localhost:70000/ http://localhost:65535/\n";
        assert_eq!(scanned(&[out]), vec![65535]);
    }

    /// A run that prints a great many is capped, and the text carried
    /// between reads stays small however much it prints.
    #[test]
    fn a_noisy_run_is_bounded() {
        let mut scanner = PortScanner::default();
        for port in 3000..3100 {
            scanner.feed(format!("http://localhost:{port}\n").as_bytes());
        }
        assert_eq!(scanner.ports().len(), MAX_PORTS);
        scanner.feed(&[b'x'; 100_000]);
        assert!(scanner.text.len() <= CARRY);
    }
}
