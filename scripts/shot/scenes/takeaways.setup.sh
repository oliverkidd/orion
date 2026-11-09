# Meeting takeaways (5.8): a Standup that ended this morning, read already — six takeaways, three of
# them yours and accepted — kept where meeting takeaways keeps its meetings, so no Microsoft 365 or
# model is asked. The demo project (linked to takeaways out of the box) has no todos yet, so its
# groups show as new; Blue sky's list has Ideas, with one idea in it, and Tools › CLI.
repo="$(cd "$DEMO" && pwd -P)"
today="$(date +%F)"
start="$(date -u +%Y-%m-%dT09:00:00Z)"; end="$(date -u +%Y-%m-%dT09:32:00Z)"
mkdir -p "$WORK/data/automations" "$WORK/data/todos"
cat > "$WORK/data/todos/blue-sky.json" <<JSON
{"version": 2, "repo_path": "blue sky", "next_id": 10,
 "groups": [{"id": 1, "name": "Ideas"}, {"id": 3, "name": "Tools"}, {"id": 4, "parent": 3, "name": "CLI"}],
 "items": [{"id": 2, "group": 1, "text": "Share a session as a read-only link", "created": "$today"}]}
JSON
cat > "$WORK/data/automations/meeting-takeaways.json" <<JSON
{"meetings": [{
  "id": "evt-standup", "subject": "Standup", "start": "$start", "end": "$end",
  "join_url": "https://teams.microsoft.com/l/meetup-join/19%3ameeting_demo", "stage": "to_place",
  "me": "Ada Lovelace",
  "takeaways": [
    {"text": "Fix the invoices 500 when there's no billing address", "owner": "AL", "mine": true, "accepted": true,
     "dest": "$repo", "group_path": ["Billing"], "priority": 2, "ticket": "ENG-412"},
    {"text": "Look at the HEIC avatar crash after the invoices fix", "owner": "AL", "mine": true, "accepted": true,
     "dest": "$repo", "group_path": ["Mobile", "Crashes"], "priority": 1},
    {"text": "Update the release notes for the mobile build", "owner": "GH", "mine": false, "accepted": false,
     "dest": "$repo", "group_path": ["Mobile", "Release"]},
    {"text": "A little habit tracker that nags you to stretch", "owner": "AL", "mine": true, "accepted": true,
     "dest": "blue sky", "group_path": ["Ideas"]},
    {"text": "Book the room for the offsite", "owner": "KJ", "mine": false, "accepted": false, "dest": null},
    {"text": "Send the design review notes round", "owner": "GH", "mine": false, "accepted": false,
     "dest": "$repo", "group_path": ["Design"]}
  ]
}]}
JSON
