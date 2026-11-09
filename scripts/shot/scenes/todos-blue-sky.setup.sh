# Todos › Blue sky (5.9): a list of ideas in two groups, one built already into its own folder in the
# Blue sky folder, the rest not started. The Blue sky folder is a scratch one: no scene makes the real
# ~/Code/orion-blue-sky.
export ORION_BLUE_SKY_DIR="$WORK/orion-blue-sky"
mkdir -p "$WORK/data/todos" "$ORION_BLUE_SKY_DIR/habit-tracker"
today="$(date +%F)"
cat > "$WORK/data/todos/blue-sky.json" <<JSON
{"version": 2, "repo_path": "blue sky", "next_id": 10,
 "groups": [{"id": 1, "name": "Product"}, {"id": 2, "name": "Tools"}],
 "items": [
  {"id": 3, "group": 1, "text": "Onboarding tour that plays the first session for you", "created": "$today"},
  {"id": 4, "group": 1, "text": "Share a session as a read-only link", "created": "$today"},
  {"id": 5, "group": 2, "text": "A tiny habit tracker", "created": "$today", "built_in": "habit-tracker/"},
  {"id": 6, "group": 2, "text": "Changelog to podcast script", "created": "$today"}
 ]}
JSON
