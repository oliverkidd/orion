# A branch with a commit of its own, and an origin whose main it is not on:
# what the new pull request form fills its title and description from.
git -C "$DEMO" update-ref refs/remotes/origin/main main
FX="$WORK/demo-worktrees/feature-x"
printf 'fn sort() {}\n' > "$FX/sort.rs"
git -C "$FX" add sort.rs
git -C "$FX" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "Sort the list in place" -m "No second buffer: the rows swap where they stand."
