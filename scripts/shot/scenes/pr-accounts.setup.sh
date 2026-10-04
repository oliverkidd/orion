# A stand-in home for the CLAUDE ACCOUNTS scenes, so no real login is ever read or shown: the default
# account signed in as you@example.com, an added account named "Work" in ~/.claude-work signed in as
# work@example.com, and ~/.claude-old — a dir a removal left behind — signed in as old@example.com.
export HOME="$WORK/home"; unset CLAUDE_CONFIG_DIR
mkdir -p "$HOME/.claude" "$HOME/.claude-work" "$HOME/.claude-old/projects"
printf '{"oauthAccount":{"emailAddress":"you@example.com"}}' > "$HOME/.claude.json"
printf '{"oauthAccount":{"emailAddress":"work@example.com"}}' > "$HOME/.claude-work/.claude.json"
printf '{"oauthAccount":{"emailAddress":"old@example.com"}}' > "$HOME/.claude-old/.claude.json"
mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<JSON
{"claude_enabled": true, "codex_enabled": true, "cursor_enabled": true,
 "pi_enabled": true, "muse_enabled": true, "opencode_enabled": true,
 "claude_accounts": [{"id": "claude-work", "name": "Work", "config_dir": "$HOME/.claude-work"}]}
JSON
