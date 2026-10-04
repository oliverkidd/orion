//! CLAUDE ACCOUNTS: one machine signed in to Claude Code more than once.
//!
//! Claude Code keeps everything an account is — its login, its settings
//! and every transcript — in one config dir: `CLAUDE_CONFIG_DIR`,
//! `~/.claude` when that is unset. A second subscription is the same
//! `claude` pointed at another dir, so an account here is that dir under
//! a stable id (`claude_accounts` in config.json), and
//! [`ClaudeAccount::descriptor`] makes it a registry harness: built-in
//! Claude's row — program, flags, hooks, resume, everything — plus
//! `env.CLAUDE_CONFIG_DIR`. A change to Claude's row reaches every
//! account, and nothing is copied by hand.
//!
//! Who a dir is signed in as is Claude Code's own record:
//! `oauthAccount.emailAddress` in `.claude.json` — inside the config dir
//! when `CLAUDE_CONFIG_DIR` names one, in the home dir when it is unset
//! ([`Record::of`]). [`read_email`] takes that one field and nothing else:
//! the rest of the file is the account's business, not orion's.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::harness::HarnessDescriptor;

/// One extra Claude account, as `claude_accounts` in config.json holds it:
/// `{"id": "claude-2", "config_dir": "~/.claude-2"}`, plus `"name":
/// "Work"` once it has a display name. Built-in Claude is the default
/// account and has no entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeAccount {
    /// Stable id: the registry id the account's sessions point back at
    /// (`custom_harness`), so it outlives a rename of anything else.
    /// Lowercase letters, digits and hyphens, never a built-in's id.
    pub id: String,
    /// The account's Claude Code config dir. A leading `~/` is expanded
    /// where it is used, so the entry travels to another machine as
    /// written.
    pub config_dir: String,
    /// The name it goes by, as the user typed it — `Work`, `Work
    /// Laptop` — shown before the email it is signed in as (`Work
    /// (a@b.co)`). Empty: no name of its own, so it is `Claude (a@b.co)`.
    /// A rename changes this alone, never the id or the dir its sessions
    /// and its login hang off; written only while set, so an unnamed
    /// entry stays the two keys above.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Whether the pickers offer it — the Agents tab's switch. Written
    /// only while off, so an entry stays the two keys above.
    #[serde(default = "enabled_by_default", skip_serializing_if = "is_on")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

fn is_on(value: &bool) -> bool {
    *value
}

impl ClaudeAccount {
    /// The config dir, `~/` expanded.
    pub fn dir(&self) -> PathBuf {
        PathBuf::from(crate::harness::expand_home(self.config_dir.trim()))
    }

    /// Why this entry cannot be an account, or None when it can. The
    /// registry leaves a broken entry out; the Agents tab names the reason.
    pub fn problem(&self) -> Option<String> {
        let id = self.id.trim();
        if id.is_empty() {
            return Some("Claude account with an empty id".into());
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Some(format!(
                "Claude account id `{id}`: use lowercase letters, digits and hyphens"
            ));
        }
        if crate::harness::builtin(id).is_some() {
            return Some(format!(
                "Claude account id `{id}` collides with a built-in harness"
            ));
        }
        if self.config_dir.trim().is_empty() {
            return Some(format!("Claude account `{id}` has no config_dir"));
        }
        // A relative dir would be read from each session's own checkout.
        if !self.dir().is_absolute() {
            return Some(format!(
                "Claude account `{id}` config_dir `{}`: give an absolute or ~/ path",
                self.config_dir.trim()
            ));
        }
        None
    }

    /// The registry row this account launches as: `claude` — the
    /// effective row, the `harnesses` map's deltas for it included — under
    /// the account's id, with its own switch, its [`ClaudeAccount::name`]
    /// for a label — empty while it has none, never Claude's (the TUI
    /// adds the email it is signed in as) — and its config dir in `env`,
    /// on top of any env Claude's row carries.
    pub fn descriptor(&self, claude: &HarnessDescriptor) -> HarnessDescriptor {
        let mut descriptor = claude.clone();
        descriptor.id = self.id.trim().to_string();
        descriptor.label = self.name.trim().to_string();
        descriptor.enabled = self.enabled;
        descriptor.env.insert(
            crate::env::CLAUDE_CONFIG_DIR.to_string(),
            self.config_dir.trim().to_string(),
        );
        descriptor
    }
}

/// Where Claude Code records who one config dir is signed in as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// `.claude.json`: in the config dir `CLAUDE_CONFIG_DIR` names, in the
    /// home dir when it is unset — never inside `~/.claude` itself.
    pub file: PathBuf,
    /// `.config.json` in the config dir, which an older Claude Code wrote
    /// and a current one still reads first when it is there.
    pub legacy: PathBuf,
}

impl Record {
    /// The record of a CLI launched with `CLAUDE_CONFIG_DIR` set to
    /// `pinned` — or, for None, launched without it, which leaves this
    /// process's own `CLAUDE_CONFIG_DIR` (else the home dir) in charge.
    /// None with no home to resolve against.
    pub fn of(pinned: Option<&Path>) -> Option<Self> {
        let named = pinned
            .map(Path::to_path_buf)
            .or_else(|| crate::env::non_empty(crate::env::CLAUDE_CONFIG_DIR).map(PathBuf::from));
        let (config_dir, holder) = match named {
            Some(dir) => (dir.clone(), dir),
            None => {
                let home = crate::env::home_dir()?;
                (home.join(".claude"), home)
            }
        };
        Some(Self {
            file: holder.join(".claude.json"),
            legacy: config_dir.join(".config.json"),
        })
    }

    /// The file Claude Code reads now: the legacy one while it exists.
    pub fn current(&self) -> &Path {
        if self.legacy.is_file() {
            &self.legacy
        } else {
            &self.file
        }
    }
}

/// The email `file` (a [`Record`]'s) says its config dir is signed in as:
/// `oauthAccount.emailAddress`, trimmed. `Ok(None)` for a record with no
/// signed-in account — never signed in, or signed out since (`claude auth
/// logout` clears `oauthAccount`). Nothing else in the file is kept:
/// every other key is skipped as it is parsed.
pub fn read_email(file: &Path) -> std::io::Result<Option<String>> {
    #[derive(Deserialize)]
    struct Account {
        #[serde(rename = "emailAddress")]
        email_address: Option<String>,
    }
    #[derive(Deserialize)]
    struct Signed {
        #[serde(rename = "oauthAccount")]
        oauth_account: Option<Account>,
    }
    let signed: Signed = serde_json::from_slice(&std::fs::read(file)?)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    Ok(signed
        .oauth_account
        .and_then(|account| account.email_address)
        .map(|email| email.trim().to_string())
        .filter(|email| !email.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, dir: &str) -> ClaudeAccount {
        ClaudeAccount {
            id: id.into(),
            config_dir: dir.into(),
            name: String::new(),
            enabled: true,
        }
    }

    /// The entry stays the two keys it was written with while on, and an
    /// `enabled` an older build never wrote reads as on.
    #[test]
    fn an_entry_is_an_id_and_a_dir() {
        let read: ClaudeAccount =
            serde_json::from_str(r#"{"id": "claude-2", "config_dir": "~/.claude-2"}"#).unwrap();
        assert_eq!(read, account("claude-2", "~/.claude-2"));
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            serde_json::json!({"id": "claude-2", "config_dir": "~/.claude-2"})
        );
        let off = ClaudeAccount {
            enabled: false,
            ..read
        };
        assert_eq!(serde_json::to_value(&off).unwrap()["enabled"], false);
    }

    /// A display name is a third key, written only while there is one, and
    /// an entry an older build wrote — no `name` — reads as unnamed.
    #[test]
    fn a_name_is_written_only_while_set() {
        let named: ClaudeAccount = serde_json::from_str(
            r#"{"id": "claude-work", "config_dir": "~/.claude-work", "name": "Work Laptop"}"#,
        )
        .unwrap();
        assert_eq!(named.name, "Work Laptop");
        assert_eq!(
            serde_json::to_value(&named).unwrap(),
            serde_json::json!({
                "id": "claude-work", "config_dir": "~/.claude-work", "name": "Work Laptop"
            })
        );
        let unnamed = ClaudeAccount {
            name: String::new(),
            ..named
        };
        assert_eq!(
            serde_json::to_value(&unnamed).unwrap(),
            serde_json::json!({"id": "claude-work", "config_dir": "~/.claude-work"})
        );
    }

    #[test]
    fn ids_and_dirs_are_checked() {
        assert_eq!(account("claude-2", "~/.claude-2").problem(), None);
        assert!(account("", "~/.claude-2").problem().is_some());
        assert!(account("Claude 2", "~/.claude-2").problem().is_some());
        assert!(account("claude", "~/.claude").problem().is_some());
        assert!(account("codex", "~/.codex").problem().is_some());
        assert!(account("claude-2", "  ").problem().is_some());
        assert!(
            account("claude-2", ".claude-2").problem().is_some(),
            "relative"
        );
        assert_eq!(account("claude-2", "/srv/claude-2").problem(), None);
    }

    /// Everything but the id, the label, the switch and the dir is
    /// Claude's own row — the map's deltas for `claude` included — so a
    /// change to it reaches every account.
    #[test]
    fn an_account_is_claudes_row_in_another_dir() {
        let mut claude = crate::harness::builtin("claude").unwrap();
        claude.program = "/opt/claude".into();
        claude.env.insert("ANTHROPIC_LOG".into(), "debug".into());
        let off = ClaudeAccount {
            enabled: false,
            ..account("claude-2", "~/.claude-2")
        };
        let row = off.descriptor(&claude);
        assert_eq!(row.id, "claude-2");
        assert_eq!(row.label, "");
        assert!(!row.enabled);
        assert_eq!(row.program, "/opt/claude");
        assert_eq!(row.resume, claude.resume);
        assert_eq!(row.hooks.as_deref(), Some("claude"));
        assert_eq!(row.env["ANTHROPIC_LOG"], "debug");
        assert_eq!(row.env["CLAUDE_CONFIG_DIR"], "~/.claude-2");
        assert!(row.takes_claude_sessions());
        let home = crate::env::home_dir().unwrap();
        assert_eq!(row.pinned_claude_config_dir(), Some(home.join(".claude-2")));
        // A name is the row's label; the id, and so the sessions on it,
        // stay where they were.
        let named = ClaudeAccount {
            name: " Work ".into(),
            ..off
        };
        let row = named.descriptor(&claude);
        assert_eq!((row.id.as_str(), row.label.as_str()), ("claude-2", "Work"));
    }

    /// Claude Code's own rule: `.claude.json` sits inside a config dir
    /// `CLAUDE_CONFIG_DIR` names — `~/.claude` too, when named — and in the
    /// home dir only when the variable is unset.
    #[test]
    fn the_record_is_inside_a_named_dir() {
        let dir = tempfile::tempdir().unwrap();
        let record = Record::of(Some(dir.path())).unwrap();
        assert_eq!(record.file, dir.path().join(".claude.json"));
        assert_eq!(record.legacy, dir.path().join(".config.json"));
        assert_eq!(record.current(), dir.path().join(".claude.json"));
        std::fs::write(dir.path().join(".config.json"), "{}").unwrap();
        assert_eq!(record.current(), dir.path().join(".config.json"));
    }

    #[test]
    fn only_the_email_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude.json");
        std::fs::write(
            &file,
            r#"{"numStartups": 3, "projects": {"/x": {"allowedTools": []}},
                "oauthAccount": {"accountUuid": "u", "emailAddress": " a@b.co ",
                                 "organizationName": "Org"},
                "userID": "z"}"#,
        )
        .unwrap();
        assert_eq!(read_email(&file).unwrap().as_deref(), Some("a@b.co"));
        std::fs::write(&file, r#"{"numStartups": 3}"#).unwrap();
        assert_eq!(read_email(&file).unwrap(), None, "never signed in");
        std::fs::write(&file, r#"{"oauthAccount": null}"#).unwrap();
        assert_eq!(read_email(&file).unwrap(), None, "signed out");
        std::fs::write(&file, r#"{"oauthAccount": {"emailAd"#).unwrap();
        assert!(read_email(&file).is_err(), "half a file is no answer");
        assert!(read_email(&dir.path().join("missing.json")).is_err());
    }
}
