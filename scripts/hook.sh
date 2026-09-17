#!/bin/sh
# stdlib-only bootstrap; setup is explicit and never runs from a hook.
export PYTHONIOENCODING=utf-8
exec python3 "$(dirname "$0")/plugin_runtime.py" hook "$@"
