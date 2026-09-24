#!/bin/sh
# Host is explicit in each platform's hook config; inherited environment is not a detector.
exec sh "$(dirname "$0")/hook.sh" ingestion_hooks.py "$@"
