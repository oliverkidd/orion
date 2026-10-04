# The HOME splash's sky in the slate preset: `theme` written before boot, the PREWARM POOL off.
mkdir -p "$WORK/data"
printf '{"theme":"slate","prewarm_agents":false,"prewarm_sessions":false}\n' > "$WORK/data/config.json"
