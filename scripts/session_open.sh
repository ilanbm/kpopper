#!/bin/sh
# Both existing-record opening and first-use guidance share the host payload's cwd.
exec python3 "$(dirname "$0")/session_start.py"
