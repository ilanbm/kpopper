"""Public authoring/refresh entry point for a single portable HTML copy."""
import argparse
import json
from pathlib import Path
import sys

try:
    from . import documents as D
except ImportError:
    import documents as D


def parser():
    result = argparse.ArgumentParser(prog="kpop document", description="Author and refresh standalone HTML documents with an evidence layer.")
    sub = result.add_subparsers(dest="command", required=True)
    sub.add_parser("guide", help="read the authoring contract and examples")
    build = sub.add_parser("build", help="package authored HTML and source checks in one file")
    build.add_argument("--html", required=True, type=Path)
    build.add_argument("--manifest", required=True, type=Path)
    refresh = sub.add_parser("refresh", help="reread explicit sources and prepare a correction on a new copy")
    refresh.add_argument("artifact", type=Path)
    refresh.add_argument("--sources", required=True, type=Path, help="JSON object mapping source IDs to local source inputs")
    for command in (build, refresh):
        command.add_argument("--root", type=Path, help="source root; defaults to the manifest/source-input directory")
        command.add_argument("--out", required=True, type=Path)
        command.add_argument("--overwrite", action="store_true", help="replace an existing output copy, never an input/source")
    inspect = sub.add_parser("inspect", help="validate and read the embedded checks, sources and decisions without running author code")
    inspect.add_argument("artifact", type=Path)
    return result


def main(argv=None):
    options = parser().parse_args(argv)
    try:
        if options.command == "guide":
            print((Path(__file__).with_name("document-guide.md")).read_text(encoding="utf-8"))
            return 0
        if options.command == "inspect":
            data = D.load_artifact(D.read_bytes(options.artifact, D.MAX_ARTIFACT).decode("utf-8"))
            print(json.dumps(D.summary(data), ensure_ascii=False, indent=2))
            return 0
        inputs = options.manifest if options.command == "build" else options.sources
        root = (options.root or inputs.parent).resolve()
        manifest = D.read_json(D.read_bytes(inputs).decode("utf-8"))
        protected = [inputs]
        source_inputs = manifest.get("sources", {}) if options.command == "build" else manifest
        if not isinstance(source_inputs, dict):
            raise D.DocumentError("Source inputs must be an object")
        for value in source_inputs.values():
            if isinstance(value, dict) and "path" in value:
                protected.append(D.scoped(root, value["path"]))
        if options.command == "build":
            protected.append(options.html)
            data = D.build(D.read_bytes(options.html, D.MAX_ARTIFACT).decode("utf-8"), manifest, root)
        else:
            protected.append(options.artifact)
            original = D.load_artifact(D.read_bytes(options.artifact, D.MAX_ARTIFACT).decode("utf-8"))
            data = D.refresh(original, source_inputs, root)
        output = D.write_output(data, options.out, options.overwrite, protected)
        counts = {status: sum(c["status"] == status for c in data["checks"].values())
                  for status in ("match", "mismatch", "unavailable", "unchecked")}
        print(json.dumps({"output": str(output), "checks": counts, "coverage": data["coverage"],
                          "proposals": len(data["groups"]), "source_files_changed": False}, ensure_ascii=False, indent=2))
        return 0
    except (D.DocumentError, OSError, UnicodeError, KeyError, TypeError) as error:
        print("document: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
