//! A FLASH: the one line the FOOTER speaks in place of its key hints, and
//! what kind of line it is. The footer only speaks when something failed,
//! a setting has to change, something happened that nothing on screen
//! shows, or a wait would otherwise be silent — and the kind picks the
//! mark and color it is drawn in (`ui::footer`), from the fixed status
//! roles, so a theme preset never moves them:
//!
//! | kind | mark | color |
//! |---|---|---|
//! | [`FlashKind::Failed`] | `✕` | the needs-you crimson |
//! | [`FlashKind::Setup`] | `⚠` | the working gold |
//! | [`FlashKind::Done`] | `✓` | the done green |
//! | [`FlashKind::Working`] | the WORKING SPINNER | gold mark, muted text |
//! | [`FlashKind::Note`] | `·` | muted |

use std::fmt;
use std::ops::Deref;

/// What a [`Flash`] says, which sets how the footer draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashKind {
    /// Something you asked for didn't happen, and the line says why.
    Failed,
    /// It can't run until something is set up, and the line says where.
    Setup,
    /// It worked, and nothing on screen shows it.
    Done,
    /// A wait that would otherwise be silent.
    Working,
    /// Worth a line, but neither a problem nor a result.
    Note,
}

/// One footer line and its kind. It reads as its text (`Deref<Target =
/// str>`), so `app.flash.as_deref()` is the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub kind: FlashKind,
    pub text: String,
}

impl Flash {
    fn new(kind: FlashKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    pub fn failed(text: impl Into<String>) -> Self {
        Self::new(FlashKind::Failed, text)
    }

    pub fn setup(text: impl Into<String>) -> Self {
        Self::new(FlashKind::Setup, text)
    }

    pub fn done(text: impl Into<String>) -> Self {
        Self::new(FlashKind::Done, text)
    }

    pub fn working(text: impl Into<String>) -> Self {
        Self::new(FlashKind::Working, text)
    }

    pub fn note(text: impl Into<String>) -> Self {
        Self::new(FlashKind::Note, text)
    }
}

impl Deref for Flash {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Flash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
