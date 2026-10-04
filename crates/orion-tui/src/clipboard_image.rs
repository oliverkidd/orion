//! CLIPBOARD IMAGES: `^V` in a box bound for an agent on this machine —
//! the QUICK PROMPT, an AGENT PRESET's task box, the FOLLOW-UP box —
//! pastes the image on the system clipboard as a file the agent can open.
//!
//! A terminal's own paste (⌘V) only ever carries text, so a screenshot
//! taken to the clipboard (⌃⇧⌘4) never reached the box at all; dragging
//! it in was the one way (`dropped_files`). `^V` is the other: the
//! clipboard's image is written as a PNG into the DATA DIR's
//! `attachments/` — the folder drops are copied to, pruned the same way —
//! under a name taken from its bytes (`clipboard-<hash>.png`), so pasting
//! one screenshot twice leaves one file, and the box gets that path at the
//! caret, as a staged drop does.
//!
//! Reading the clipboard is a process to start: `pngpaste -` when it is on
//! PATH (it converts whatever image type is there), otherwise `osascript`
//! asking for `«class PNGf»`, and failing that `«class TIFF»` — what some
//! apps copy instead — turned into a PNG by `sips`. `osascript` takes a
//! good part of a second to start, so the event loop runs this on the
//! blocking pool and lands the answer (`view_jobs::Answer::ClipboardImage`).
//! A clipboard holding text, or nothing, is [`Pasted::NoImage`], which the
//! footer says rather than pasting anything.
//!
//! The unit tests never reach the real clipboard: under `cfg(test)` the
//! reader returns whatever [`with_clipboard`] set for the thread, and no
//! image without it.

use std::io;
use std::path::{Path, PathBuf};

/// What a `^V` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pasted {
    /// The clipboard's image, kept here.
    Saved(PathBuf),
    /// Text on the clipboard, or nothing: no image to paste.
    NoImage,
    /// There was no reading it, or no writing it down — why, in a phrase.
    Failed(String),
}

/// The first bytes of every PNG.
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The clipboard's image, kept in `dir` (the DATA DIR's `attachments/`).
/// Blocking: a process or two is started to read the clipboard.
pub fn paste_into(dir: &Path) -> Pasted {
    match read_png(dir) {
        Ok(Some(png)) => match keep(&png, dir) {
            Ok(path) => Pasted::Saved(path),
            Err(err) => Pasted::Failed(format!("couldn't save it: {err}")),
        },
        Ok(None) => Pasted::NoImage,
        Err(err) => Pasted::Failed(err),
    }
}

/// `png` written into `dir` as `clipboard-<hash>.png`, copies older than
/// a week pruned first (`dropped_files::prune`). The same image pasted
/// again finds its file already there and only freshens its age.
pub(crate) fn keep(png: &[u8], dir: &Path) -> io::Result<PathBuf> {
    use std::hash::{DefaultHasher, Hasher};
    crate::dropped_files::prune(dir);
    std::fs::create_dir_all(dir)?;
    let mut hasher = DefaultHasher::new();
    hasher.write(png);
    let dest = dir.join(format!("clipboard-{:016x}.png", hasher.finish()));
    if std::fs::read(&dest).is_ok_and(|there| there == png) {
        return crate::dropped_files::touched(dest);
    }
    orion_core::settings::write_atomic(&dest, png)?;
    Ok(dest)
}

/// What goes into the box for the image at `path`: the path, quoted the
/// way a staged drop's is (the DATA DIR is under `Application Support`),
/// a space before it when the caret follows a word (`before` is the char
/// there), and one after, so the next word typed stands apart.
pub fn insertion(path: &Path, before: Option<char>) -> String {
    let quoted = crate::dropped_files::quoted(&path.to_string_lossy());
    let lead = if before.is_some_and(|c| !c.is_whitespace()) {
        " "
    } else {
        ""
    };
    format!("{lead}{quoted} ")
}

#[cfg(test)]
thread_local! {
    static CLIPBOARD: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// Test hook: the clipboard holds `png` (None: no image) for this thread
/// while `f` runs.
#[cfg(test)]
pub fn with_clipboard<T>(png: Option<Vec<u8>>, f: impl FnOnce() -> T) -> T {
    CLIPBOARD.with(|slot| {
        let prev = slot.replace(png);
        let out = f();
        slot.replace(prev);
        out
    })
}

/// The clipboard's image as PNG bytes: None when it holds none. `scratch`
/// is a folder a conversion may leave its working files in for a moment.
#[cfg(test)]
fn read_png(_scratch: &Path) -> Result<Option<Vec<u8>>, String> {
    Ok(CLIPBOARD.with(|slot| slot.borrow().clone()))
}

#[cfg(not(test))]
fn read_png(scratch: &Path) -> Result<Option<Vec<u8>>, String> {
    #[cfg(target_os = "macos")]
    {
        mac::read_png(scratch)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = scratch;
        linux::read_png()
    }
}

/// `bytes` when they are a PNG, else the reason they are not used.
#[cfg_attr(test, allow(dead_code))]
fn png_or_err(bytes: Vec<u8>, from: &str) -> Result<Option<Vec<u8>>, String> {
    if bytes.starts_with(PNG_MAGIC) {
        Ok(Some(bytes))
    } else {
        Err(format!("{from} gave something that is not a PNG"))
    }
}

/// `«data PNGf89504E47…»`, what `osascript` prints for clipboard data, as
/// the bytes it spells. None for anything else. The guillemets are not
/// relied on — under a locale that is not UTF-8 they come out as other
/// bytes — only the `data` word, the four-letter class and the hex.
#[cfg_attr(test, allow(dead_code))]
fn applescript_data(out: &str) -> Option<Vec<u8>> {
    let after = &out[out.find("data ")? + "data ".len()..];
    let hex = after
        .get(4..)?
        .trim_end_matches(|c: char| !c.is_ascii_hexdigit());
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(all(target_os = "macos", not(test)))]
mod mac {
    use super::{applescript_data, png_or_err};
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub(super) fn read_png(scratch: &Path) -> Result<Option<Vec<u8>>, String> {
        // `pngpaste` reads every image type the pasteboard offers and
        // hands back a PNG; with it installed, its answer is the answer.
        let path = crate::config::search_path();
        if let Some(pngpaste) = crate::install::which(&path, "pngpaste") {
            if let Ok(out) = quiet(Command::new(pngpaste).arg("-")).output() {
                if out.status.success() && !out.stdout.is_empty() {
                    return png_or_err(out.stdout, "pngpaste");
                }
            }
        }
        if let Some(png) = clipboard_as("PNGf")? {
            return png_or_err(png, "the clipboard");
        }
        let Some(tiff) = clipboard_as("TIFF")? else {
            return Ok(None);
        };
        tiff_to_png(&tiff, scratch).map(Some)
    }

    fn quiet(command: &mut Command) -> &mut Command {
        command.stdin(Stdio::null()).stderr(Stdio::piped())
    }

    /// The clipboard as AppleScript class `class` — None when it holds
    /// nothing that converts to it (AppleScript's -1700, "Can't make some
    /// data into the expected type").
    fn clipboard_as(class: &str) -> Result<Option<Vec<u8>>, String> {
        let script = format!("the clipboard as «class {class}»");
        let out = quiet(Command::new("osascript").args(["-e", &script]))
            .output()
            .map_err(|err| format!("osascript: {err}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("-1700") || err.contains("-25131") {
                return Ok(None);
            }
            return Err(format!("osascript: {}", err.trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        applescript_data(&text)
            .map(Some)
            .ok_or_else(|| "osascript printed no image data".to_string())
    }

    /// A TIFF made a PNG by `sips`, which ships with macOS, through two
    /// files in `scratch` removed again either way.
    fn tiff_to_png(tiff: &[u8], scratch: &Path) -> Result<Vec<u8>, String> {
        std::fs::create_dir_all(scratch).map_err(|err| err.to_string())?;
        let stem = format!(".clipboard-{}", std::process::id());
        let from = scratch.join(format!("{stem}.tiff"));
        let to = scratch.join(format!("{stem}.png"));
        let converted = std::fs::write(&from, tiff)
            .map_err(|err| err.to_string())
            .and_then(|()| {
                let status = quiet(Command::new("sips").args(["-s", "format", "png"]))
                    .arg(&from)
                    .arg("--out")
                    .arg(&to)
                    .stdout(Stdio::null())
                    .status()
                    .map_err(|err| format!("sips: {err}"))?;
                if !status.success() {
                    return Err("sips could not convert the clipboard's TIFF".into());
                }
                std::fs::read(&to).map_err(|err| err.to_string())
            });
        let _ = std::fs::remove_file(&from);
        let _ = std::fs::remove_file(&to);
        png_or_err(converted?, "sips").map(|png| png.unwrap_or_default())
    }
}

#[cfg(all(not(target_os = "macos"), not(test)))]
mod linux {
    use super::png_or_err;
    use std::process::{Command, Stdio};

    /// `wl-paste` on Wayland, `xclip` on X11, each asked for `image/png`.
    pub(super) fn read_png() -> Result<Option<Vec<u8>>, String> {
        let (program, args): (&str, &[&str]) = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            ("wl-paste", &["--no-newline", "--type", "image/png"])
        } else {
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            )
        };
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|err| format!("{program}: {err}"))?;
        if !out.status.success() || out.stdout.is_empty() {
            return Ok(None);
        }
        png_or_err(out.stdout, program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(body: &[u8]) -> Vec<u8> {
        [PNG_MAGIC, body].concat()
    }

    #[test]
    fn an_image_on_the_clipboard_is_kept_once_under_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let attachments = dir.path().join("attachments");
        let shot = png(b"pixels");
        let Pasted::Saved(path) = with_clipboard(Some(shot.clone()), || paste_into(&attachments))
        else {
            panic!("expected the image saved");
        };
        assert_eq!(std::fs::read(&path).unwrap(), shot);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("clipboard-") && name.ends_with(".png"),
            "{name}"
        );
        // The same image again: the same file, not a second one.
        let again = with_clipboard(Some(shot), || paste_into(&attachments));
        assert_eq!(again, Pasted::Saved(path));
        let other = with_clipboard(Some(png(b"other")), || paste_into(&attachments));
        assert!(matches!(other, Pasted::Saved(p) if p.file_name().unwrap() != name.as_str()));
        assert_eq!(std::fs::read_dir(&attachments).unwrap().count(), 2);
    }

    #[test]
    fn a_clipboard_without_an_image_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let attachments = dir.path().join("attachments");
        assert_eq!(paste_into(&attachments), Pasted::NoImage);
        assert_eq!(
            with_clipboard(None, || paste_into(&attachments)),
            Pasted::NoImage
        );
        assert!(!attachments.exists());
    }

    #[test]
    fn the_inserted_path_is_quoted_and_spaced_off_the_words_around_it() {
        let plain = Path::new("/data/attachments/clipboard-1.png");
        assert_eq!(insertion(plain, None), "/data/attachments/clipboard-1.png ");
        assert_eq!(
            insertion(plain, Some(' ')),
            "/data/attachments/clipboard-1.png "
        );
        assert_eq!(
            insertion(plain, Some('e')),
            " /data/attachments/clipboard-1.png "
        );
        let spaced = Path::new("/Library/Application Support/orion/attachments/clipboard-1.png");
        assert_eq!(
            insertion(spaced, Some('\n')),
            "'/Library/Application Support/orion/attachments/clipboard-1.png' "
        );
    }

    #[test]
    fn osascript_data_is_read_back_into_bytes() {
        assert_eq!(
            applescript_data("«data PNGf89504E470D0A1A0A»\n"),
            Some(PNG_MAGIC.to_vec())
        );
        assert_eq!(applescript_data("«data TIFF4D4D»"), Some(vec![0x4d, 0x4d]));
        assert_eq!(
            applescript_data("\u{fffd}data TIFF4D4D\u{fffd}\n"),
            Some(vec![0x4d, 0x4d]),
            "guillemets mangled by a non-UTF-8 locale"
        );
        assert_eq!(applescript_data("hello"), None);
        assert_eq!(applescript_data("«data PNGf123»"), None);
        assert!(png_or_err(b"GIF89a".to_vec(), "x").is_err());
    }
}
