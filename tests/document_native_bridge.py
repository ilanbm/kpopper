"""Call the checkout-built native test bridge; never load the removed Python reader."""
import json
import os
from pathlib import Path
import subprocess
import tempfile


def call(operation, *arguments):
    binary = Path(os.environ["KPOP_DOCUMENT_TEST_BINARY"])
    with tempfile.TemporaryDirectory(prefix="kpop-document-request-") as temporary:
        root = Path(temporary)
        request, response = root / "request.json", root / "response.json"
        request.write_text(json.dumps({"op": operation, "args": [str(x) if isinstance(x, Path) else x for x in arguments]},
                                     ensure_ascii=False), encoding="utf-8")
        result = subprocess.run([str(binary), "--exact", "annotated_document::document_ui::request", "--ignored"],
                                env=dict(os.environ, KPOP_DOCUMENT_REQUEST=str(request), KPOP_DOCUMENT_RESPONSE=str(response)),
                                capture_output=True, text=True, encoding="utf-8", timeout=30)
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        payload = json.loads(response.read_text(encoding="utf-8"))
        if "error" in payload:
            raise ValueError(payload["error"])
        return payload["ok"]


def build(authored, manifest, root, at=None):
    return call("build", authored, manifest, root, at)


def refresh(data, sources, root, at=None):
    return call("refresh", data, sources, root, at)


def render(data):
    return call("render", data)


def load_artifact(source):
    return call("load_artifact", source)


def summary(data):
    return call("summary", data)


def selected_html(data):
    return call("selected_html", data)
