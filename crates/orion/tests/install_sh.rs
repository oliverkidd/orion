//! `install.sh`'s dependency step, run for real against a PATH of stubs: a
//! `brew` that writes down what it was asked and "installs" a formula by
//! dropping a stub of its program beside itself, a `curl` that serves a
//! stand-in for fresh's or micro's quick-install script, a `git` that is
//! just there.
//! The PATH holds nothing else, so no real installer is ever reached and
//! nothing is downloaded. The script's functions run without its `main` —
//! the last line, which would fetch orion itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// install.sh without its closing `main "$@"`.
fn functions() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../install.sh");
    let text = std::fs::read_to_string(path).unwrap();
    text.trim_end()
        .strip_suffix("main \"$@\"")
        .expect("install.sh ends by calling main")
        .to_string()
}

/// The shells the script is held to: POSIX sh, and dash where there is one
/// — the strictest sh a Linux box is likely to run it with.
fn shells() -> Vec<&'static str> {
    ["/bin/sh", "/bin/dash"]
        .into_iter()
        .filter(|shell| Path::new(shell).exists())
        .collect()
}

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for sub in ["bin", "home", "install"] {
            std::fs::create_dir_all(sandbox.path(sub)).unwrap();
        }
        // The few programs the step itself runs.
        for tool in ["sh", "chmod", "mkdir", "uname"] {
            let real = ["/bin", "/usr/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(tool))
                .find(|path| path.exists())
                .unwrap_or_else(|| panic!("no {tool}"));
            std::os::unix::fs::symlink(real, sandbox.path("bin").join(tool)).unwrap();
        }
        sandbox
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    /// A stub `name` on the PATH running `body`.
    fn stub(&self, name: &str, body: &str) -> &Self {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path("bin").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    fn programs(&self, names: &[&str]) -> &Self {
        for name in names {
            self.stub(name, "exit 0");
        }
        self
    }

    /// Homebrew: each `install <formula>` written down and the formula's
    /// program dropped on the PATH (`fresh-editor` installs `fresh`).
    fn brew(&self) -> &Self {
        self.stub(
            "brew",
            r#"echo "brew $*" >> "$LOG"
if [ "$1" = install ]; then
    shift
    for formula in "$@"; do
        case "$formula" in
        --*) ;;
        fresh-editor) printf '#!/bin/sh\n' > "$BIN/fresh"; chmod +x "$BIN/fresh" ;;
        *) printf '#!/bin/sh\n' > "$BIN/$formula"; chmod +x "$BIN/$formula" ;;
        esac
    done
fi"#,
        )
    }

    /// curl: written down, and serving a script that does what the real
    /// one does — fresh's links `fresh` into `$FRESH_BIN_DIR`, micro's
    /// leaves `micro` in the folder it runs in.
    fn curl(&self) -> &Self {
        self.stub(
            "curl",
            r##"echo "curl $*" >> "$LOG"
case "$*" in
*sinelaw/fresh*) echo 'printf "#!/bin/sh\n" > "$FRESH_BIN_DIR/fresh"; chmod +x "$FRESH_BIN_DIR/fresh"' ;;
*) echo 'printf "#!/bin/sh\n" > micro; chmod +x micro' ;;
esac"##,
        )
    }

    /// `uname` saying this is `system`, in place of the machine's.
    fn system(&self, system: &str) -> &Self {
        std::fs::remove_file(self.path("bin").join("uname")).unwrap();
        self.stub("uname", &format!("echo {system}"))
    }

    /// `call` after the script's functions, in `shell`, with nothing but
    /// the sandbox on PATH.
    fn run(&self, shell: &str, call: &str, env: &[(&str, &str)]) -> Output {
        Command::new(shell)
            .arg("-c")
            .arg(format!("{}\n{call}\n", functions()))
            .env_clear()
            .env("PATH", self.path("bin"))
            .env("HOME", self.path("home"))
            .env("ORION_INSTALL_DIR", self.path("install"))
            .env("LOG", self.path("log"))
            .env("BIN", self.path("bin"))
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.path("log")).unwrap_or_default()
    }
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_script_is_posix_sh() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../install.sh");
    for shell in shells() {
        let out = Command::new(shell).arg("-n").arg(&script).output().unwrap();
        assert!(out.status.success(), "{shell} -n: {}", said(&out));
    }
}

/// Everything already here: not a word, nothing run.
#[test]
fn nothing_is_said_or_run_when_everything_is_here() {
    for shell in shells() {
        for editor in ["fresh", "micro", "edit"] {
            let sandbox = Sandbox::new();
            sandbox.programs(&["git", "gh", editor]).brew().curl();
            let out = sandbox.run(shell, "ensure_deps", &[]);
            assert!(out.status.success(), "{shell}: {}", said(&out));
            assert_eq!(said(&out), "", "{shell} with {editor}");
            assert_eq!(sandbox.log(), "", "{shell} with {editor}");
        }
    }
}

/// No editor of the three: fresh from Homebrew, and gh with it.
#[test]
fn fresh_and_gh_come_from_homebrew() {
    const LOG: &str = "brew install fresh-editor\nbrew install gh\n";
    for shell in shells() {
        let sandbox = Sandbox::new();
        sandbox.programs(&["git", "vim"]).brew().curl();
        let out = sandbox.run(shell, "ensure_deps", &[]);
        assert!(out.status.success(), "{shell}: {}", said(&out));
        assert_eq!(sandbox.log(), LOG);
        assert!(
            said(&out).contains("(brew install fresh-editor)"),
            "{}",
            said(&out)
        );
        assert!(said(&out).contains("gh auth login"), "{}", said(&out));
        assert!(sandbox.path("bin/fresh").exists());

        // Run again: all of it is here now, and nothing more is done.
        let again = sandbox.run(shell, "ensure_deps", &[]);
        assert_eq!(said(&again), "");
        assert_eq!(sandbox.log(), LOG);
    }
}

/// No Homebrew on Linux: fresh by its own quick-install script, linked
/// into the install dir; gh pointed at, not installed.
#[test]
fn without_homebrew_on_linux_fresh_lands_in_the_install_dir() {
    for shell in shells() {
        let sandbox = Sandbox::new();
        sandbox.programs(&["git"]).curl().system("Linux");
        let out = sandbox.run(shell, "ensure_deps", &[]);
        assert!(out.status.success(), "{shell}: {}", said(&out));
        assert_eq!(
            sandbox.log(),
            "curl -fsSL https://raw.githubusercontent.com/sinelaw/fresh/refs/heads/master/scripts/install.sh\n"
        );
        assert!(sandbox.path("install/fresh").exists(), "{}", said(&out));
        assert!(
            said(&out).contains("https://github.com/cli/cli#installation"),
            "{}",
            said(&out)
        );
    }
}

/// No Homebrew on a Mac, where fresh's script would want it: micro by its
/// quick-install script, into the install dir.
#[test]
fn without_homebrew_on_a_mac_micro_lands_in_the_install_dir() {
    for shell in shells() {
        let sandbox = Sandbox::new();
        sandbox.programs(&["git"]).curl().system("Darwin");
        let out = sandbox.run(shell, "ensure_deps", &[]);
        assert!(out.status.success(), "{shell}: {}", said(&out));
        assert_eq!(sandbox.log(), "curl -fsSL https://getmic.ro\n");
        assert!(sandbox.path("install/micro").exists(), "{}", said(&out));
    }
}

/// An installer that fails: a warning, and the install carries on.
#[test]
fn a_failed_editor_install_warns_and_carries_on() {
    let sandbox = Sandbox::new();
    sandbox
        .programs(&["git", "gh"])
        .stub("brew", r#"echo "brew $*" >> "$LOG"; exit 1"#);
    let out = sandbox.run("/bin/sh", "ensure_deps; echo carried-on", &[]);
    assert!(out.status.success(), "{}", said(&out));
    assert!(
        said(&out).contains("warning: couldn't install fresh"),
        "{}",
        said(&out)
    );
    assert!(said(&out).ends_with("carried-on\n"), "{}", said(&out));
}

/// No git: the install stops, saying how to get it.
#[test]
fn git_is_required() {
    for shell in shells() {
        let sandbox = Sandbox::new();
        sandbox.programs(&["micro", "gh"]).brew();
        let out = sandbox.run(shell, "ensure_deps", &[]);
        assert!(!out.status.success(), "{shell}");
        assert!(said(&out).contains("orion needs git"), "{}", said(&out));
        assert_eq!(sandbox.log(), "", "nothing installed in its place");
    }
}

/// `--no-deps` and `ORION_NO_DEPS=1` skip the step; anything else on the
/// command line is refused.
#[test]
fn the_step_can_be_skipped() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(
        "/bin/sh",
        r#"parse_args --no-deps; echo "skip=$NO_DEPS""#,
        &[],
    );
    assert_eq!(said(&out), "skip=1\n");
    let out = sandbox.run(
        "/bin/sh",
        r#"parse_args; echo "skip=$NO_DEPS""#,
        &[("ORION_NO_DEPS", "1")],
    );
    assert_eq!(said(&out), "skip=1\n");
    let out = sandbox.run("/bin/sh", r#"parse_args; echo "skip=$NO_DEPS""#, &[]);
    assert_eq!(said(&out), "skip=\n");
    let out = sandbox.run("/bin/sh", "parse_args --deps", &[]);
    assert!(!out.status.success());
    assert!(said(&out).contains("unknown option: --deps"));
}
