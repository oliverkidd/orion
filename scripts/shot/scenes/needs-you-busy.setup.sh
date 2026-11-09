# NEEDS YOU with something in every column it can fill without a network: the README grid's three
# projects and their stand-in agent (needs-you-busy.keys launches three of it), and
# three pull requests on other repos asking for your review, answered by the stand-in `gh search prs`.
# The poll is on (it is off in every other scene) so the search runs; Linear is never asked.
. "$HERE/scenes/readme-grid.setup.sh"
unset LINEAR_API_KEY
export ORION_NEEDS_YOU_POLL_SECS=300
cat > "$ORION_GH_FIXTURES/search-prs.json" <<'JSON'
[
  {"url": "https://github.com/orbit/orbit-web/pull/212", "number": 212, "title": "Show token expiry on the account page",
   "repository": {"name": "orbit-web", "nameWithOwner": "orbit/orbit-web"}, "author": {"login": "maya"},
   "createdAt": "2026-11-07T09:12:00Z"},
  {"url": "https://github.com/orbit/infra/pull/88", "number": 88, "title": "Move the staging database to the new cluster",
   "repository": {"name": "infra", "nameWithOwner": "orbit/infra"}, "author": {"login": "sam"},
   "createdAt": "2026-11-06T16:40:00Z"},
  {"url": "https://github.com/orbit/sdk/pull/31", "number": 31, "title": "Typed errors for the retry helper",
   "repository": {"name": "sdk", "nameWithOwner": "orbit/sdk"}, "author": {"login": "li"},
   "createdAt": "2026-11-08T08:05:00Z"}
]
JSON
