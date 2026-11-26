//! WHERE A SESSION WRITES: the checkouts a session changes besides its
//! own. A session in one orion project can work in another's checkout
//! without moving there — `cd /…/other-repo/feature && git commit …` — and
//! orion only knows a session by the checkout it was started in. The
//! DAEMON reads every Bash line and every file edit off the session's hooks
//! and keeps, per session, the other checkouts it wrote to lately
//! ([`Visit`], `ServerEvent::Visits`), so the PR FIXER knows that branch
//! already has an agent at work on it.
//!
//! Only writes count — a git commit, merge, push … or a file edit. A
//! session that merely reads another branch (`git log`, `cat`) is not at
//! work on it.

use std::path::{Path, PathBuf};

/// How long a write keeps a session counted as at work in a checkout that
/// isn't its own: two hours since its last one there.
pub const VISIT_TTL_MS: i64 = 2 * 60 * 60 * 1000;

/// The git subcommands that change a checkout or its branch.
const GIT_WRITES: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "commit",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "stash",
    "switch",
];

/// The git options that take the next word as their value.
const GIT_VALUED: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// One shell word, quotes stripped. Splits on whitespace and on the
/// separators `;`, `&&`, `||`, `|`, `(`, `)` — each its own word — so
/// `cd /x&&git commit` reads as four words.
fn words(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => quote = Some(c),
                c if c.is_whitespace() => flush(&mut cur, &mut out),
                ';' | '(' | ')' => {
                    flush(&mut cur, &mut out);
                    out.push(c.to_string());
                }
                '&' | '|' => {
                    flush(&mut cur, &mut out);
                    let mut sep = c.to_string();
                    if chars.peek() == Some(&c) {
                        sep.push(c);
                        chars.next();
                    }
                    out.push(sep);
                }
                c => cur.push(c),
            },
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn is_separator(word: &str) -> bool {
    matches!(word, ";" | "&&" | "||" | "|" | "&" | "(" | ")")
}

/// `word` as an absolute path: `/…` as is, `~/…` under `home`. None for
/// anything else — a relative path says nothing without the shell's cwd.
fn absolute(word: &str, home: &Path) -> Option<PathBuf> {
    let word = word.trim_end_matches([',', ':']);
    if let Some(rest) = word.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    word.starts_with('/').then(|| PathBuf::from(word))
}

/// The absolute paths a shell line names: every `/…` or `~/…` word, a
/// `NAME=/…` assignment's value, and an `--opt=/…` option's. Where a
/// checkout is is decided by whoever matches them against checkouts.
pub fn from_command(command: &str, home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for word in words(command) {
        let value = match word.split_once('=') {
            Some((key, value)) if !key.is_empty() && !key.contains('/') => value,
            _ => word.as_str(),
        };
        if let Some(path) = absolute(value, home) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// Whether the line runs a git command that changes a checkout or its
/// branch ([`GIT_WRITES`]), past git's own options (`-C <dir>`, `-c k=v`,
/// `--no-pager` …).
pub fn writes_git(command: &str) -> bool {
    let words = words(command);
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let is_git = w == "git" || w.ends_with("/git");
        // `command git …`, `env X=1 git …` and the like still start a git.
        let starts = i == 0
            || is_separator(&words[i - 1])
            || matches!(words[i - 1].as_str(), "command" | "exec" | "env" | "sudo")
            || words[i - 1].contains('=');
        if is_git && starts {
            let mut j = i + 1;
            while j < words.len() {
                let o = words[j].as_str();
                if GIT_VALUED.contains(&o) {
                    j += 2;
                } else if o.starts_with('-') {
                    j += 1;
                } else {
                    break;
                }
            }
            if words
                .get(j)
                .is_some_and(|s| GIT_WRITES.contains(&s.as_str()))
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/me")
    }

    #[test]
    fn a_cd_into_another_checkout_names_it() {
        let line = "cd /Users/me/Projects/.orion-worktrees/riplo-os/module-platform && git fetch -q origin dev && git status --porcelain | wc -l";
        assert_eq!(
            from_command(line, &home()),
            vec![PathBuf::from(
                "/Users/me/Projects/.orion-worktrees/riplo-os/module-platform"
            )]
        );
        assert!(!writes_git(line), "fetch and status change nothing");
    }

    #[test]
    fn quoted_tilde_and_option_paths_are_read() {
        let line =
            r#"git -C "~/code/app" commit -q -m "fix: x" && FOO=/tmp/out cp a --dest=/srv/b"#;
        assert_eq!(
            from_command(line, &home()),
            vec![
                PathBuf::from("/Users/me/code/app"),
                PathBuf::from("/tmp/out"),
                PathBuf::from("/srv/b"),
            ]
        );
        assert!(writes_git(line));
    }

    #[test]
    fn relative_paths_and_plain_words_name_nothing() {
        assert!(from_command("cd src && ls -la", &home()).is_empty());
        assert!(from_command("echo hello", &home()).is_empty());
    }

    #[test]
    fn git_writes_are_found_past_options_and_separators() {
        assert!(writes_git("cd /x&&git commit -m hi"));
        assert!(writes_git(
            "git -c core.editor=true merge origin/dev --no-edit"
        ));
        assert!(writes_git("command git push origin HEAD"));
        assert!(writes_git("(git add -A; git stash)"));
        assert!(!writes_git("git log --oneline -3; git diff"));
        assert!(!writes_git("echo git commit"));
        assert!(!writes_git("gh pr merge 12"));
    }
}
