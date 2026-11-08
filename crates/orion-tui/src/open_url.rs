//! Handing things to macOS's open(1): the one place orion runs it for a
//! link or an app.
//!
//! A link opens in the browser its account's **Open links in** row picks
//! (`harnesses.<id>.browser`): a link from a session's pane, or from a row
//! that belongs to a session, passes that session's account; every other
//! link passes none and opens in the system's default browser.

use orion_core::harness::LinkBrowser;

/// One browser **Open links in** steps through, after `default`.
struct Browser {
    /// What the row shows and the config stores (`harnesses.<id>.browser.app`).
    name: &'static str,
    /// The app `open -a` names.
    app: &'static str,
    /// The arguments that open a profile, by its name — None for a browser
    /// with no command-line profile.
    profile: Option<fn(&str) -> Vec<String>>,
}

const BROWSERS: &[Browser] = &[
    Browser {
        name: "Safari",
        app: "Safari",
        profile: None,
    },
    Browser {
        name: "Chrome",
        app: "Google Chrome",
        profile: Some(|p| vec![format!("--profile-directory={p}")]),
    },
    Browser {
        name: "Firefox",
        app: "Firefox",
        profile: Some(|p| vec!["-P".into(), p.into()]),
    },
    Browser {
        name: "Arc",
        app: "Arc",
        profile: None,
    },
];

/// What **Open links in** says, and stores, for the system's own browser.
pub const DEFAULT: &str = "default";

/// The values **Open links in**'s ←/→ steps through: `default`, then each
/// browser by name.
pub fn choices() -> Vec<String> {
    std::iter::once(DEFAULT)
        .chain(BROWSERS.iter().map(|b| b.name))
        .map(String::from)
        .collect()
}

fn known(app: &str) -> Option<&'static Browser> {
    BROWSERS
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(app.trim()))
}

/// The row's value for a stored choice: `default`, `Chrome`, or
/// `Chrome · Profile 1`.
pub fn label(browser: Option<&LinkBrowser>) -> String {
    match browser {
        None => DEFAULT.into(),
        Some(LinkBrowser { app, profile: None }) => app.clone(),
        Some(LinkBrowser {
            app,
            profile: Some(p),
        }) => format!("{app} · {p}"),
    }
}

/// What the row's prompt holds for a stored choice: the browser, then the
/// profile after a space, as it is typed.
pub fn typed(browser: Option<&LinkBrowser>) -> String {
    match browser {
        None => String::new(),
        Some(LinkBrowser { app, profile }) => match profile {
            Some(p) => format!("{app} {p}"),
            None => app.clone(),
        },
    }
}

/// Read the row's prompt: empty or `default` is the system's browser; a
/// known browser's name may be followed by a profile (`Chrome Profile 1`,
/// `firefox work`); anything else is an app's name, whole, for `open -a`.
pub fn parse(text: &str) -> Option<LinkBrowser> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case(DEFAULT) {
        return None;
    }
    let (head, rest) = text.split_once(' ').unwrap_or((text, ""));
    let Some(browser) = known(head) else {
        return Some(LinkBrowser {
            app: text.into(),
            profile: None,
        });
    };
    let rest = rest.trim();
    Some(LinkBrowser {
        app: browser.name.into(),
        profile: (!rest.is_empty()).then(|| rest.to_string()),
    })
}

/// The arguments to open(1) that open `url` in `browser`: plain `open
/// <url>` for the default; `open -a <App> <url>` for a browser; and, for a
/// profile, `open -na <App> --args <flags> <url>` — `--args` reaches only
/// an app being launched, so `-n` launches one, which hands the URL to the
/// running browser under that profile. A profile on a browser without one
/// (Safari, Arc, an app typed by name) is ignored.
pub fn open_args(url: &str, browser: Option<&LinkBrowser>) -> Vec<String> {
    let Some(choice) = browser else {
        return vec![url.into()];
    };
    let found = known(&choice.app);
    let app = found.map_or(choice.app.as_str(), |b| b.app).to_string();
    let flags = found
        .and_then(|b| b.profile)
        .zip(choice.profile.as_deref().filter(|p| !p.trim().is_empty()))
        .map(|(flags, p)| flags(p.trim()));
    match flags {
        Some(flags) => {
            let mut args = vec!["-na".into(), app, "--args".into()];
            args.extend(flags);
            args.push(url.into());
            args
        }
        None => vec!["-a".into(), app, url.into()],
    }
}

/// Open `url` in the browser account `account`'s **Open links in** picks
/// — the default browser with no account, or none picked. The scheme
/// allowlist is defense in depth: the link scanner only ever produces
/// http(s) URLs, but the text comes from untrusted PTY output.
pub(crate) fn open_url(url: &str, account: Option<&str>) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    if cfg!(test) {
        #[cfg(test)]
        OPENED.with(|opened| {
            opened
                .borrow_mut()
                .push((url.to_string(), account.map(String::from)))
        });
        return true;
    }
    let cfg = account.map(|_| crate::config::Config::load());
    let browser = cfg
        .as_ref()
        .zip(account)
        .and_then(|(cfg, id)| cfg.link_browser(id));
    let mut open = std::process::Command::new("open");
    open.args(open_args(url, browser));
    spawn_and_reap(open, "open url")
}

/// The account a session's links open for: the registry id its harness
/// goes by (`claude`, `claude-work`, `codex`).
pub fn account_of(agent: &orion_core::Agent) -> String {
    orion_core::harness::registry_id(agent.kind, agent.custom_harness.as_deref()).to_string()
}

/// `open -a <bundle> <path>`: whether LaunchServices took the hand-off.
pub(crate) fn open_in_app(bundle: &std::path::Path, path: &std::path::Path) -> bool {
    if cfg!(test) {
        return true;
    }
    let mut open = std::process::Command::new("open");
    open.arg("-a").arg(bundle).arg(path);
    spawn_and_reap(open, "open in app")
}

#[cfg(test)]
thread_local! {
    static OPENED: std::cell::RefCell<Vec<(String, Option<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The URLs [`open_url`] handed the browser on this test's thread, taken.
#[cfg(test)]
pub(crate) fn take_opened() -> Vec<String> {
    take_opened_as().into_iter().map(|(url, _)| url).collect()
}

/// [`take_opened`], each URL with the account it was opened for.
#[cfg(test)]
pub(crate) fn take_opened_as() -> Vec<(String, Option<String>)> {
    OPENED.with(|opened| std::mem::take(&mut *opened.borrow_mut()))
}

/// Start `command` and let it finish on its own: true once it is running.
/// `open` spends 50 to 150 ms talking to LaunchServices before it exits,
/// and the key that asked for a browser tab used to spend them with it —
/// the loop frozen, the flash unpainted. Whether it then succeeds is not
/// something the keypress can wait to learn; a failure is logged (the
/// `spawn_open_command` rule), and the reaper thread is what keeps the
/// child from lingering as a zombie.
fn spawn_and_reap(mut command: std::process::Command, what: &'static str) -> bool {
    use std::process::Stdio;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => tracing::warn!(what, %status, "open failed"),
        Err(err) => tracing::warn!(what, %err, "open not reaped"),
        Ok(_) => {}
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(app: &str, profile: Option<&str>) -> LinkBrowser {
        LinkBrowser {
            app: app.into(),
            profile: profile.map(String::from),
        }
    }

    const URL: &str = "https://example.com/a?b=c";

    /// The command line each browser, with and without a profile, opens a
    /// link with.
    #[test]
    fn each_browser_and_profile_has_its_command_line() {
        let cases: &[(Option<LinkBrowser>, &[&str])] = &[
            (None, &[URL]),
            (Some(pick("Safari", None)), &["-a", "Safari", URL]),
            (Some(pick("Chrome", None)), &["-a", "Google Chrome", URL]),
            (Some(pick("Firefox", None)), &["-a", "Firefox", URL]),
            (Some(pick("Arc", None)), &["-a", "Arc", URL]),
            (
                Some(pick("Chrome", Some("Profile 1"))),
                &[
                    "-na",
                    "Google Chrome",
                    "--args",
                    "--profile-directory=Profile 1",
                    URL,
                ],
            ),
            (
                Some(pick("Firefox", Some("work"))),
                &["-na", "Firefox", "--args", "-P", "work", URL],
            ),
            // No command-line profile: the profile is ignored.
            (Some(pick("Safari", Some("x"))), &["-a", "Safari", URL]),
            (Some(pick("Arc", Some("x"))), &["-a", "Arc", URL]),
            // A stored name in another case still finds its app.
            (Some(pick("chrome", None)), &["-a", "Google Chrome", URL]),
            // An app typed by name goes to `open -a` as it is.
            (
                Some(pick("Brave Browser", Some("x"))),
                &["-a", "Brave Browser", URL],
            ),
            (
                Some(pick("Chrome", Some("  "))),
                &["-a", "Google Chrome", URL],
            ),
        ];
        for (browser, want) in cases {
            assert_eq!(open_args(URL, browser.as_ref()), *want, "{browser:?}");
        }
    }

    #[test]
    fn the_prompt_reads_a_browser_then_a_profile() {
        assert_eq!(parse(""), None);
        assert_eq!(parse(" Default "), None);
        assert_eq!(parse("safari"), Some(pick("Safari", None)));
        assert_eq!(
            parse("Chrome Profile 1"),
            Some(pick("Chrome", Some("Profile 1")))
        );
        assert_eq!(parse("firefox  work "), Some(pick("Firefox", Some("work"))));
        assert_eq!(parse("Brave Browser"), Some(pick("Brave Browser", None)));
        // What the row shows, typed back, is what it stored.
        for stored in [pick("Chrome", Some("Profile 1")), pick("Arc", None)] {
            assert_eq!(parse(&typed(Some(&stored))), Some(stored));
        }
        assert_eq!(
            label(Some(&pick("Chrome", Some("Profile 1")))),
            "Chrome · Profile 1"
        );
        assert_eq!(label(None), "default");
        assert_eq!(choices(), ["default", "Safari", "Chrome", "Firefox", "Arc"]);
    }

    #[test]
    fn only_http_links_open() {
        assert!(!open_url("file:///etc/passwd", None));
        assert!(!open_url("javascript:alert(1)", Some("claude")));
        assert!(take_opened().is_empty());
        assert!(open_url(URL, Some("claude-2")));
        assert_eq!(
            take_opened_as(),
            [(URL.to_string(), Some("claude-2".to_string()))]
        );
    }

    /// Every open(1) orion runs goes through here: a `Command::new("open")`
    /// anywhere else would hand a link to the default browser behind the
    /// account's back. `app_bundle.rs` launches orion's own app, never a
    /// link, and waits on it with a cleared environment.
    #[test]
    fn nothing_else_runs_open() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = ["open_url.rs", "app_bundle.rs"];
        let needle = concat!("Command::new(", "\"open\")");
        let mut found = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !allowed.iter().any(|a| path.ends_with(a))
                    && std::fs::read_to_string(&path).unwrap().contains(needle)
                {
                    found.push(path.display().to_string());
                }
            }
        }
        assert!(found.is_empty(), "open(1) outside open_url.rs: {found:?}");
    }
}
