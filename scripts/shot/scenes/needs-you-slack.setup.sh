# NEEDS YOU with Slack cards, without a Slack: the cards the SLACK section saves in needs-you.json
# (`needs_you::slack::Kept`) are what a restart shows, so the scene writes that file as a run that
# heard these messages would have left it — a DM, a group DM, a mention and a reply in your thread,
# each a few minutes old, with the names Slack gave.
mkdir -p "$WORK/data"
now=$(date +%s)
ts() { echo "$((now - $1 * 60)).000100"; }
cat > "$WORK/data/needs-you.json" <<JSON
{
  "slack": {
    "waiting": [
      {"channel": "D01", "root": null, "in_thread": false, "direct": true, "user": "U01",
       "text": "Can you look at the token refresh PR before standup?", "ts": "$(ts 4)", "why": "direct"},
      {"channel": "G02", "root": null, "in_thread": false, "direct": true, "user": "U02",
       "text": "Staging is back up &amp; seeded, try the checkout flow again", "ts": "$(ts 18)", "why": "direct"},
      {"channel": "C03", "root": "$(ts 35)", "in_thread": false, "direct": false, "user": "U03",
       "text": "<@U00> is the --json flag going into this release?", "ts": "$(ts 35)", "why": "mention"},
      {"channel": "C04", "root": "$(ts 90)", "in_thread": true, "direct": false, "user": "U01",
       "text": "Rolled back, the migration needs a second look\nLogs attached", "ts": "$(ts 12)", "why": "thread"}
    ],
    "threads": [["C04", "$(ts 90)"]],
    "users": {"U00": "you", "U01": "Maya Chen", "U02": "Sam Ortiz", "U03": "Li Wei"},
    "channels": {"C03": "eng", "C04": "deploys"}
  }
}
JSON
