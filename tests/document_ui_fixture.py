"""Synthetic artifacts for offline tests of the packaged document JavaScript."""
import copy
from html import escape
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts import documents as D


ATTACK = '</script><script>window.evidenceExecuted=true</script><img src=x onerror="window.evidenceExecuted=true"> $& $` $\' שלום 📚'
NOW = "2026-09-10T00:00:00+00:00"
LATER = "2026-09-11T00:00:00+00:00"


def fixtures():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)

        def write(name, value):
            (root / (name + ".json")).write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")

        def span(cid, value):
            return '<span data-kpopper-claim="' + cid + '">' + escape(value) + '</span>'

        def value(cid, source, pointer, label=None):
            return {"id": cid, "label": label or cid.title(), "kind": "value",
                    "inputs": [{"source": source, "pointer": pointer}]}

        write("counts", {"registered": 80, "finished": 60, "private": "NEVER EMBED THE UNSELECTED SECRET"})
        write("books", {"books": 240})
        write("words", {"text": "Before & after"})
        write("stable", {"value": 10})
        sources = {name: {"name": name.title(), "path": name + ".json", "format": "json"}
                   for name in ("counts", "books", "words", "stable")}
        sources["words"].update(name="Selected note " + ATTACK, representation="extraction", uri="https://example.test/citation")
        sources["missing"] = {"name": "Unavailable original", "unavailable": "The original was not supplied."}
        claims = [value("registered", "counts", "/registered", "Registered readers $& $` $'"),
                  {"id": "rate", "label": "Completion rate", "kind": "ratio", "inputs": [
                      {"source": "counts", "pointer": "/finished"}, {"source": "counts", "pointer": "/registered"}],
                   "format": {"scale": 100, "decimals": 0, "suffix": "%"}},
                  value("books", "books", "/books"), value("note", "words", "/text"),
                  value("stable", "stable", "/value"), value("missing", "missing", "/number"),
                  {"id": "interpretation", "label": "Interpretation", "kind": "inference", "inputs": [{"source": "counts"}],
                   "reason": "Participation alone does not establish satisfaction. " + ATTACK}]
        authored = ('<!doctype html><html lang="en" dir="ltr"><head><meta charset="utf-8"><title>📚 Reading</title>'
                    '<style id="author-style">body{background:#f3e8df;color:#243342}em{font-weight:700}</style>'
                    '</head><body><h1>Reading — 📚 שלום</h1><p>Of ' + span("registered", "80") + ' readers, ' +
                    span("rate", "75%") + ' finished.</p><p>Books: ' + span("books", "240") + '.</p>'
                    '<p>Note: ' + span("note", "Before & after") + '.</p><p>Stable: ' + span("stable", "10") + '.</p>'
                    '<p>Missing: ' + span("missing", "900") + '.</p><p data-kpopper-claim="interpretation">'
                    'This may help <em>build a reading habit</em>.</p><p>There are 500 unmarked books.</p>'
                    '<pre id="tutorial">' + escape('</script><script>tutorial()</script>') + '</pre>'
                    '<nav><a id="toc-chapter" href="#chapter-%D7%A9%D7%9C%D7%95%D7%9D"><em>Chapter</em></a>'
                    '<a id="toc-empty" href="#">Start</a><a id="toc-top" href="#top">Top</a>'
                    '<a id="toc-authored" href="#chapter-%D7%A9%D7%9C%D7%95%D7%9D">Author control</a>'
                    '<a id="toc-missing" href="#missing-target">Missing section</a>'
                    '<a id="toc-other" href="#chapter-%D7%A9%D7%9C%D7%95%D7%9D" target="_blank">Other view</a>'
                    '<a id="toc-external" href="https://example.test/#chapter">External citation</a></nav>'
                    '<h2 id="chapter-שלום">Chapter</h2>'
                    '<button id="author-control" type="button">Count</button>'
                    '<script id="author-script">window.authorStarted=true; window.authorCount=0;'
                    'document.getElementById("author-control").addEventListener("click",function(){window.authorCount+=1;});'
                    'document.addEventListener("click",function(event){if(event.target.id==="toc-authored"){'
                    'window.authorFragmentHandled=true;event.preventDefault();}});'
                    '</script></body></html>')
        manifest = {"version": 1, "title": "Reading 📚 <evidence>", "language": "en", "sources": sources, "claims": claims}
        initial = D.build(authored, manifest, root, NOW)
        write("counts", {"registered": 100, "finished": 60, "private": "NEVER EMBED THE UNSELECTED SECRET"})
        write("books", {"books": 300})
        write("words", {"text": ATTACK})
        refreshed = D.refresh(initial, {name: sources[name] for name in ("counts", "books", "words")}, root, LATER)
        variants = {}
        for name, ids in {"counts": {"registered"}, "books": {"books"}, "both": {"registered", "books"},
                          "attack": {"note"}, "all": {"registered", "books", "note"}}.items():
            selected = copy.deepcopy(refreshed)
            for group in selected["groups"]:
                if set(group["members"]) & ids:
                    group.update(decision="accepted", decided_at=LATER)
            variants[name] = D.selected_html(selected)
        hebrew = copy.deepcopy(refreshed)
        hebrew["language"] = "he-IL"
        zero = D.build('<!doctype html><html><head><title>One</title></head><body><p>' + span("stable", "10") + '</p></body></html>',
                       {"version": 1, "sources": {"stable": sources["stable"]}, "claims": [value("stable", "stable", "/value")]}, root, NOW)

        def layout_case(body, after="40", language="en", extra_head="", ids=("cap",)):
            write("layout", {"value": after})
            html = '<!doctype html><html><head><title>Layout</title>' + extra_head + '</head><body>' + body + '</body></html>'
            data = D.build(html, {"version": 1, "language": language,
                                 "sources": {"layout": {"name": "Layout readings", "path": "layout.json", "format": "json"}},
                                 "claims": [value(cid, "layout", "/value") for cid in ids]}, root, NOW)
            accepted = copy.deepcopy(data)
            for group in accepted["groups"]:
                group.update(decision="accepted", decided_at=LATER)
            return {"html": D.render(data), "selected": D.selected_html(accepted)}

        disclosure = '<p>Main: ' + span("main", "24") + '</p><details id="disclosure"><summary>More</summary><p>' + span("cap", "24") + '</p></details>'
        layouts = {
            "disclosure": layout_case(disclosure, ids=("main", "cap")),
            "disclosure_he": layout_case(disclosure, language="he", ids=("main", "cap")),
            "tab": layout_case('<section class="tab" id="tab"><p>' + span("cap", "24") + '</p></section>',
                               extra_head='<style>.tab:not(.active){display:none}</style>'),
            "contents": layout_case('<p><span style="display:contents" data-kpopper-claim="cap">24</span></p>'),
            "empty": layout_case('<p id="empty-parent">Selected: ' + span("cap", "") + '</p>', after="A selected value"),
            "large": layout_case('<p>' + span("cap", "Before") + '</p>', after="&" * 20000),
        }
        return {"html": D.render(refreshed), "hebrew": D.render(hebrew), "zero": D.render(zero), "authored": authored,
                "data": refreshed, "variants": variants, "attack": ATTACK, "layouts": layouts}


if __name__ == "__main__":
    if "--validate" in sys.argv:
        data = D.load_artifact(sys.stdin.read())
        print(json.dumps({"selected": D.selected_html(data), "summary": D.summary(data)}, ensure_ascii=False))
    else:
        print(json.dumps(fixtures(), ensure_ascii=False))
