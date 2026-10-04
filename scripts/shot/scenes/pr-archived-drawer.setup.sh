# The stand-in home of pr-accounts.setup.sh, so the box names a placeholder account, never a real one.
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
