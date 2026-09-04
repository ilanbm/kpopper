"""Element rules over rendered HTML, without a browser or authoring-format shortcuts.

Record prose is attributed to its entry; connective prose is reviewed separately. Runtime
counts and calendar markers are identified as computations, not claimed as recorded values.
"""
from html.parser import HTMLParser
import re


class Element:
    def __init__(self, tag, attrs=(), parent=None):
        self.tag, self.attrs, self.parent = tag, dict(attrs), parent
        self.children = []

    def walk(self):
        yield self
        for child in self.children:
            if isinstance(child, Element):
                yield from child.walk()

    def text(self):
        return ''.join(c.text() if isinstance(c, Element) else c for c in self.children)

    def classes(self):
        return set(self.attrs.get('class', '').split())

    def ancestors(self):
        node = self
        while node:
            yield node
            node = node.parent


class Surface(HTMLParser):
    VOID = {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'}

    def __init__(self, page):
        super().__init__(convert_charrefs=True)
        self.root = Element('document')
        self.current = self.root
        self.feed(page)

    def handle_starttag(self, tag, attrs):
        node = Element(tag, attrs, self.current)
        self.current.children.append(node)
        if tag not in self.VOID:
            self.current = node

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in self.VOID:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        for n in self.current.ancestors():
            if n.tag == tag:
                self.current = n.parent or self.root
                break

    def handle_data(self, data):
        self.current.children.append(data)


VALUE = re.compile(r'(?<![\w])[-+−]?\d+(?:[,.]\d+)*(?:%|°)?|[“「][^”」]+[”」]|"[^"\n]+"')
KEY = re.compile(r'[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+')


def lint_output(page, entries, judgments, info):
    """Return failures and warnings. Hidden tab panels are reading surfaces too."""
    tree = Surface(page).root
    ids = set(entries) | set(judgments)
    short_ids = {key for key in ids if '.' not in key}
    fail, warn = [], []
    nodes = list(tree.walk())
    lang = info.get('language', 'en')

    def say(bucket, message):
        if message not in bucket:
            bucket.append(message)

    def source(node):
        return next((p.attrs.get('data-id') or p.attrs.get('data-origin') or p.attrs.get('data-judgment')
                     for p in node.ancestors() if p.attrs.get('data-id') or p.attrs.get('data-origin')
                     or p.attrs.get('data-judgment')), None)

    for node in nodes:
        chain = list(node.ancestors())
        if any(p.tag in ('script', 'style', 'head') for p in chain):
            continue
        for child in node.children:
            if not isinstance(child, str):
                continue
            # Missing dependencies have no popover to carry their identity. Their full
            # keys are the explicit reading-surface exception: what someone must fetch.
            if any(p.classes() & {'wait', 'dead'} for p in chain):
                continue
            for key in KEY.findall(child):
                if key in ids:
                    say(fail, f'bare key on the reading surface: {key} — give it a human label')
            key = child.strip()
            if key in short_ids:
                owner = source(node)
                is_label = bool(node.classes() & {'kl', 'k'}) or owner == key
                is_bare_prose = any('txt' in p.classes() and p.text().strip() == key for p in chain)
                literal_value = owner == key and str(entries.get(key, {}).get('v')) == key
                if (is_label or is_bare_prose) and not literal_value:
                    say(fail, f'bare key on the reading surface: {key} — give it a human label')
        if 'data-id' in node.attrs:
            key = node.attrs['data-id']
            if key not in ids:
                say(fail, f'reference {key} does not resolve to an entry')
            # A recorded sentence owns its wording while its literal inline references
            # identify other entries. Allow that composition only when the record itself
            # contains the nested id; an invented second reference still fails.
            outer = [p for p in chain[1:] if p.attrs.get('data-id') in entries]
            if outer and key in entries:
                for parent in outer:
                    owner = parent.attrs['data-id']
                    recorded = entries[owner].get('v', entries[owner].get('rule', ''))
                    if not isinstance(recorded, str) or not re.search(
                            r'(?<![\w.])' + re.escape(key) + r'(?![\w.])', recorded):
                        say(fail, f'value has more than one reference: {owner}, {key}')
        if node.classes() & {'card', 'al', 'rsn'}:
            if not any(p.attrs.get('data-judgment') for p in chain):
                say(fail, 'judgment text has no judgment or review attribution')
        if node.classes() & {'big', 'kvv', 'v', 'when'} and node.text().strip():
            refs = {n.attrs['data-id'] for n in node.walk() if 'data-id' in n.attrs}
            inherited = source(node)
            if inherited:
                refs.add(inherited)
            if not refs and not node.attrs.get('data-clock'):
                say(fail, 'rendered value has no reference: ' + node.text()[:70])
        if node.attrs.get('data-judgment'):
            key = node.attrs['data-judgment']
            children = list(node.walk())
            if not any('judgment-label' in c.classes() or 'tag' in c.classes() for c in children):
                say(fail, f'{key}: judgment is not visibly marked as a judgment')
            needs_warning = info.get('flags', {}).get(key) or judgments.get(key, {}).get('unverified')
            if needs_warning and not any(c.attrs.get('data-warning') for c in children):
                say(fail, f'{key}: what is unverified is not said in place')
            state = node.attrs.get('data-review')
            if not state:
                say(fail, f'{key}: text carries no review state')
            if (state == 'moved' or 'moved' in info.get('flags', {}).get(key, ())) and not (
                    'moved' in node.classes() or any(c.classes() & {'mv', 'moved'} for c in children)):
                say(fail, f'{key}: moved since reviewed is not flagged')
        if node.attrs.get('data-prose') == 'connective':
            texts = list(node.walk())
            refs = {c.attrs['data-id'] for c in texts if c.attrs.get('data-id')}
            if not refs:
                say(warn, 'unanchored prose: connective, or an unrecorded claim — ' + node.text().strip()[:70])
            state = node.attrs.get('data-review')
            if not state:
                say(fail, 'connective text carries no review state')
            if state == 'unread':
                say(warn, 'connective text has not been reviewed against every reference')
                if not any(c.attrs.get('data-warning') for c in texts):
                    say(fail, 'unreviewed text is not flagged in place')
            if state == 'moved' and not any('moved' in c.classes() for c in texts):
                say(fail, 'connective text moved since reviewed is not flagged')
            unbound = []
            for part in texts:
                if source(part) or part.attrs.get('data-warning'):
                    continue
                # Only authored text, excluding the renderer's review and move messages.
                if any('mvd' in p.classes() or 'state' in p.classes() for p in part.ancestors()):
                    continue
                unbound.extend(x for x in part.children if isinstance(x, str))
            unbound = ' '.join(unbound)
            for match in VALUE.finditer(unbound):
                say(fail, 'connective value has no reference: ' + match.group())
            letters = [c for c in unbound if c.isalpha()]
            if len(letters) >= 20:
                he = len(re.findall(r'[\u0590-\u05ff]', unbound)) / len(letters)
                ar = len(re.findall(r'[\u0600-\u06ff]', unbound)) / len(letters)
                if (lang == 'he' and he < .3) or (lang == 'ar' and ar < .3) or (lang == 'en' and max(he, ar) > .5):
                    say(fail, 'connective prose does not follow the record language: ' + lang)
        if 'data-component' in node.attrs:
            heading = next((c for c in node.walk() if c.tag == 'h2'), None)
            title = (''.join(c if isinstance(c, str) else c.text()
                             for c in heading.children if isinstance(c, str) or 'n' not in c.classes()).strip()
                     if heading else '?')
            if node.attrs['data-component'] == 'table' and not any(c.attrs.get('data-prose') for c in node.walk()):
                say(warn, 'a dump with a heading: ' + title)
            if not node.attrs.get('data-why'):
                say(warn, 'section without why: ' + title)

    for tab in info.get('tabs', []):
        if not tab.get('serves'):
            say(warn, 'tab without serves: ' + (tab.get('title') or 'Now'))
    if lang not in ('en', 'he', 'ar'):
        say(fail, f'no chrome catalog for record language {lang}')
    if info.get('direction', 'ltr') not in ('ltr', 'rtl'):
        say(fail, 'record direction must be ltr or rtl')
    for node in nodes:
        if node.tag == 'html' and node.attrs.get('lang') != lang:
            say(fail, 'page chrome language differs from the record')
    return fail, warn
