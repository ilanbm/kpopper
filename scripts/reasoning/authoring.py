"""Capability-bound writer worlds; parsing and evidence, never a second evaluator."""
import copy

from .contract import (PROFILE, capabilities, CapabilityError, OutputBudget,
                       operational_bounds, validate_value, admitted)
from .evaluate import Evaluator
from .language import lower, references, literal, node_expression, required_modules
from .snapshot import Snapshot, _fields


DECLARATION = {'version': 2, 'profile': PROFILE, 'requires': ['arithmetic/v1']}


def declaration(document):
    """Validated destination requirements, unioned with retained capabilities."""
    from ..pending_grounding import entries
    meta = document.get('meta')
    # A first core judgment writes typed history before the line editor inserts
    # its declaration into the same staged document. Existing undeclared typed
    # history was already rejected when the original world was prepared; this
    # writer-only pass must still be able to declare the bytes it just created.
    declared = isinstance(meta, dict) and 'reasoning' in meta
    modules = set(capabilities(document)['requires'] if declared else ()) | {'arithmetic/v1'}
    fields = _fields(document)
    for _, body in entries(document).values():
        if isinstance(body, dict) and 'rule' in body and not isinstance(body['rule'], dict):
            expression = {}  # Retained qualitative rule text is not newly executable.
        else:
            try:
                expression = node_expression({'body': body})
            except (ValueError, TypeError, SyntaxError, RecursionError):
                # Capability discovery is not the expression validator. Invalid
                # structured syntax is still refused by World with the entry/field
                # location; do not let this earlier metadata pass leak a raw error.
                expression = {}
        modules.update(required_modules(expression))
        if isinstance(body, dict) and isinstance(body.get(fields['predicate']), dict):
            try:
                modules.update(required_modules(lower(body[fields['predicate']])))
            except (ValueError, TypeError, SyntaxError, RecursionError):
                # World is the fail-closed validator for structured predicates.
                pass
    return {**DECLARATION, 'requires': sorted(modules)}


def declare_document(document):
    result = copy.deepcopy(document)
    result.setdefault('meta', {})['reasoning'] = declaration(document)
    return result


def validate_declared(document):
    cap = capabilities(document)
    if cap['profile'] == PROFILE and set(declaration(document)['requires']) - set(cap['requires']):
        raise CapabilityError('unsupported_capability', 'composition/v1 must be declared for composed inputs')


def load(reader, paths, *, read_mode='frozen'):
    """Authorize only source collection, never a surrounding legacy computation."""
    token = reader._CORE_READS.set(True)
    try:
        return reader.load(paths, read_mode=read_mode)
    finally:
        reader._CORE_READS.reset(token)


def selected(document, requested=None):
    cap = capabilities(document)
    if requested not in (None, PROFILE):
        raise CapabilityError('unsupported_capability', 'unsupported writer profile')
    return requested == PROFILE or cap['profile'] == PROFILE


def _executables(document):
    fields = _fields(document)
    from ..pending_grounding import entries
    for nid, (_, body) in entries(document).items():
        if not isinstance(body, dict):
            continue
        for field in ('rule', fields['predicate']):
            value = body.get(field)
            if isinstance(value, dict):
                yield nid, field, value
            elif isinstance(value, str):
                try:
                    lower({'expr': value})
                except (ValueError, TypeError, SyntaxError, RecursionError):
                    continue
                yield nid, field, value


def _promotion_blockers(reader, document, context=None):
    blockers = list(_executables(document))
    from ..pending_grounding import entries
    known = entries(document)
    ids = set(known) | set(entries(context)) if context is not None else set(known)
    for nid, (_, body) in known.items():
        values = [(key, body[key]) for key in ('v', 'quoted') if key in body] if isinstance(body, dict) else [('value', body)]
        for field, value in values:
            if isinstance(value, (list, dict)):
                try:
                    literal(value)
                except (ValueError, TypeError, RecursionError):
                    blockers.append((nid, field, value))
            elif value is not None and type(value) not in (str, int, float, bool):
                blockers.append((nid, field, value))
            elif isinstance(value, str) and reader.EXPR.search(value) and any(ref in ids for ref in reader.ID.findall(value)):
                blockers.append((nid, field, value))
    return blockers


def pending_compatible(reader, paths, destination, *, snapshot=None, decisions=None, resume=(), project=None, mode=None):
    """Check the effective overlay without changing its immutable ledger or receipts.

    Accepted revisions remain attached until an explicit terminal decision retires
    them. A cached acceptance alone never authorizes reinterpreting their profile.
    Callers that mutate hold the project policy lock through their final recheck.
    """
    from pathlib import Path
    from .. import knowledge_views as V, pending_grounding as G, pending_publication as C
    project = project or V.project_for(paths)
    if not project.git or (mode or project.config()['mode']) != 'advanced' or Path(paths[0]).resolve() != project.record():
        return
    # Configured readers can be loaded by file path under a separate package
    # identity. Pass the repository location across that boundary, not a class.
    snapshot = snapshot if snapshot is not None else G.Store(project.root).snapshot()
    decisions = decisions if decisions is not None else C.Publisher(project.root)._load()['decisions']
    meaning = G.meaning_capabilities(destination)
    for revision, bundle in snapshot['bundles'].items():
        if revision not in resume and decisions.get(revision, {}).get('state') in C.TERMINAL:
            continue
        G.validate_bundle(bundle)
        if G.meaning_capabilities(bundle['manifest']['document']) != meaning:
            raise ValueError('pending_profile_reconciliation_required: ' + revision)


def prepare(reader, paths, action):
    doc = load(reader, paths)
    try:
        active = selected(doc, action.get('profile'))
        validate_declared(doc)
        aimed = doc.hypotheses.get(action.get('hypothesis'))
        if aimed is not None and capabilities(aimed['doc'])['profile'] == PROFILE:
            active = True
        if not active:
            # Legacy writers still refuse an attached core proposal.
            for hyp in doc.hypotheses.values():
                if capabilities(hyp['doc'])['profile'] == PROFILE:
                    raise CapabilityError('unsupported_capability', 'use core/v1 consumer')
            return doc, None
        if 'meta' in doc and not isinstance(doc['meta'], dict):
            raise ValueError('requires explicit migration: metadata is not a mapping')
        if capabilities(doc)['profile'] != PROFILE:
            worlds = [doc, *(hyp['doc'] for hyp in doc.hypotheses.values())]
            if any(hyp.get('error') for hyp in doc.hypotheses.values()):
                raise ValueError('requires explicit migration: unreadable hypothesis')
            if any(_promotion_blockers(reader, world, doc) for world in worlds if capabilities(world)['profile'] != PROFILE):
                raise ValueError('requires explicit migration: existing executable legacy fields')
            # The raw ledger also contains retired evidence. Only the effective
            # overlay constrains the destination's interpretation.
            destination = copy.deepcopy(doc)
            destination = declare_document(destination)
            pending_compatible(reader, paths, destination)
        for hyp in doc.hypotheses.values():
            cap = capabilities(hyp['doc'])
            if cap['profile'] != PROFILE and _promotion_blockers(reader, hyp['doc'], doc):
                raise ValueError('requires explicit migration: executable legacy hypothesis')
        original = Snapshot.capture(paths, read_mode='frozen')
        doc = copy.deepcopy(doc)
        doc = declare_document(doc)
        return doc, World(reader, doc, original=original)
    except (ValueError, TypeError, SyntaxError) as error:
        raise reader.Refused(getattr(error, 'code', 'refused') + ': ' + str(error)) from None


def authored_value(value):
    """Lossless writer-facing value; this only decodes native typed values."""
    if value['type'] == 'number':
        return int(value['numerator']) if value['denominator'] == '1' else {
            'rational': [value['numerator'], value['denominator']]}
    if value['type'] == 'list':
        return [authored_value(item) for item in value['items']]
    if value['type'] == 'record':
        return {key: authored_value(item) for key, item in value['fields'].items()}
    return value.get('value')


class Readings(dict):
    """A raw-body mapping with an explicit immutable computational world."""
    def __init__(self, world, bodies):
        super().__init__(bodies)
        self.world = world


class World:
    def __init__(self, reader, document, *, original=None, operational_limits=None):
        self.reader = reader
        self.document = copy.deepcopy(document)
        self.fields = _fields(document)
        self.bounds = operational_bounds(operational_limits)
        source = original.to_data() if original is not None else None
        self.snapshot = Snapshot.from_data(document,
            context=source['context'] if source else None,
            hypotheses=source['hypotheses'] if source else getattr(document, 'hypotheses', None),
            as_of=None)
        self.engine = Evaluator(self.snapshot, operational_limits=self.bounds)
        self.raw = Readings(self, reader.bodies(document))
        self.ids = set(self.snapshot.to_data()['nodes'])
        self._readings = {}
        self._assessment = None
        capabilities(document)
        from .assessment import _history
        for node in self.snapshot.to_data()['nodes'].values():
            body = node['body']
            seen = body.get(self.fields['snapshot']) if isinstance(body, dict) else None
            for old in seen.values() if isinstance(seen, dict) else ():
                computed = old.get('computed') if isinstance(old, dict) else None
                if isinstance(computed, dict) and 'version' in computed:
                    error = _history(old, document)[2]
                    if error:
                        raise reader.Refused(error + ': cannot write against unsupported core history')
        for nid, field, value in _executables(document):
            if isinstance(value, dict):
                try:
                    self._check_builtin(references(lower(value)))
                except (ValueError, TypeError, SyntaxError) as error:
                    raise reader.Refused(f'{nid}.{field}: {error}') from None

    def _check_builtin(self, names):
        bad = [name for name in names if self.reader.is_builtin(name)]
        if bad:
            raise self.reader.Refused('unsupported_core_builtin: ' + ', '.join(sorted(bad)))

    def result(self, nid):
        from .assessment import dependency_result
        self._check_builtin([nid])
        if nid not in self._readings:
            self._readings[nid] = dependency_result(self.snapshot, nid, self.engine)
        return copy.deepcopy(self._readings[nid])

    def require(self, result, *, blocked=False):
        if admitted(result, blocked=blocked):
            return result
        reasons = ', '.join(item['code'] for item in result['diagnostics']) or result['status']
        raise self.reader.Refused('cannot compute core/v1 evidence: ' + reasons)

    @staticmethod
    def same(left, right):
        from fractions import Fraction
        from .contract import digest
        def number(value):
            if type(value) in (int, float):
                return Fraction(str(value))
            if isinstance(value, dict) and set(value) == {'rational'}:
                parts = value['rational']
                if isinstance(parts, list) and len(parts) == 2 and all(type(part) in (str, int) for part in parts):
                    try:
                        return Fraction(int(parts[0]), int(parts[1]))
                    except (ValueError, ZeroDivisionError):
                        pass
            return None
        a, b = number(left), number(right)
        if a is not None and b is not None:
            return a == b
        return digest(left) == digest(right)

    def same_value(self, nid, candidate):
        """Compare native tagged values without guessing meaning from raw records."""
        expression = literal(candidate)
        document = copy.deepcopy(self.document)
        document.setdefault('meta', {})['reasoning'] = {
            **declaration(document), 'requires': sorted(set(declaration(document)['requires']) |
                                                       set(required_modules(expression)))}
        engine = Evaluator(Snapshot.from_data(document), runtime=self.engine.runtime,
                           operational_limits=self.bounds)
        proposed = self.require(engine.evaluate(expression, declared=[]))
        current = self.require(self.result(nid))
        return current['value'] == proposed['value']

    def value(self, nid):
        result = self.result(nid)
        if result['status'] == 'operational_error':
            self.require(result)
        if result['status'] != 'ok':
            return None
        return authored_value(result['value'])

    def predicate(self, expression, declared=None):
        if not isinstance(expression, dict):
            return None
        tree = lower(expression)
        self._check_builtin(references(tree))
        result = self.engine.evaluate(tree, declared=declared if declared is not None else references(tree))
        if result['status'] == 'operational_error':
            self.require(result)
        if result['status'] == 'ok' and result['value']['type'] == 'boolean':
            return result['value']['value']
        return None

    def candidate(self, action):
        doc = copy.deepcopy(self.document)
        from ..pending_grounding import entries
        existing = entries(doc)
        if action['kind'] == 'add':
            ids, jud, fields = self.reader.infer(doc)
            collection = existing[action['id']][0] if action['id'] in existing else self.reader._collection_for(
                doc, ids, jud, fields, action['id'], action['body'], action.get('into'))
            doc.setdefault(collection, {})[action['id']] = copy.deepcopy(action['body'])
        elif action['kind'] == 'set' and action['id'] in existing:
            collection, body = existing[action['id']]
            doc[collection][action['id']] = self.reader._peer('recording').set_body(body, action)
        return World(self.reader, declare_document(doc), original=self.snapshot, operational_limits=self.bounds)

    def normalize(self, action, *, previous_raw=None):
        if action['kind'] != 'add' or not isinstance(action.get('body'), dict):
            return action, []
        action = copy.deepcopy(action)
        body, nid = action['body'], action['id']
        predicate = self.fields['deps'] in body
        field = self.fields['predicate'] if predicate else 'rule'
        source = body.get(field)
        previous = (self.raw if previous_raw is None else previous_raw).get(nid)
        if not isinstance(source, str) or isinstance(previous, dict) and previous.get(field) == source:
            return action, []
        def keep(reason):
            return action, [f'NOTE {nid}.{field} kept as text: {reason}']
        if not predicate and any(key in body for key in ('v', 'quoted')):
            return keep('a stored reading and a calculation need distinct fields/entries')
        try:
            comparison = self.reader.CMP.match(source) if predicate else None
            try:
                tree = self.reader.E.convert_authored(source, predicate=predicate,
                    legacy_rhs=comparison.group(3) if comparison else None)
                tree = lower(tree)
            except (ValueError, TypeError, SyntaxError, RecursionError):
                tree = lower({'expr': source})
                if 'composition/v1' not in required_modules(tree):
                    raise ValueError('implicit text is outside the characterized scalar subset')
        except (ValueError, TypeError, SyntaxError, RecursionError) as error:
            return keep(str(error))
        refs = references(tree)
        self._check_builtin(refs)
        if not predicate and any(ref not in self.ids and ref != nid for ref in refs):
            return keep('unknown or ambiguous reference; use rule={expr: "..."} for an intended calculation')
        # Resolve potential coercion from authored scalar types, without executing.
        # Explicit expressions make the typed choice; implicit text stays conservative.
        def scalar_type(node, visiting=()):
            if 'ref' in node:
                ref = node['ref']
                if ref in visiting or ref not in self.raw:
                    return None
                reading = self.raw[ref]
                if isinstance(reading, dict):
                    if isinstance(reading.get('rule'), dict):
                        return scalar_type(lower(reading['rule']), (*visiting, ref))
                    reading = reading.get('v', reading.get('quoted'))
                return 'text' if isinstance(reading, str) else 'boolean' if type(reading) is bool else 'number' if type(reading) in (int, float) else None
            if 'text' in node: return 'text'
            if 'bool' in node: return 'boolean'
            if 'num' in node: return 'number'
            if node.get('op') in ('add', 'sub', 'mul', 'div'):
                kinds = [scalar_type(child, visiting) for child in node['args']]
                if any(kind in ('text', 'boolean') for kind in kinds):
                    raise ValueError('text or boolean arithmetic needs an explicit choice of typed semantics')
                return 'number'
            return None
        try:
            if predicate and tree.get('op') in ('eq', 'ne', 'lt', 'le', 'gt', 'ge'):
                kinds = [scalar_type(child) for child in tree['args']]
                if 'text' in kinds and ('number' in kinds or all('ref' in child for child in tree['args'])):
                    return keep('comparisons between text readings need an explicit choice of typed semantics')
            else:
                scalar_type(tree)
        except ValueError as error:
            return keep(str(error))
        body[field] = {'expr': source}
        return action, [f'{nid}.{field}: stored as a readable expression']

    def validate(self, action, doc, ids, jud, fields):
        P = self.reader
        raw = self.raw
        # Structural, citation, identity and permission rules are shared. Their
        # value/predicate callbacks dispatch through this explicit raw world.
        checks = [P._known_key, P._sound_dependencies, P._sound_references, P._sound_citation,
                  P._reopener_is_prose, P._request_names_the_asking,
                  P._measure_is_a_name, P._nearest_existing, P._forks_on_contradiction,
                  P._trail_is_tool_written, P._drops_are_named]
        out = []
        for check in checks:
            out.extend(check(action, doc, ids, jud, fields, raw))
        body = action.get('body', {})
        if action.get('section') or (isinstance(body, dict) and P._arrangement_shaped(body, fields, raw)) \
                or action['id'] in jud and P.is_arrangement(jud[action['id']], raw):
            out.append('unsupported_core_builtin: page arrangement/section review requires its own declared inputs')
        if isinstance(body, dict):
            deps = body.get(fields['deps'], [])
            self._check_builtin(deps if isinstance(deps, list) else [])
        stored = action['kind'] == 'set' or action['kind'] == 'add' and (
            not isinstance(body, dict) or any(key in body for key in ('v', 'quoted')))
        if stored:
            try:
                self.require(self.candidate(action).result(action['id']), blocked=bool(
                    P._blocked_text(body) if isinstance(body, dict) else False))
            except (ValueError, TypeError, SyntaxError) as error:
                out.append('value: ' + str(error))
        if action['kind'] == 'add' and isinstance(body, dict):
            if fields['deps'] in body and not P._blocked_text(body):
                predicate = body.get(fields['predicate'])
                if not isinstance(predicate, dict) and (predicate not in (None, '') or not P._reopened_text(body)):
                    out.append('condition cannot be computed under core/v1; choose explicit typed operands or declare the unavailable condition with blocked_on')
            candidate = self.candidate(action)
            for field, predicate in (('rule', False), (fields['predicate'], True)):
                expression = body.get(field)
                if not isinstance(expression, dict):
                    continue
                try:
                    tree = lower(expression)
                    refs = references(tree)
                    self._check_builtin(refs)
                    if not predicate and any(key in body for key in ('v', 'quoted')):
                        out.append('a structured rule cannot also store v or quoted')
                    result = candidate.engine.evaluate(tree if predicate else {'ref': action['id']},
                        declared=body.get(fields['deps'], []) if predicate else [action['id']])
                    self.require(result, blocked=bool(P._blocked_text(body)))
                    if predicate and result['status'] == 'ok':
                        if result['value']['type'] != 'boolean':
                            out.append('predicate must compute a boolean')
                        elif result['value']['value']:
                            out.append('wrong_if already holds - the judgment would be born broken')
                except (ValueError, TypeError, SyntaxError) as error:
                    out.append(f'{field}: {error}')
        return out

    def history(self, dep, judgments, page):
        self._check_builtin([dep])
        body = self.raw.get(dep)
        if dep in judgments:
            b = judgments[dep]['body']
            return str(b.get('verdict') or b.get('title') or dep)
        if isinstance(body, dict) and not any(key in body for key in ('v', 'quoted', 'rule', 'collection_scope')):
            when = body.get('read') or body.get('of')
            return f'read {when}' if when else self.reader.named(body) or 'present'
        if isinstance(body, dict) and isinstance(body.get('rule'), str):
            return body['rule']
        result = self.require(self.result(dep))
        history = {'version': 2, 'value': validate_value(result['value']), 'basis': result['basis']}
        if isinstance(body, dict):
            field = 'collection_scope' if 'collection_scope' in body else 'rule'
            if field in body:
                history['rule'] = copy.deepcopy(body[field])
        return {'computed': history}

    def assessment(self):
        from .assessment import assess
        if self._assessment is None:
            self._assessment = assess(self.snapshot, operational_limits=self.bounds)
            for node in self._assessment['nodes'].values():
                results = [node['computation'], node['state']['falsifier'].get('computation')]
                results += [dep['computation'] for dep in node['state']['basis']['dependencies'].values()]
                for result in results:
                    if result and result['status'] == 'operational_error':
                        self.require(result)
        return self._assessment

    def state(self, nid):
        node = self.assessment()['nodes'][nid]
        state = node['state']
        if state['falsifier']['status'] == 'holds':
            return 'FIRED', 'wrong_if holds under core/v1'
        if state['integrity']['issues']:
            return 'UNKNOWN', ', '.join(sorted({issue['code'] for issue in state['integrity']['issues']}))
        deps = state['basis']['dependencies']
        if any(dep['comparison'] == 'changed' or dep['rule_changed'] or dep['basis_comparison'] == 'changed' for dep in deps.values()):
            return 'MOVED', 'dependency value, rule or computational basis changed since review'
        if node['attention']:
            return 'REVIEW', 'core/v1 assessment requests review'
        return 'HOLDS', 'core/v1 assessment has no review finding'


def declare(lines, reader):
    """Change only the generated capability member, preserving surrounding text."""
    text = '\n'.join(lines)
    import yaml
    doc = reader.parse(text=text) or {}
    meta = doc.get('meta')
    if meta is not None and not isinstance(meta, dict):
        raise reader.Refused('requires explicit migration: metadata is not a mapping')
    desired = declaration(doc)
    if isinstance(meta, dict) and meta.get('reasoning') == desired:
        return
    encoded = yaml.safe_dump({'reasoning': desired}, sort_keys=False).rstrip().splitlines()
    root = yaml.compose(text)
    if meta is None:
        at = root.start_mark.line if root is not None else 0
        lines[at:at] = ['meta:', *['  ' + line for line in encoded], '']
        return
    # Rewrite only generated value nodes. PyYAML's end mark for a block mapping
    # or sequence includes following comments and blank lines, so replacing the
    # whole declaration would silently delete hand-authored surrounding text.
    value = next(value for key, value in root.value if key.value == 'meta')
    if 'reasoning' in meta:
        key, declaration_node = next((key, child) for key, child in value.value if key.value == 'reasoning')
        if declaration_node.start_mark.index < key.end_mark.index:
            raise reader.Refused('requires explicit migration: aliased reasoning declaration')
        members = {child_key.value: (child_key, child)
                   for child_key, child in declaration_node.value}
        edits = []
        version_key, version_node = members['version']
        if meta['reasoning']['version'] != desired['version']:
            if version_node.start_mark.index < version_key.end_mark.index:
                raise reader.Refused('requires explicit migration: aliased reasoning declaration')
            edits.append((version_node.start_mark.index, version_node.end_mark.index,
                          str(desired['version'])))
        requires_key, requires_node = members['requires']
        existing = meta['reasoning']['requires']
        if existing != desired['requires']:
            if requires_node.start_mark.index < requires_key.end_mark.index:
                raise reader.Refused('requires explicit migration: aliased reasoning declaration')
            if requires_node.flow_style:
                replacement = yaml.safe_dump(desired['requires'], default_flow_style=True,
                                             sort_keys=False, width=100000).strip()
                edits.append((requires_node.start_mark.index, requires_node.end_mark.index,
                              replacement))
            else:
                # Requirements only grow: declarations are validated before this
                # point and declaration() unions retained modules with inferred
                # ones. Insert missing block items without spanning their comments.
                missing = [item for item in desired['requires'] if item not in existing]
                indent = ' ' * requires_node.start_mark.column
                for item in missing:
                    following = next((node for current, node in zip(existing, requires_node.value)
                                      if current > item), None)
                    rendered = yaml.safe_dump(item, default_flow_style=True).splitlines()[0]
                    if following is not None:
                        at = text.rfind('\n', 0, following.start_mark.index) + 1
                        edits.append((at, at, indent + '- ' + rendered + '\n'))
                    else:
                        last = requires_node.value[-1]
                        newline = text.find('\n', last.end_mark.index)
                        if newline < 0:
                            edits.append((len(text), len(text), '\n' + indent + '- ' + rendered))
                        else:
                            at = newline + 1
                            edits.append((at, at, indent + '- ' + rendered + '\n'))
        for start, end, replacement in sorted(edits, reverse=True):
            text = text[:start] + replacement + text[end:]
        lines[:] = text.split('\n')
        return
    if value.flow_style:
        at = value.start_mark.index + 1
        item = yaml.safe_dump({'reasoning': desired}, default_flow_style=True, sort_keys=False, width=100000).strip()[1:-1]
        text = text[:at] + item + (', ' if meta else '') + text[at:]
        lines[:] = text.split('\n')
    else:
        at = value.start_mark.line
        lines[at:at] = ['  ' + line for line in encoded]
