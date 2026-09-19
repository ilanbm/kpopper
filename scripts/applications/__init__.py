"""Optional applications; importing their catalog needs only the standard library."""
import importlib
import json
import sys

HTML_REQUIREMENTS = ("html5lib>=1.1,<2", "tinycss2>=1.2,<2")
HTML_MODULES = ("html5lib", "tinycss2")
APPLICATIONS = {
    "hub": {"display_name": "kpopper Hub", "description": "Render or verify the record's HTML page.",
             "usage": "[--open] [--out PATH] [--tree] [--verify] [--checks]"},
    "annotated-doc": {"display_name": "Annotated Documents", "description": "Author and refresh standalone HTML with evidence.",
                 "usage": "guide|build|inspect|refresh [OPTIONS]"},
}


ALIASES = {"page": "hub", "document": "annotated-doc"}

def catalog():
    return [{"name": name, "layer": "application", "status": "experimental",
             "extra": "html", "command": "kpop experimental " + name,
             "aliases": [alias for alias, target in ALIASES.items() if target == name], **details}
            for name, details in APPLICATIONS.items()]


def show_catalog(as_json=False):
    if as_json:
        print(json.dumps({"applications": catalog()}))
        return
    print("usage: kpop experimental APPLICATION [OPTIONS]\n\nExperimental applications:")
    for app in catalog():
        print("  %-30s %s" % (app["command"], app["description"]))
    print("\nOptional runtime: python -m pip install 'kpopper[html]'\n"
          "Plugin: python3 /path/to/plugin/scripts/plugin_runtime.py setup --applications html\n"
          "Interfaces and artifact formats may change. Use APPLICATION --help for details.")


def require_html():
    """Diagnose missing extras without installing anything or importing a renderer."""
    try:
        for name in HTML_MODULES:
            importlib.import_module(name)
    except ImportError as error:
        raise ValueError("HTML application dependencies unavailable: %s.\n"
                         "Install with: %s -m pip install 'kpopper[html]'\n"
                         "For a plugin: python3 /path/to/plugin/scripts/plugin_runtime.py "
                         "setup --applications html" % (error, sys.executable)) from None
