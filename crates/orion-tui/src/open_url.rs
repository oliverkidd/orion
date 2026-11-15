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
    /// The app `open -a` names, and its bundle in an Applications folder
    /// (`<app>.app`).
    app: &'static str,
    /// The arguments that open a profile, by its name — None for a browser
    /// with no command-line profile.
    profile: Option<fn(&str) -> Vec<String>>,
    /// Homebrew's cask for it — None for Safari, which comes with macOS —
    /// and its download page, for `i` on a browser that isn't installed.
    cask: Option<(&'static str, &'static str)>,
}

/// Chromium's profile flag, which every browser built on it takes.
fn chromium_profile(p: &str) -> Vec<String> {
    vec![format!("--profile-directory={p}")]
}

/// Firefox's profile flag, which Zen, built on it, takes too.
fn firefox_profile(p: &str) -> Vec<String> {
    vec!["-P".into(), p.into()]
}

const BROWSERS: &[Browser] = &[
    Browser {
        name: "Safari",
        app: "Safari",
        profile: None,
        cask: None,
    },
    Browser {
        name: "Chrome",
        app: "Google Chrome",
        profile: Some(chromium_profile),
        cask: Some(("google-chrome", "https://www.google.com/chrome/")),
    },
    Browser {
        name: "Firefox",
        app: "Firefox",
        profile: Some(firefox_profile),
        cask: Some(("firefox", "https://www.mozilla.org/firefox/")),
    },
    Browser {
        name: "Brave",
        app: "Brave Browser",
        profile: Some(chromium_profile),
        cask: Some(("brave-browser", "https://brave.com/download/")),
    },
    Browser {
        name: "Edge",
        app: "Microsoft Edge",
        profile: Some(chromium_profile),
        cask: Some(("microsoft-edge", "https://www.microsoft.com/edge/download")),
    },
    Browser {
        name: "Vivaldi",
        app: "Vivaldi",
        profile: Some(chromium_profile),
        cask: Some(("vivaldi", "https://vivaldi.com/download/")),
    },
    Browser {
        name: "Zen",
        app: "Zen",
        profile: Some(firefox_profile),
        cask: Some(("zen", "https://zen-browser.app/download/")),
    },
    Browser {
        name: "Opera",
        app: "Opera",
        profile: None,
        cask: Some(("opera", "https://www.opera.com/download")),
    },
    Browser {
        name: "Arc",
        app: "Arc",
        profile: None,
        cask: Some(("arc", "https://arc.net")),
    },
];

/// What **Open links in** says, and stores, for the system's own browser.
pub const DEFAULT: &str = "default";

/// The values **Open links in**'s ←/→ steps through: `default`, the
/// browsers this Mac has, then the ones it doesn't — each of those a row
/// `i` installs.
pub fn choices() -> Vec<String> {
    let (here, missing): (Vec<&Browser>, Vec<&Browser>) =
        BROWSERS.iter().partition(|b| is_installed(b));
    std::iter::once(DEFAULT)
        .chain(here.into_iter().chain(missing).map(|b| b.name))
        .map(String::from)
        .collect()
}

/// Whether `browser`'s app is in an Applications folder — the system's or
/// the user's. Safari comes with macOS. A test has only Safari, plus what
/// it names ([`with_installed`]).
fn is_installed(browser: &Browser) -> bool {
    if browser.cask.is_none() {
        return true;
    }
    #[cfg(test)]
    return INSTALLED.with(|names| names.borrow().contains(&browser.name));
    #[cfg(not(test))]
    {
        let bundle = format!("{}.app", browser.app);
        std::iter::once(std::path::PathBuf::from("/"))
            .chain(orion_core::env::home_dir())
            .any(|root| root.join("Applications").join(&bundle).is_dir())
    }
}

/// Whether the browser **Open links in** stores as `name` is on this Mac:
/// None for a name that isn't one of orion's browsers (an app typed by
/// name, or a CLI).
pub fn installed(name: &str) -> Option<bool> {
    known(name).map(is_installed)
}

/// How the browser stored as `name` gets onto this Mac, when it isn't
/// there: its Homebrew cask, and its download page. None for one that is
/// installed, or that orion doesn't know.
pub fn missing_cask(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let browser = known(name)?;
    let (cask, link) = browser.cask?;
    (!is_installed(browser)).then_some((browser.name, cask, link))
}

/// The browser stored, or typed, as `app`: by orion's name for it
/// (`Brave`) or its app's (`Brave Browser`), in any case.
fn known(app: &str) -> Option<&'static Browser> {
    let app = app.trim();
    BROWSERS
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(app) || b.app.eq_ignore_ascii_case(app))
}

/// The row's value for a stored choice: `default`, `Chrome`, or
/// `Chrome · Profile 1` — with `· not installed` after a browser this Mac
/// doesn't have.
pub fn label(browser: Option<&LinkBrowser>) -> String {
    let Some(LinkBrowser { app, profile }) = browser else {
        return DEFAULT.into();
    };
    let mut label = app.clone();
    if let Some(p) = profile {
        label.push_str(&format!(" · {p}"));
    }
    if installed(app) == Some(false) {
        label.push_str(" · not installed");
    }
    label
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
/// known browser's name, or its app's (`Brave Browser`), may be followed
/// by a profile (`Chrome Profile 1`, `firefox work`); anything else is an
/// app's name, whole, for `open -a`.
pub fn parse(text: &str) -> Option<LinkBrowser> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case(DEFAULT) {
        return None;
    }
    // The longest name the text starts with, as a whole word: `Brave
    // Browser work` is Brave's app, then a profile.
    let lower = text.to_ascii_lowercase();
    let found = BROWSERS
        .iter()
        .flat_map(|b| [(b, b.app), (b, b.name)])
        .filter(|(_, word)| {
            let word = word.to_ascii_lowercase();
            lower == word || lower.starts_with(&format!("{word} "))
        })
        .max_by_key(|(_, word)| word.len());
    let Some((browser, word)) = found else {
        return Some(LinkBrowser {
            app: text.into(),
            profile: None,
        });
    };
    let rest = text[word.len()..].trim();
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
/// (Safari, Opera, Arc, an app typed by name) is ignored.
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
    static INSTALLED: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Run `f` with the browsers `names` installed, as well as Safari.
#[cfg(test)]
pub(crate) fn with_installed<T>(names: &[&'static str], f: impl FnOnce() -> T) -> T {
    let prev = INSTALLED.with(|cell| cell.replace(names.to_vec()));
    let out = f();
    INSTALLED.with(|cell| cell.replace(prev));
    out
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
            (Some(pick("Brave", None)), &["-a", "Brave Browser", URL]),
            (Some(pick("Edge", None)), &["-a", "Microsoft Edge", URL]),
            (
                Some(pick("Vivaldi", Some("Profile 3"))),
                &[
                    "-na",
                    "Vivaldi",
                    "--args",
                    "--profile-directory=Profile 3",
                    URL,
                ],
            ),
            (
                Some(pick("Zen", Some("work"))),
                &["-na", "Zen", "--args", "-P", "work", URL],
            ),
            (Some(pick("Opera", Some("x"))), &["-a", "Opera", URL]),
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
            (Some(pick("SigmaOS", Some("x"))), &["-a", "SigmaOS", URL]),
            // A choice stored by its app's name still finds its profile flag.
            (
                Some(pick("Brave Browser", Some("x"))),
                &[
                    "-na",
                    "Brave Browser",
                    "--args",
                    "--profile-directory=x",
                    URL,
                ],
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
        assert_eq!(parse("Brave Browser"), Some(pick("Brave", None)));
        assert_eq!(
            parse("brave browser Profile 2"),
            Some(pick("Brave", Some("Profile 2")))
        );
        assert_eq!(parse("Microsoft Edge"), Some(pick("Edge", None)));
        assert_eq!(
            parse("Zenith"),
            Some(pick("Zenith", None)),
            "a whole word only"
        );
        assert_eq!(parse("SigmaOS"), Some(pick("SigmaOS", None)));
        // What the row shows, typed back, is what it stored.
        for stored in [pick("Chrome", Some("Profile 1")), pick("Arc", None)] {
            assert_eq!(parse(&typed(Some(&stored))), Some(stored));
        }
        with_installed(&["Chrome"], || {
            assert_eq!(
                label(Some(&pick("Chrome", Some("Profile 1")))),
                "Chrome · Profile 1"
            )
        });
        assert_eq!(label(None), "default");
    }

    /// ←/→ step the browsers this Mac has first, then the ones it doesn't,
    /// which the row marks and `i` installs by their cask.
    #[test]
    fn installed_browsers_come_first_and_missing_ones_say_so() {
        with_installed(&["Firefox", "Brave"], || {
            assert_eq!(
                choices(),
                [
                    "default", "Safari", "Firefox", "Brave", "Chrome", "Edge", "Vivaldi", "Zen",
                    "Opera", "Arc"
                ]
            );
            assert_eq!(label(Some(&pick("Brave", Some("Work")))), "Brave · Work");
            assert_eq!(label(Some(&pick("Safari", None))), "Safari");
            assert_eq!(
                label(Some(&pick("Edge", Some("Profile 2")))),
                "Edge · Profile 2 · not installed"
            );
            // An app typed by name is the user's word; orion can't tell.
            assert_eq!(label(Some(&pick("Orion", None))), "Orion");
            assert_eq!(installed("Orion"), None);

            assert_eq!(
                missing_cask("vivaldi"),
                Some(("Vivaldi", "vivaldi", "https://vivaldi.com/download/"))
            );
            assert_eq!(missing_cask("Brave"), None, "installed");
            assert_eq!(missing_cask("Safari"), None, "comes with macOS");
            assert_eq!(missing_cask("Orion"), None, "not one of orion's");
        });
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
