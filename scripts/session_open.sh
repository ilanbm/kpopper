#!/bin/sh
# The session opener, run by the plugin's SessionStart hook. Prints nothing when
# the directory keeps no record, so projects without one stay untouched.
[ -f PROVENANCE.yaml ] || exit 0
python3 "$(dirname "$0")/provenance.py" open --chars 2000 2>/dev/null \
  || echo "PROVENANCE.yaml is here but the reader could not run (python3 + PyYAML): read the record before relying on it."
