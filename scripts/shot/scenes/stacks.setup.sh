# STACK STATUS: a stand-in `docker` on PATH for the DAEMON's poll — feature-x's compose stack running,
# the root checkout's stopped, and one started from a checkout orion doesn't know. Verbs just succeed.
# The paths as git records them — macOS's /var is /private/var.
REAL_DEMO="$(cd "$DEMO" && pwd -P)"
REAL_FEATURE="$(cd "$WORK/demo-worktrees/feature-x" && pwd -P)"
mkdir -p "$WORK/stub-docker"
cat > "$WORK/stub-docker/docker" <<STUB
#!/bin/sh
if [ "\$1" = ps ]; then
  for i in 1 2 3 4 5; do printf 'demo-feature-x\t%s\trunning\n' "$REAL_FEATURE"; done
  for i in 1 2 3; do printf 'demo-main\t%s\texited\n' "$REAL_DEMO"; done
  for i in 1 2; do printf 'scratch-api\t%s\trunning\n' "\$HOME/Code/scratch-api"; done
fi
exit 0
STUB
chmod +x "$WORK/stub-docker/docker"
export PATH="$WORK/stub-docker:$PATH"
