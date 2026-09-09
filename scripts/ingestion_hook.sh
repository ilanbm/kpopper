#!/bin/sh
# Host is explicit in each platform's hook config; inherited environment is not a detector.
exec python3 "$(dirname "$0")/ingestion_hooks.py" "$@"
