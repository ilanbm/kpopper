"""The suite's own state directory.

The reader keeps a parsed record under the user's state directory, and the suite reads
thousands of records it makes and throws away. Point the whole run at a directory of its own,
before any test imports the reader, so a run leaves nothing behind in the state the person
using this tool keeps - and the tests still exercise the same path they would at a keyboard.

A run that sets XDG_STATE_HOME itself is left alone.
"""
import os
import tempfile

if not os.environ.get("XDG_STATE_HOME"):
    os.environ["XDG_STATE_HOME"] = tempfile.mkdtemp(prefix="kpopper-tests-state.")
