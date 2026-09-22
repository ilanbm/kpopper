#!/bin/sh
# stdlib-only bootstrap; setup is explicit and never runs from a hook.
export PYTHONIOENCODING=utf-8
python3 "$(dirname "$0")/plugin_runtime.py" hook "$@"
# Diagnostics belong to context; an unexpected child exit must not reject a tool
# result or turn an async completion into a fresh user request.
exit 0
