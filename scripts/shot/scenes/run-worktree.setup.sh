# A project that says how it is run: its Run command (Settings → Projects) is a stand-in dev server
# (dev-server.sh: a banner, then a request line every two seconds, ending on its own after two
# minutes so a stray run never outlives the harness) and its Open command names its page. The entry
# is keyed by the physical path, as the DAEMON stores the project. SHELL is
# /bin/sh so the run's pane shows the run rather than this machine's login profile, and the keys wait
# long enough for the run's first lines to land.
cat > "$DEMO/dev-server.sh" <<'SH'
printf '\n  demo dev server\n\n  Local:   http://localhost:3000/\n\n'
for i in $(seq 60); do sleep 2; echo "  GET / 200 in ${i}ms"; done
SH
git -C "$DEMO" add dev-server.sh
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "how the demo runs"
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<JSON
{"claude_enabled": true, "codex_enabled": true, "cursor_enabled": true,
 "pi_enabled": true, "muse_enabled": true, "opencode_enabled": true,
 "projects": {"$(cd "$DEMO" && pwd -P)": {"run_command": "sh dev-server.sh",
                                         "open_command": "open http://localhost:3000"}}}
JSON
export SHELL=/bin/sh
export SHOT_KEY_SECS=1.5
