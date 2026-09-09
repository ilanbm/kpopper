"""Portable source-fidelity and static-resource boundaries for authored documents."""

from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.document_html import DocumentError, parse_html, patch_html, text_content


def document(body, head="", doctype="<!doctype html>"):
    return doctype + '<html lang="en"><head>' + head + '</head><body>' + body + '</body></html>'


class DocumentHTMLTests(unittest.TestCase):
    def parse(self, body, ids=("claim",), head=""):
        return parse_html(document(body, head), ids)

    def test_exact_source_spans_unicode_and_entity_decoding(self):
        source = document('😀\r\n<p class="author">Revenue: <span DATA-KPOPPER-CLAIM="claim">€12&nbsp;&amp; 3</span>.</p>',
                          '<style>p { color: red; }</style>')
        result = parse_html(source, ["claim"])
        anchor = result["anchors"]["claim"]
        self.assertEqual(source[anchor["start"]:anchor["end"]], '€12&nbsp;&amp; 3')
        self.assertEqual(anchor["raw"], '€12&nbsp;&amp; 3')
        self.assertEqual(anchor["text"], '€12\u00a0& 3')
        self.assertEqual(anchor["tag"], "span")
        self.assertTrue(anchor["text_only"])
        self.assertEqual(anchor["context_text"], 'Revenue: €12\u00a0& 3.')
        self.assertEqual(source[anchor["context_start"]:anchor["context_end"]],
                         'Revenue: <span DATA-KPOPPER-CLAIM="claim">€12&nbsp;&amp; 3</span>.')
        self.assertEqual(source[:result["head_end"]], '<!doctype html><html lang="en"><head>')
        changed = patch_html(source, [{"start": anchor["start"], "end": anchor["end"],
                                       "before_raw": anchor["raw"], "after_raw": "€14"}])
        self.assertEqual(changed, source.replace('€12&nbsp;&amp; 3', '€14'))

    def test_author_scripts_css_comments_and_layout_are_untouched(self):
        head = "<script>const endpoint = 'https://example.invalid'; const closing = '<\\/script>';\n" \
               "function render() { return '<b>Local author code</b>'; }</script>" \
               "<style>p::before { content: 'https://example.invalid'; }</style>"
        source = document('<!-- authored -->\n<p><b data-kpopper-claim="claim">3</b></p>'
                          '<pre>&lt;/script&gt; &amp; tutorial</pre>', head)
        result = parse_html(source, ["claim"])
        self.assertEqual(patch_html(source, []), source)
        self.assertEqual(result["anchors"]["claim"]["text"], "3")
        self.assertIn("</script> & tutorial", result["coverage"]["excerpts"])

    def test_no_doctype_and_explicit_head_attributes(self):
        source = '<html><head data-theme="😀 > light"></head><body><p data-kpopper-claim="claim">ok</p></body></html>'
        result = parse_html(source, iter(["claim"]))
        self.assertEqual(source[:result["head_end"]], '<html><head data-theme="😀 > light">')

    def test_inline_markup_and_comments_are_not_text_only(self):
        for markup in ('<em>A &amp; B</em>', 'A<!-- context --> &amp; B'):
            with self.subTest(markup=markup):
                result = self.parse('<p data-kpopper-claim="claim">' + markup + '</p>')
                self.assertFalse(result["anchors"]["claim"]["text_only"])
                self.assertEqual(result["anchors"]["claim"]["text"], "A & B")

    def test_context_selects_nearest_allowed_container_or_self(self):
        for container in ("p", "li", "td", "th", "figcaption", "h1", "h2", "h3", "h4", "h5", "h6"):
            inner = '<' + container + '>Before <span data-kpopper-claim="claim">12</span> after</' + container + '>'
            if container in {"td", "th"}:
                inner = '<table><tr>' + inner + '</tr></table>'
            elif container == "li":
                inner = '<ul>' + inner + '</ul>'
            with self.subTest(container=container):
                self.assertEqual(self.parse(inner)["anchors"]["claim"]["context_text"], "Before 12 after")
        result = self.parse('<div>Outside <span data-kpopper-claim="claim">Inside</span></div>')
        self.assertEqual(result["anchors"]["claim"]["context_text"], "Inside")

    def test_newlines_follow_html5_text_normalization(self):
        result = self.parse('<pre data-kpopper-claim="claim">\r\n1\r\n2\r3</pre>')
        anchor = result["anchors"]["claim"]
        self.assertEqual(anchor["text"], "1\n2\n3")
        self.assertEqual(anchor["raw"], "\r\n1\r\n2\r3")

    def test_resource_audit_allows_inline_svg_tables_math_and_data_assets(self):
        body = '<table><col/><tr><td data-kpopper-claim="claim">5</td></tr></table>' \
               '<svg viewBox="0 0 10 10" xmlns="http://www.w3.org/2000/svg">' \
               '<defs><linearGradient id="paint"><stop offset="0"/></linearGradient></defs>' \
               '<path d="M0 0" fill="url(#paint)"/><use href="#paint"/>' \
               '<image href="data:image/png;base64,aA=="/>' \
               '<text><![CDATA[a < b]]></text></svg>' \
               '<math><mi>x</mi><mo>+</mo><mn>1</mn></math>' \
               '<img src="data:image/png;base64,aA=="><video poster="data:image/png;base64,aA=="></video>' \
               '<a href="https://example.invalid/source">Source</a>'
        result = self.parse(body, head='<style>.x { background: url("data:image/png;base64,aA=="); }' \
                                 '.y { filter: url(#paint); }</style>')
        self.assertEqual(result["anchors"]["claim"]["text"], "5")

    def test_no_manifest_claims_still_reports_unmarked_prose(self):
        result = self.parse('<p>A conclusion with no numerals.</p>', ids=[])
        self.assertEqual(result["anchors"], {})
        self.assertEqual(result["coverage"], {"anchored_claims": 0, "unmarked_blocks": 1,
                                              "unmarked_values": 0,
                                              "excerpts": ["A conclusion with no numerals."]})

    def test_coverage_excludes_anchors_and_explicitly_nonvisible_content(self):
        body = '<p data-kpopper-claim="claim">Checked 100</p>' \
               '<p>Unmarked 42 on 2026-09-10 and “a quotation”.</p>' \
               '<p>Prose <b>without</b> numbers.</p>' \
               '<p hidden>888</p><div aria-hidden="TRUE">999</div>' \
               '<p style="display: none">444</p><template><p>333</p></template>' \
               '<noscript><p>222</p></noscript><script>const v = 111;</script><style>.v { z-index: 777; }</style>'
        coverage = self.parse(body, head='<title>Title 666</title>')["coverage"]
        self.assertEqual(coverage, {"anchored_claims": 1, "unmarked_blocks": 2, "unmarked_values": 3,
                                    "excerpts": ['Unmarked 42 on 2026-09-10 and “a quotation”.',
                                                 'Prose without numbers.']})

    def test_coverage_does_not_double_count_nested_blocks(self):
        coverage = self.parse('<section><div><p>A 1.</p><p>B 2.</p></div></section>', ids=[])["coverage"]
        self.assertEqual((coverage["unmarked_blocks"], coverage["unmarked_values"]), (2, 2))
        coverage = self.parse('<p>Prefix <span data-kpopper-claim="claim">1</span> suffix.</p>')["coverage"]
        self.assertEqual(coverage["excerpts"], ["Prefix suffix."])

    def test_text_content_decodes_visible_html5_text(self):
        self.assertEqual(text_content('Hello <b>A&amp;B</b><!--hide--><script>bad</script>'
                                      '<style>bad</style><template>bad</template><noscript>bad</noscript>'
                                      '<span hidden>bad</span><i aria-hidden="true">bad</i>'
                                      '<span style="visibility:hidden">bad</span>!'), "Hello A&B!")
        self.assertEqual(text_content('<pre>\r\n😀 &lt;/script&gt;</pre>'), '😀 </script>')

    def test_incomplete_and_recovered_html_is_rejected(self):
        cases = [
            '<p data-kpopper-claim="claim">1</p>',
            '<html><body><p>1</p></body></html>',
            '<html><head></head><body><p>1</p></body>',
            document('<p data-kpopper-claim="claim">1'),
            document('<p><div data-kpopper-claim="claim">1</div></p>'),
            document('<b><i data-kpopper-claim="claim">1</b></i>'),
            document('<table><span data-kpopper-claim="claim">1</span></table>'),
            document('<span data-kpopper-claim="claim"/>'),
            document('<span data-kpopper-claim="claim" DATA-KPOPPER-CLAIM="claim">1</span>'),
            document('<span data-kpopper-claim="claim">&#0;</span>'),
            document('<span data-kpopper-claim="claim">&bogus;</span>'),
            document('<plaintext>remainder'),
            document('<p data-kpopper-claim="claim">1</p>') + '<p>outside</p>',
            '<html><head></head>outside<body><p data-kpopper-claim="claim">1</p></body></html>',
            '<?xml version="1.0"?>' + document('<p data-kpopper-claim="claim">1</p>'),
        ]
        for source in cases:
            with self.subTest(source=source), self.assertRaises(DocumentError):
                parse_html(source, ["claim"])

    def test_missing_duplicate_nested_unknown_and_bad_ids_rejected(self):
        cases = [
            ('<p>1</p>', ['claim']),
            ('<p data-kpopper-claim="claim">1</p>', []),
            ('<p data-kpopper-claim="claim">1</p><p data-kpopper-claim="claim">2</p>', ['claim']),
            ('<p data-kpopper-claim="claim"><span data-kpopper-claim="other">1</span></p>', ['claim', 'other']),
            ('<p data-kpopper-claim="1bad">1</p>', ['1bad']),
            ('<p data-kpopper-claim="bad id">1</p>', ['bad id']),
            ('<p data-kpopper-claim="claim">1</p>', ['claim', 'claim']),
            ('<p data-kpopper-claim="claim">1</p>', 'claim'),
            ('<p data-kpopper-claim="claim">1</p>', [None]),
            ('<p data-kpopper-claim="claim">1</p>', None),
            ('<img data-kpopper-claim="claim" alt="1">', ['claim']),
            ('<svg><text data-kpopper-claim="claim"/></svg>', ['claim']),
        ]
        for body, ids in cases:
            with self.subTest(body=body, ids=ids), self.assertRaises(DocumentError):
                self.parse(body, ids=ids)

    def test_stable_id_boundary(self):
        for key in ['Claim-1.value_name', 'a' * 128]:
            self.assertIn(key, self.parse('<p data-kpopper-claim="' + key + '">1</p>', ids=[key])["anchors"])
        with self.assertRaises(DocumentError):
            self.parse('<p data-kpopper-claim="' + 'a' * 129 + '">1</p>', ids=['a' * 129])

    def test_hidden_raw_text_and_template_anchors_rejected(self):
        bodies = [
            '<p hidden data-kpopper-claim="claim">1</p>',
            '<p aria-hidden="true" data-kpopper-claim="claim">1</p>',
            '<div hidden><span data-kpopper-claim="claim">1</span></div>',
            '<p style="display: NONE !important" data-kpopper-claim="claim">1</p>',
            '<p style="d\\69splay: n\\6fne" data-kpopper-claim="claim">1</p>',
            '<p style="visibility: collapse" data-kpopper-claim="claim">1</p>',
            '<p style="content-visibility: hidden" data-kpopper-claim="claim">1</p>',
            '<template><span data-kpopper-claim="claim">1</span></template>',
            '<noscript data-kpopper-claim="claim">1</noscript>',
            '<textarea data-kpopper-claim="claim">1</textarea>',
            '<script data-kpopper-claim="claim">1</script>',
            '<p data-kpopper-claim="claim">1<span hidden>2</span></p>',
            '<p data-kpopper-claim="claim">1<script>2</script></p>',
        ]
        for body in bodies:
            with self.subTest(body=body), self.assertRaises(DocumentError):
                self.parse(body)
        with self.assertRaises(DocumentError):
            self.parse('<p>1</p>', head='<title data-kpopper-claim="claim">1</title>')

    def test_ordinary_raw_text_without_claims_does_not_create_fake_anchors(self):
        result = self.parse('<textarea>&lt;span data-kpopper-claim="fake"&gt;1&lt;/span&gt;</textarea>'
                            '<p data-kpopper-claim="claim">2</p>',
                            head='<script>const markup = \'<span data-kpopper-claim="fake">3</span>\';</script>')
        self.assertEqual(list(result["anchors"]), ["claim"])

    def test_inline_visibility_uses_the_last_winning_declaration(self):
        for style in ['display:none; display:block', 'visibility:hidden; visibility:visible',
                      'display:block !important; display:none', 'display:none; display:block !important',
                      'opacity:0; opacity:1', 'opacity:1 !important; opacity:0',
                      'font-size:0px; font-size:1rem', 'font-size:1em !important; font-size:0']:
            with self.subTest(style=style):
                self.parse('<p style="' + style + '" data-kpopper-claim="claim">1</p>')
        with self.assertRaises(DocumentError):
            self.parse('<p style="display:none !important; display:block" data-kpopper-claim="claim">1</p>')

    def test_inert_and_inline_zero_visibility_reject_claims_and_exclude_text(self):
        attributes = ['inert', 'inert="false"', 'style="opacity:0"', 'style="opacity:0.0"',
                      'style="opacity:0%"', 'style="opacity:0 !important; opacity:1"',
                      'style="font-size:0"', 'style="font-size:-0.0px"', 'style="font-size:0em"',
                      'style="font-size:0rem"', 'style="font-size:0pt"', 'style="font-size:0vw"',
                      'style="font-size:0%"', 'style="font-size:0 !important; font-size:1rem"']
        for attribute in attributes:
            with self.subTest(attribute=attribute):
                for body in [
                    '<p ' + attribute + ' data-kpopper-claim="claim">1</p>',
                    '<div ' + attribute + '><span data-kpopper-claim="claim">1</span></div>',
                    '<p data-kpopper-claim="claim">1<span ' + attribute + '>2</span></p>',
                ]:
                    with self.assertRaises(DocumentError):
                        self.parse(body)
                fragment = '<p ' + attribute + '>Hidden 99</p><p>Shown 1</p>'
                coverage = self.parse(fragment, ids=[])["coverage"]
                self.assertEqual(coverage["excerpts"], ["Shown 1"])
                self.assertEqual((coverage["unmarked_blocks"], coverage["unmarked_values"]), (1, 1))
                self.assertEqual(text_content(fragment), "Shown 1")

    def test_display_contents_empty_anchors_and_revealable_contexts_are_supported(self):
        for body, expected in [
            ('<span data-kpopper-claim="claim"></span>', ""),
            ('<span style="display:contents" data-kpopper-claim="claim"></span>', ""),
            ('<div style="display:contents"><span data-kpopper-claim="claim">1</span></div>', "1"),
            ('<span style="display:contents" data-kpopper-claim="claim">1</span>', "1"),
            ('<details><summary>Reveal</summary><p data-kpopper-claim="claim">1</p></details>', "1"),
            ('<div role="tabpanel"><p data-kpopper-claim="claim">1</p></div>', "1"),
        ]:
            with self.subTest(body=body):
                anchor = self.parse(body)["anchors"]["claim"]
                self.assertEqual(anchor["text"], expected)
                self.assertTrue(anchor["text_only"])
                if not expected:
                    self.assertEqual(anchor["start"], anchor["end"])

    def test_external_html_and_svg_resources_rejected(self):
        bodies = [
            '<img src="https://example.invalid/a.png">',
            '<img src="/a.png">', '<img src="a.png">',
            '<img src="//example.invalid/a.png">',
            '<video poster="https://example.invalid/a.png"></video>',
            '<div background="https://example.invalid/a.png"></div>',
            '<script src="https://example.invalid/code.js"></script>',
            '<img src="data:image/png,x" srcset="data:image/png,x 1x">',
            '<source srcset="data:image/png,x 1x">',
            '<iframe srcdoc="&lt;p&gt;frame&lt;/p&gt;"></iframe>',
            '<object data="data:text/html,x"></object>', '<embed src="data:text/plain,x">',
            '<svg><image href="https://example.invalid/a.svg"/></svg>',
            '<svg xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="other.svg#id"/></svg>',
            '<svg><path fill="url(https://example.invalid/a.svg#id)"/></svg>',
            '<svg><path fill="u\\72l(https://example.invalid/a.svg#id)"/></svg>',
            '<svg><set attributeName="href" to="https://example.invalid/a.svg"/></svg>',
            '<svg xml:base="https://example.invalid/"><use href="#id"/></svg>',
            '<svg><animate attributeName="fill" values="red;url(https://example.invalid/a.svg)"/></svg>',
            '<link rel="preload" href="https://example.invalid/font.woff2">',
        ]
        for body in bodies:
            with self.subTest(body=body), self.assertRaises(DocumentError):
                self.parse(body, ids=[])

    def test_base_refresh_csp_and_stylesheet_links_rejected(self):
        for head in [
            '<base href="https://example.invalid/">', '<base target="_blank">',
            '<meta http-equiv=" ReFrEsH " content="0;url=https://example.invalid">',
            '<meta http-equiv="Content-Security-Policy" content="default-src *">',
            '<meta http-equiv="Content-Security-Policy-Report-Only" content="default-src *">',
            '<link rel="alternate stylesheet" href="data:text/css,p{}">',
            '<link rel="stylesheet" href="local.css">',
        ]:
            with self.subTest(head=head), self.assertRaises(DocumentError):
                self.parse('<p>1</p>', ids=[], head=head)

    def test_escaped_css_resource_references_cannot_evade_audit(self):
        rules = [
            '@import "local.css";', '@\\69mport/**/"local.css";',
            '@import url(data:text/css,p{});',
            '.x { background: url(https://example.invalid/image); }',
            '.x { background: u\\72l("https://example.invalid/image"); }',
            '.x { background: url(\\68ttps://example.invalid/image); }',
            '.x { --asset: url(https://example.invalid/image); background: var(--asset); }',
            '.x { background: image-set("https://example.invalid/image" 1x); }',
            '.x { background: -webkit-image-set("https://example.invalid/image" 1x); }',
            '.x { background: image("https://example.invalid/image"); }',
            '.x { background: src("https://example.invalid/image"); }',
            '.x { --asset: "https://example.invalid/image"; background: image-set(var(--asset) 1x); }',
            '@font-face { font-family: Local; src: url("other.woff"); }',
            '@media (min-width: 1px) { .x { mask: url(other.svg); } }',
            '.x { background: url("unterminated); }',
        ]
        for rule in rules:
            with self.subTest(rule=rule), self.assertRaises(DocumentError):
                self.parse('<p>1</p>', ids=[], head='<style>' + rule + '</style>')
        with self.assertRaises(DocumentError):
            self.parse('<p style="background:u\\72l(&quot;https://example.invalid/image&quot;)">1</p>', ids=[])

    def test_css_data_urls_fragments_and_literal_strings_are_allowed(self):
        rules = '.x { background: u\\72l("data:image/png;base64,aA=="); mask: url(#local); }' \
                '.y { background: image-set("data:image/png;base64,aA==" 1x); }' \
                '.z::after { content: "@import https://example.invalid"; }'
        self.parse('<p data-kpopper-claim="claim">1</p>', head='<style>' + rules + '</style>')

    def test_patches_are_atomic_validate_before_raw_and_leave_source_untouched_on_error(self):
        source = "😀 abc def ghi"
        edits = [{"start": 10, "end": 13, "before_raw": "ghi", "after_raw": "G"},
                 {"start": 2, "end": 5, "before_raw": "abc", "after_raw": "A"}]
        self.assertEqual(patch_html(source, edits), "😀 A def G")
        self.assertEqual(edits[0]["start"], 10)
        invalid = [
            [{"start": -1, "end": 1, "before_raw": "", "after_raw": "X"}],
            [{"start": 2, "end": 99, "before_raw": "abc", "after_raw": "X"}],
            [{"start": 5, "end": 2, "before_raw": "", "after_raw": "X"}],
            [{"start": True, "end": 2, "before_raw": " ", "after_raw": "X"}],
            [{"start": 2, "end": 5, "before_raw": "old", "after_raw": "X"}],
            [{"start": 2, "end": 5, "before_raw": "abc", "after_raw": None}],
            [{"start": 2, "end": 5, "before_raw": "abc"}],
            [edits[1], {"start": 3, "end": 4, "before_raw": "b", "after_raw": "B"}],
            [{"start": 2, "end": 2, "before_raw": "", "after_raw": "X"}, edits[1]],
            [{"start": 2, "end": 2, "before_raw": "", "after_raw": "X"}] * 2,
            [None], None,
        ]
        for batch in invalid:
            with self.subTest(batch=batch), self.assertRaises(DocumentError):
                patch_html(source, batch)
        self.assertEqual(source, "😀 abc def ghi")

    def test_adjacent_edits_and_single_empty_span_are_supported(self):
        self.assertEqual(patch_html("abc", [
            {"start": 0, "end": 1, "before_raw": "a", "after_raw": "A"},
            {"start": 1, "end": 2, "before_raw": "b", "after_raw": "B"},
        ]), "ABc")
        self.assertEqual(patch_html("😀", [
            {"start": 1, "end": 1, "before_raw": "", "after_raw": "!"},
        ]), "😀!")


if __name__ == "__main__":
    unittest.main()
