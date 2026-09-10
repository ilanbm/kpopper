"""Validate authored documents while retaining exact, codepoint-based source spans.

This is a static authoring boundary, not a sanitizer or a JavaScript evaluator.
The HTML5 parser establishes browser structure; a separate, deliberately strict
collector establishes editable spans. No serialized DOM replaces the author's text.
"""

from dataclasses import dataclass, field
from html import unescape
from html.parser import HTMLParser
import re

import html5lib
import tinycss2


class DocumentError(ValueError):
    """An authored document cannot safely supply the requested exact spans."""


_ID = re.compile(r"[A-Za-z][A-Za-z0-9_.-]{0,127}\Z")
_VOID = frozenset("area base br col embed hr img input link meta param source track wbr".split())
_RAW = frozenset("script style title textarea xmp iframe noembed noframes plaintext".split())
_INVISIBLE = frozenset("head script style template noscript title link meta base".split())
_CONTEXT = frozenset("p li td th figcaption h1 h2 h3 h4 h5 h6".split())
_BLOCK = _CONTEXT | frozenset("body div section article aside header footer main nav blockquote pre address dt dd caption summary legend".split())
_SVG_CSS = frozenset("fill stroke filter clip-path mask cursor marker marker-start marker-mid marker-end color-profile".split())
_URL_ATTRS = frozenset(("src", "poster", "background", "href", "xlink:href"))
_LENGTH_UNITS = frozenset((
    "px cm mm q in pc pt em ex cap ch ic rem rex rcap rch ric lh rlh "
    "vw vh vi vb vmin vmax svw svh svi svb svmin svmax lvw lvh lvi lvb lvmin lvmax "
    "dvw dvh dvi dvb dvmin dvmax cqw cqh cqi cqb cqmin cqmax"
).split())
_VALUE = re.compile(
    r'"[^"\n]+"|“[^”\n]+”|「[^」\n]+」|(?<!\w)\d{4}-\d{1,2}-\d{1,2}(?!\w)'
    r'|(?<!\w)[+−-]?\d+(?:[,.]\d+)*(?:%|°)?'
)
_HTML_NS = "http://www.w3.org/1999/xhtml"


def _normalize(value):
    return value.replace("\r\n", "\n").replace("\r", "\n")


def _local(tag):
    return tag.rsplit("}", 1)[-1].lower() if isinstance(tag, str) else ""


def _attribute_name(name):
    for namespace, prefix in (("http://www.w3.org/1999/xlink", "xlink:"),
                              ("http://www.w3.org/XML/1998/namespace", "xml:"),
                              ("http://www.w3.org/2000/xmlns/", "xmlns:")):
        if name.startswith("{" + namespace + "}"):
            local = name.split("}", 1)[1]
            return ("xmlns" if prefix == "xmlns:" and local == "xmlns" else prefix + local).lower()
    return name.lower()


@dataclass(eq=False)
class _Node:
    tag: str
    attrs: dict
    parent: object = None
    start: int = 0
    end: int = 0
    children: list = field(default_factory=list)
    dom: object = None
    self_closed: bool = False


@dataclass
class _Comment:
    text: str


class _Spans(HTMLParser):
    # HTMLParser normally recognizes only script/style as raw text. These other
    # HTML5 text modes must not turn literal '<tag>' text into editable elements.
    CDATA_CONTENT_ELEMENTS = tuple((_RAW - {"plaintext"}) | {"noscript"})

    def __init__(self, source):
        super().__init__(convert_charrefs=False)
        self.source = source
        self.lines = [0] + [m.end() for m in re.finditer("\n", source)]
        self.root = _Node("document", {})
        self.stack = [self.root]
        self.nodes = []
        self.doctypes = 0

    def source_position(self):
        line, column = self.getpos()
        return self.lines[line - 1] + column

    def handle_starttag(self, tag, attrs):
        self._start(tag, attrs, False)

    def handle_startendtag(self, tag, attrs):
        self._start(tag, attrs, True)

    def _start(self, tag, attrs, closed):
        if len({key for key, _ in attrs}) != len(attrs):
            raise DocumentError("Duplicate HTML attribute on <" + tag + ">.")
        if tag == "plaintext":
            raise DocumentError("<plaintext> has no bounded HTML closing span.")
        foreign = tag in {"svg", "math"} or any(n.tag in {"svg", "math"} for n in self.stack)
        if closed and tag not in _VOID and not foreign:
            raise DocumentError("Use an explicit closing tag for <" + tag + ">.")
        start = self.source_position() + len(self.get_starttag_text())
        node = _Node(tag, {key: _normalize(value or "") for key, value in attrs},
                     self.stack[-1], start, start)
        node.self_closed = closed
        self.stack[-1].children.append(node)
        self.nodes.append(node)
        if tag not in _VOID and not closed:
            self.stack.append(node)

    def handle_endtag(self, tag):
        if len(self.stack) == 1 or self.stack[-1].tag != tag:
            raise DocumentError("HTML requires explicit, balanced closing tags; unexpected </" + tag + ">.")
        self.stack.pop().end = self.source_position()

    def handle_data(self, value):
        raw = self.stack[-1].tag
        if raw in {"title", "textarea"}:
            value = unescape(value)
        self.stack[-1].children.append(_normalize(value))

    def handle_entityref(self, name):
        self.handle_data(unescape("&" + name + ";"))

    def handle_charref(self, name):
        self.handle_data(unescape("&#" + name + ";"))

    def handle_comment(self, value):
        self.stack[-1].children.append(_Comment(value))

    def handle_decl(self, decl):
        if not decl.lower().startswith("doctype html") or len(self.stack) != 1 or self.nodes or self.doctypes:
            raise DocumentError("Only one HTML doctype before the document is supported.")
        self.doctypes += 1

    def unknown_decl(self, decl):
        if decl.startswith("CDATA[") and any(n.tag in {"svg", "math"} for n in self.stack):
            self.handle_data(decl[len("CDATA["):])
        else:
            raise DocumentError("Unsupported or ambiguous HTML declaration.")

    def handle_pi(self, data):
        raise DocumentError("Processing instructions are not supported in authored HTML.")

    def finish(self):
        self.feed(self.source)
        self.close()
        if len(self.stack) != 1:
            raise DocumentError("Missing explicit closing tag for <" + self.stack[-1].tag + ">.")
        if self.rawdata:
            raise DocumentError("Incomplete HTML token at the end of the document.")
        roots = [c for c in self.root.children if isinstance(c, _Node)]
        if len(roots) != 1 or roots[0].tag != "html":
            raise DocumentError("Author a complete document with explicit <html>, <head>, and <body> elements.")
        if any(isinstance(c, str) and c.strip() for c in self.root.children):
            raise DocumentError("Text outside the explicit <html> element is not supported.")
        children = [c for c in roots[0].children if isinstance(c, _Node)]
        if [c.tag for c in children] != ["head", "body"]:
            raise DocumentError("The explicit <html> element must contain <head> followed by <body>.")
        if any(isinstance(c, str) and c.strip() for c in roots[0].children):
            raise DocumentError("Place authored text inside the explicit <head> or <body>.")
        for node in self.nodes:
            if node.tag in {"pre", "listing", "textarea"} and node.children and isinstance(node.children[0], str):
                if node.children[0].startswith("\n"):
                    node.children[0] = node.children[0][1:]
        return roots[0], children[0], children[1]


def _plain(node, visible=False):
    if visible and _hidden(node.tag, node.attrs):
        return ""
    return "".join(_plain(c, visible) if isinstance(c, _Node) else c
                   for c in node.children if isinstance(c, (str, _Node)))


def _dom_text(node, visible=False):
    if not isinstance(node.tag, str):
        return ""
    if visible and _hidden(_local(node.tag), {_attribute_name(k): v for k, v in node.attrib.items()}):
        return ""
    return (node.text or "") + "".join(_dom_text(c, visible) + (c.tail or "") for c in node)


def _hidden(tag, attrs):
    if tag in _INVISIBLE or "hidden" in attrs or "inert" in attrs or attrs.get("aria-hidden", "").strip().lower() == "true":
        return True
    if tag == "input" and attrs.get("type", "").strip().lower() == "hidden":
        return True
    declarations = {}
    for declaration in tinycss2.parse_declaration_list(attrs.get("style", ""), skip_comments=True, skip_whitespace=True):
        if declaration.type != "declaration" or declaration.lower_name not in {
                "display", "visibility", "content-visibility", "opacity", "font-size"}:
            continue
        previous = declarations.get(declaration.lower_name)
        if previous is None or declaration.important or not previous.important:
            declarations[declaration.lower_name] = declaration
    for name, declaration in declarations.items():
        values = [t for t in declaration.value if t.type not in {"comment", "whitespace"}]
        value = values[0].value.lower() if len(values) == 1 and values[0].type == "ident" else ""
        if ((name == "display" and value == "none")
                or (name == "visibility" and value in {"hidden", "collapse"})
                or (name == "content-visibility" and value == "hidden")):
            return True
        if name in {"opacity", "font-size"} and len(values) == 1:
            token = values[0]
            numeric = token.type in {"number", "percentage"}
            length = name == "font-size" and token.type == "dimension" and token.lower_unit in _LENGTH_UNITS
            if (numeric or length) and token.value == 0:
                return True
    return False


def _cross_check(source_node, dom_node):
    if source_node.tag != _local(dom_node.tag):
        raise DocumentError("HTML5 parsing changes authored element structure near <" + source_node.tag + ">.")
    attrs = {_attribute_name(k): v for k, v in dom_node.attrib.items()}
    if source_node.attrs != attrs:
        raise DocumentError("HTML5 parsing changes attributes on <" + source_node.tag + ">.")
    source_node.dom = dom_node
    source_children = [c for c in source_node.children if isinstance(c, _Node)]
    dom_children = [c for c in dom_node if isinstance(c.tag, str)]
    # HTML5 supplies these ordinary table containers even for valid authored
    # tables. They cannot be anchors because they have no authored attributes.
    expanded = []
    position = 0
    for child in dom_children:
        tag = _local(child.tag)
        expected = source_children[position].tag if position < len(source_children) else None
        if tag in {"tbody", "colgroup"} and child.tag.startswith("{" + _HTML_NS + "}") and tag != expected and not child.attrib:
            nested = [n for n in child if isinstance(n.tag, str)]
            expanded.extend(nested)
            position += len(nested)
        else:
            expanded.append(child)
            position += 1
    if len(source_children) != len(expanded):
        raise DocumentError("HTML5 parsing changes the number of authored elements near <" + source_node.tag + ">.")
    for source_child, dom_child in zip(source_children, expanded):
        _cross_check(source_child, dom_child)


def _asset_url(value, location):
    value = value.strip()
    if not value or value.startswith("#") or value.lower().startswith("data:"):
        return
    raise DocumentError("External resource in " + location + "; use an inline or data-URI asset: " + value[:120])


def _css_tokens(tokens, location):
    for token in tokens:
        kind = token.type
        if kind == "error":
            raise DocumentError("Malformed CSS in " + location + ": " + token.message)
        if ((kind == "at-rule" and token.lower_at_keyword == "import")
                or (kind == "at-keyword" and token.value.lower() == "import")):
            raise DocumentError("CSS @import is not supported; inline the stylesheet in <style>.")
        if kind == "url":
            _asset_url(token.value, location)
        if kind == "function":
            arguments = [t for t in token.arguments if t.type not in {"whitespace", "comment"}]
            if token.lower_name in {"url", "src"}:
                if len(arguments) != 1 or arguments[0].type != "string":
                    raise DocumentError("Ambiguous CSS url() in " + location + ".")
                _asset_url(arguments[0].value, location)
            if token.lower_name in {"image", "image-set", "-webkit-image-set"}:
                for argument in arguments:
                    if argument.type == "string":
                        _asset_url(argument.value, location)
                    if argument.type == "function" and argument.lower_name in {"var", "attr", "env"}:
                        raise DocumentError("Use explicit inline/data URLs in CSS " + token.lower_name + "().")
        for field_name in ("prelude", "content", "arguments", "value"):
            children = getattr(token, field_name, None)
            if isinstance(children, list):
                _css_tokens(children, location)


def _audit(node):
    tag, attrs = node.tag, node.attrs
    if tag in {"base", "iframe", "object", "embed"}:
        raise DocumentError("<" + tag + "> is not supported in a standalone authored document.")
    if "xml:base" in attrs:
        raise DocumentError("Remove xml:base; document assets must keep their inline/data references.")
    if tag == "meta" and attrs.get("http-equiv", "").strip().lower() in {
            "refresh", "content-security-policy", "content-security-policy-report-only"}:
        raise DocumentError("Remove authored refresh/CSP meta; the document supplies its isolation policy.")
    if "srcset" in attrs or "imagesrcset" in attrs:
        raise DocumentError("Use one inline/data-URI src instead of srcset.")
    if tag == "link" and "stylesheet" in attrs.get("rel", "").lower().split():
        raise DocumentError("Inline the stylesheet in <style> instead of a stylesheet <link>.")
    for key in _URL_ATTRS & attrs.keys():
        if tag == "a" and key in {"href", "xlink:href"}:
            # Citation hyperlinks are an intentional exception to display assets.
            continue
        _asset_url(attrs[key], "<" + tag + "> " + key)
    if "style" in attrs:
        _css_tokens(tinycss2.parse_declaration_list(attrs["style"]), "<" + tag + "> style")
    if tag == "style":
        _css_tokens(tinycss2.parse_stylesheet(_plain(node)), "<style>")
    if node.dom.tag.startswith("{http://www.w3.org/2000/svg}"):
        for key in _SVG_CSS & attrs.keys():
            _css_tokens(tinycss2.parse_component_value_list(attrs[key]), "SVG " + key)
        if tag in {"animate", "set", "animatetransform", "animatemotion"}:
            target = attrs.get("attributename", "").lower()
            for key in ("from", "to", "by", "values"):
                if key in attrs and target in _URL_ATTRS:
                    for value in attrs[key].split(";"):
                        _asset_url(value, "SVG animation " + target)
                if key in attrs and target in _SVG_CSS | {"style"}:
                    _css_tokens(tinycss2.parse_component_value_list(attrs[key]), "SVG animation " + target)


def _coverage(body, count):
    blocks = {}

    def visit(node, owner):
        if _hidden(node.tag, node.attrs):
            return
        if "data-kpopper-claim" in node.attrs:
            if owner in blocks:
                blocks[owner].append(" ")
            return
        if node.tag in _BLOCK:
            owner = node
        for child in node.children:
            if isinstance(child, str):
                blocks.setdefault(owner, []).append(child)
            elif isinstance(child, _Node):
                visit(child, owner)

    visit(body, body)
    texts = [re.sub(r"\s+", " ", "".join(parts)).strip() for parts in blocks.values()]
    texts = [text for text in texts if text]
    return {"anchored_claims": count, "unmarked_blocks": len(texts),
            "unmarked_values": sum(len(_VALUE.findall(text)) for text in texts),
            "excerpts": [text[:240] for text in texts[:8]]}


def parse_html(source, claim_ids):
    """Return exact anchor/context spans after an HTML5 and static resource audit.

    Visibility here is explicit markup and inline declarations; stylesheets and
    author scripts can still change what a live frame displays. The runtime must
    check anchors again before applying a proposal and enforce its no-network CSP.
    """
    if not isinstance(source, str):
        raise DocumentError("Authored HTML must be text.")
    if isinstance(claim_ids, (str, bytes)):
        raise DocumentError("Claim IDs must be an iterable of stable identifiers.")
    try:
        requested = list(claim_ids)
    except TypeError as error:
        raise DocumentError("Claim IDs must be an iterable of stable identifiers.") from error
    if any(not isinstance(key, str) or not _ID.fullmatch(key) for key in requested):
        raise DocumentError("Claim IDs must start with a letter and use at most 128 letters, digits, dots, underscores or hyphens.")
    if len(set(requested)) != len(requested):
        raise DocumentError("Duplicate requested claim ID.")
    collector = _Spans(source)
    try:
        html, head, body = collector.finish()
    except (AssertionError, RecursionError) as error:
        raise DocumentError("Malformed or excessively nested authored HTML.") from error
    parser = html5lib.HTMLParser(tree=html5lib.getTreeBuilder("etree"))
    dom = parser.parse(source, scripting=True)
    errors = [error for error in parser.errors if not error[1].startswith("expected-doctype-but-got-")]
    if errors:
        position, reason, _ = errors[0]
        raise DocumentError("Malformed HTML at line %s, column %s: %s." % (position[0], position[1], reason))
    try:
        _cross_check(html, dom)
    except RecursionError as error:
        raise DocumentError("Authored HTML is too deeply nested.") from error
    anchors = {}
    for node in collector.nodes:
        try:
            _audit(node)
        except RecursionError as error:
            raise DocumentError("Authored CSS is too deeply nested.") from error
        key = node.attrs.get("data-kpopper-claim")
        if key is None:
            continue
        if not _ID.fullmatch(key):
            raise DocumentError("Invalid data-kpopper-claim identifier: " + repr(key))
        if key in anchors:
            raise DocumentError("Duplicate claim anchor: " + key)
        parent = node
        inside_body = False
        while parent is not None:
            inside_body = inside_body or parent is body
            if _hidden(parent.tag, parent.attrs) or parent.tag in _RAW:
                raise DocumentError("Claim " + key + " must be visible body text, outside hidden/raw-text/template elements.")
            if parent is not node and "data-kpopper-claim" in parent.attrs:
                raise DocumentError("Nested claim anchors are not supported: " + key)
            parent = parent.parent
        if not inside_body or node.tag in _VOID or node.self_closed:
            raise DocumentError("Claim " + key + " requires an explicit body element with an inner text span.")
        descendants = [node]
        while descendants:
            current = descendants.pop()
            if current is not node and (_hidden(current.tag, current.attrs) or current.tag in _RAW):
                raise DocumentError("Claim " + key + " contains hidden/raw-text/template content.")
            descendants.extend(c for c in current.children if isinstance(c, _Node))
        context = node
        parent = node
        while parent is not None and parent is not body:
            if parent.tag in _CONTEXT:
                context = parent
                break
            parent = parent.parent
        text = _plain(node)
        context_text = _plain(context, visible=True)
        if text != _dom_text(node.dom) or context_text != _dom_text(context.dom, visible=True):
            raise DocumentError("HTML5 text differs from the authored source span for claim " + key + ".")
        anchors[key] = {"start": node.start, "end": node.end,
                        "raw": source[node.start:node.end], "text": text,
                        "text_only": all(isinstance(c, str) for c in node.children), "tag": node.tag,
                        "context_start": context.start, "context_end": context.end,
                        "context_text": context_text}
    missing, unexpected = set(requested) - anchors.keys(), anchors.keys() - set(requested)
    if missing or unexpected:
        details = []
        if missing:
            details.append("missing: " + ", ".join(sorted(missing)))
        if unexpected:
            details.append("undeclared: " + ", ".join(sorted(unexpected)))
        raise DocumentError("Claim anchors must exactly match the manifest (" + "; ".join(details) + ").")
    return {"head_end": head.start, "anchors": anchors,
            "coverage": _coverage(body, len(anchors))}


def patch_html(source, edits):
    """Apply validated, non-overlapping replacements without changing other text."""
    if not isinstance(source, str):
        raise DocumentError("Authored HTML must be text.")
    if not isinstance(edits, list):
        raise DocumentError("HTML edits must be a list.")
    checked = []
    for edit in edits:
        if not isinstance(edit, dict) or not {"start", "end", "before_raw", "after_raw"} <= edit.keys():
            raise DocumentError("Each HTML edit requires start, end, before_raw and after_raw.")
        start, end = edit["start"], edit["end"]
        if type(start) is not int or type(end) is not int or not 0 <= start <= end <= len(source):
            raise DocumentError("HTML edit span is outside the source bounds.")
        if not isinstance(edit["before_raw"], str) or not isinstance(edit["after_raw"], str):
            raise DocumentError("HTML edit before_raw and after_raw must be text.")
        if source[start:end] != edit["before_raw"]:
            raise DocumentError("HTML edit no longer matches its expected before_raw.")
        checked.append(edit)
    checked.sort(key=lambda edit: (edit["start"], edit["end"]))
    for previous, current in zip(checked, checked[1:]):
        if current["start"] < previous["end"] or current["start"] == previous["start"]:
            raise DocumentError("HTML edit spans overlap or have an ambiguous shared insertion point.")
    for edit in reversed(checked):
        source = source[:edit["start"]] + edit["after_raw"] + source[edit["end"]:]
    return source


def text_content(fragment):
    """Return decoded HTML5 text, excluding explicit nonvisible fragment content."""
    if not isinstance(fragment, str):
        raise DocumentError("An HTML fragment must be text.")
    parser = html5lib.HTMLParser(tree=html5lib.getTreeBuilder("etree"))
    root = parser.parseFragment(fragment, scripting=True)
    return _dom_text(root, visible=True)
