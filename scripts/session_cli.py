#!/usr/bin/env python3
"""Run the same session transport from an installed package or plugin checkout."""
if __package__:
    from .session.entry import main
else:
    # A direct script has scripts/ as its trusted import root. Give the parent
    # package a name so the session adapter can import the bundled reader.
    import pathlib
    import importlib
    import sys
    package_dir = pathlib.Path(__file__).resolve().parent
    sys.path.insert(0, str(package_dir.parent))
    main = importlib.import_module(package_dir.name + ".session.entry").main

if __name__ == "__main__":
    import os
    import importlib
    import pathlib
    import sys
    operation = sys.argv[1] if len(sys.argv) > 1 else ""
    if operation not in {"setup", "status", "enable", "disable", "serve", "-h", "--help", ""} and not os.environ.get("KPOPPER_SESSION_BOOTSTRAPPED"):
        package_dir = pathlib.Path(__file__).resolve().parent
        args = importlib.import_module(package_dir.name + ".session.entry").parser().parse_args()
        directory = args.input.expanduser().resolve().parent if args.input else None
        config = {} if args.no_settings else importlib.import_module(package_dir.name + ".session.settings").current(directory)
        target = config.get("python") if config.get("enabled") else None
        if target and str(pathlib.Path(sys.executable).absolute()) != target:
            os.environ["KPOPPER_SESSION_BOOTSTRAPPED"] = "1"
            if os.name == "nt":
                import subprocess
                raise SystemExit(subprocess.call([target] + sys.argv))
            os.execv(target, [target] + sys.argv)
    raise SystemExit(main())
