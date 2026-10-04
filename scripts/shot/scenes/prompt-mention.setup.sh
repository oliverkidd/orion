# A few files in the demo checkout for the `@` list to offer.
mkdir -p "$DEMO/src/auth" "$DEMO/docs"
for f in src/auth/tokens.rs src/auth/refresh.rs src/auth/mod.rs src/main.rs docs/auth.md README.md; do
  echo "// $f" > "$DEMO/$f"
done
git -C "$DEMO" add -A
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "files to mention"
