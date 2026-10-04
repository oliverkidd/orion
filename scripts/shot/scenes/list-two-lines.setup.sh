# The README grid's projects and stand-in agent, in the compact LIST layout (Settings → Appearance →
# Worktree layout: list), so every entry has a prompt for its second line.
. "$HERE/scenes/readme-grid.setup.sh"
python3 - "$WORK/data/config.json" <<'PY'
import json, sys
path = sys.argv[1]
cfg = json.load(open(path))
cfg["worktree_layout"] = "list"
json.dump(cfg, open(path, "w"))
PY
