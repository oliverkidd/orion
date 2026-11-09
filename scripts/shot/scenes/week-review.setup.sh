# Two week in review runs kept in the data dir, as a run keeps one: reviews/all/<to>/ with a review
# for each project that had changes and the run's meta.json. Nothing asks GitHub or a model.
RUN="$WORK/data/reviews/all/2026-11-06"
mkdir -p "$RUN"
cat > "$RUN/web.md" <<'MD'
# Week in review · 31 October – 6 November
web · just me · 7 pull requests merged

## Search
1. Results land as you type · #412
   - Show: Press ⌘K, type "inv": invoices land first, pages after.
   - Say: First results in 80 ms, down from 600.
2. Filters stay put between visits · #431

## Billing
1. Invoices download as PDF · #398
   - Show: Open an invoice, press Download: the PDF opens.

+ 4 other pull requests merged, not listed
MD
cat > "$RUN/api.md" <<'MD'
# Week in review · 31 October – 6 November
api · just me · 3 pull requests merged

## Webhooks
1. Webhooks retry for a day · #77
   - Say: A failed delivery is tried 12 times before it gives up.
2. Signing secrets rotate without downtime · #81

+ 1 other pull request merged, not listed
MD
# Fri 6 Nov 08:30 local, as the schedule would have written it.
cat > "$RUN/meta.json" <<JSON
{"from": "2026-10-31", "to": "2026-11-06", "written_at": $(date -j -f "%Y-%m-%d %H:%M:%S" "2026-11-06 08:30:00" +%s),
 "account": "Work (you@example.com)", "model": "sonnet", "effort": "medium",
 "reviews": [{"project": "web", "slug": "web", "points": 3}, {"project": "api", "slug": "api", "points": 2}],
 "failed": [], "read": false}
JSON
OLD="$WORK/data/reviews/all/2026-10-30"
mkdir -p "$OLD"
cp "$RUN/web.md" "$OLD/web.md"
cat > "$OLD/meta.json" <<JSON
{"from": "2026-10-24", "to": "2026-10-30", "written_at": $(date -j -f "%Y-%m-%d %H:%M:%S" "2026-10-30 08:31:00" +%s),
 "reviews": [{"project": "web", "slug": "web", "points": 9}], "read": true}
JSON
