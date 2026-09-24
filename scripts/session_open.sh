#!/bin/sh
# Both existing-record opening and first-use guidance share the host payload's cwd. The
# host's hook may name itself (--host claude|codex) so the opener's next moves read as that
# host invokes a skill; without it the reader's own verbs are named.
#
# Claude Code runs what a SessionStart hook writes to CLAUDE_ENV_FILE before each shell command
# of the session. The package's kpop and kpopper go there at the end of PATH, where Claude Code
# puts a plugin's bin/ - a directory a claude.ai-hosted plugin may not ship.
if [ -n "${CLAUDE_ENV_FILE:-}" ] && package_bin=$(CDPATH='' cd -- "$(dirname -- "$0")/bin" 2>/dev/null && pwd); then
    quoted=$(printf '%s\n' "$package_bin" | sed "s/'/'\\\\''/g")
    line="export PATH=\"\$PATH\":'$quoted'"
    { grep -qxF -- "$line" "$CLAUDE_ENV_FILE" || printf '%s\n' "$line" >> "$CLAUDE_ENV_FILE"; } 2>/dev/null
fi
exec sh "$(dirname "$0")/hook.sh" session_start.py "$@"
