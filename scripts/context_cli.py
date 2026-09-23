#!/usr/bin/env python3
"""Read graph context through the same configured runtime as checked sessions."""
if __package__:
    from .session_cli import run
else:
    from session_cli import run


if __name__ == "__main__":
    raise SystemExit(run(context=True))
