"""Compatibility entry point for the experimental annotated-doc application."""
import sys
try:
    from .applications.annotated_doc import main, parser
except ImportError:
    from applications.annotated_doc import main, parser

if __name__ == "__main__":
    raise SystemExit(main())
