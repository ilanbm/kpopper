#!/bin/sh
# Both existing-record opening and first-use guidance share the host payload's cwd. The
# host's hook may name itself (--host claude|codex) so the opener's next moves read as that
# host invokes a skill; without it the reader's own verbs are named.
exec sh "$(dirname "$0")/hook.sh" session_start.py "$@"
