#!/bin/sh
# orion installer — installs or updates the `orion` binary.
#
#   curl -fsSL https://raw.githubusercontent.com/oliverkidd/orion/main/install.sh | sh
#
# Grabs the prebuilt binary for this platform from the latest GitHub release,
# falling back to `cargo install --git` when no release (or no matching asset)
# exists. Then makes sure of what orion leans on: git (required), an editor it
# opens files in (fresh, unless fresh, micro or Microsoft Edit is already
# here) and gh for pull requests and issues. Running it again updates in
# place, and says nothing about dependencies that are already here.
#
#   curl -fsSL …/install.sh | sh -s -- --no-deps    orion alone
#
# Environment overrides:
#   ORION_INSTALL_DIR   install destination (default: ~/.local/bin)
#   ORION_NO_DEPS=1     skip the dependency step (same as --no-deps)
set -eu

REPO="oliverkidd/orion"
INSTALL_DIR="${ORION_INSTALL_DIR:-$HOME/.local/bin}"
NO_DEPS="${ORION_NO_DEPS:-}"

say() { printf '%s\n' "$*"; }
err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

parse_args() {
    for arg in "$@"; do
        case "$arg" in
        --no-deps) NO_DEPS=1 ;;
        *) err "unknown option: $arg (the one option is --no-deps)" ;;
        esac
    done
}

detect_target() {
    case "$(uname -s)" in
    Darwin)
        case "$(uname -m)" in
        arm64) echo aarch64-apple-darwin ;;
        x86_64) echo x86_64-apple-darwin ;;
        *) return 1 ;;
        esac
        ;;
    Linux)
        case "$(uname -m)" in
        x86_64) echo x86_64-unknown-linux-musl ;;
        aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
        *) return 1 ;;
        esac
        ;;
    *) err "orion runs on macOS and Linux only (the daemon needs unix sockets)" ;;
    esac
}

install_from_release() {
    url="https://github.com/$REPO/releases/latest/download/orion-$1.tar.gz"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    say "downloading $url"
    curl -fsSL "$url" -o "$tmp/orion.tar.gz" || return 1
    tar -xzf "$tmp/orion.tar.gz" -C "$tmp"
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$tmp/orion" "$INSTALL_DIR/orion"
}

install_from_source() {
    command -v cargo >/dev/null 2>&1 ||
        err "no prebuilt binary available and cargo is not installed — get Rust from https://rustup.rs and re-run"
    say "building from source (this takes a few minutes)…"
    cargo install --git "https://github.com/$REPO" orion --locked --force
}

# ---- what orion leans on ----
#
# Quiet when everything is already here; `orion doctor` checks the same
# things (and more) at any time, and never installs anything itself.

# git: every project is a git repository. Nothing orion does works without it.
need_git() {
    have git && return 0
    case "$(uname -s)" in
    Darwin) err "orion needs git — run 'xcode-select --install' (or 'brew install git'), then re-run this" ;;
    *) err "orion needs git — install it with your package manager (https://git-scm.com/downloads), then re-run this" ;;
    esac
}

# An editor orion opens files in, with VS Code's keys: fresh, micro or
# Microsoft Edit (`edit`). With none of them here, fresh — from Homebrew;
# without it, on Linux, with the quick-install script fresh's README links
# (https://github.com/sinelaw/fresh#installation), its universal build with
# `fresh` linked into the install dir. That script wants Homebrew on macOS,
# so there — or when it fails — micro instead, with the quick-install script
# micro's README links (https://getmic.ro), which drops the binary into the
# folder it runs in: the install dir.
ensure_editor() {
    for editor in fresh micro edit; do
        have "$editor" && return 0
    done
    if have brew; then
        say "installing fresh, the editor orion opens files in (brew install fresh-editor)…"
        brew install fresh-editor && return 0
    else
        mkdir -p "$INSTALL_DIR"
        if [ "$(uname -s)" = Linux ]; then
            say "installing fresh, the editor orion opens files in, into $INSTALL_DIR…"
            curl -fsSL https://raw.githubusercontent.com/sinelaw/fresh/refs/heads/master/scripts/install.sh |
                FRESH_BIN_DIR="$INSTALL_DIR" sh -s -- --method=tarball || true
            [ -x "$INSTALL_DIR/fresh" ] && return 0
        fi
        say "installing micro, the editor orion opens files in, into $INSTALL_DIR…"
        # A pipe's status is its last command's: ask the folder, not sh.
        (cd "$INSTALL_DIR" && curl -fsSL https://getmic.ro | sh) || true
        [ -x "$INSTALL_DIR/micro" ] && return 0
    fi
    say "warning: couldn't install fresh — files open in vim until it is (https://github.com/sinelaw/fresh#installation)"
}

# gh, the GitHub CLI orion reads pull requests and issues with. Recommended,
# not required: installed with Homebrew when there is one, else pointed at.
ensure_gh() {
    have gh && return 0
    if have brew; then
        say "installing gh, the GitHub CLI orion reads pull requests and issues with (brew install gh)…"
        brew install gh && say "then sign it in: gh auth login" && return 0
        say "warning: couldn't install gh — pull requests and issues need it: https://github.com/cli/cli#installation"
    else
        say "note: orion reads pull requests and issues with gh, the GitHub CLI — install it from https://github.com/cli/cli#installation"
    fi
}

ensure_deps() {
    need_git
    ensure_editor
    ensure_gh
}

main() {
    parse_args "$@"
    command -v curl >/dev/null 2>&1 || err "curl is required"

    installed=""
    if target=$(detect_target); then
        if install_from_release "$target"; then
            installed="$INSTALL_DIR/orion"
        else
            reason="couldn't download orion-$target from the latest release"
        fi
    else
        reason="no prebuilt binary for $(uname -s) $(uname -m)"
    fi

    if [ -z "$installed" ]; then
        say "$reason — building from source instead"
        install_from_source
        installed="$HOME/.cargo/bin/orion"
    fi

    version=$("$installed" --version 2>/dev/null || echo orion)
    say "installed $version → $installed"

    bin_dir=$(dirname "$installed")
    case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) say "note: $bin_dir is not on your PATH — add: export PATH=\"$bin_dir:\$PATH\"" ;;
    esac

    if [ "$NO_DEPS" != 1 ]; then
        ensure_deps
    fi

    # Replacing the file doesn't touch an already-running daemon: sessions keep
    # running on the old binary until it's restarted, and `orion kill` is the
    # user's call because it stops every session. `orion upgrade` handles this
    # itself (shutting down an idle daemon) and suppresses the note via
    # ORION_UPGRADE_HANDOFF.
    if [ -z "${ORION_UPGRADE_HANDOFF:-}" ] && pgrep -f 'orion daemon' >/dev/null 2>&1; then
        say "note: a orion daemon from the previous version is still running."
        say "      run 'orion kill' to restart onto the new one (stops all sessions)."
    fi
}

main "$@"
