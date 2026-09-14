"""A bounded, read-only projection. Text is portable; Mermaid is explicitly optional."""
import argparse
from collections import defaultdict, deque
import hashlib
import json
import re
import textwrap

try:
    from . import provenance as P
except ImportError:
    import provenance as P


MAX_EDGES = 96
FIELD_CHARS = 600
LABEL_CHARS = 80
PREMISE_ROWS = 12
CELL_CHARS = 120
RELATIONS = {
    'rests_on': 'declared premise',
    'from': 'recorded source',
    'rule_reads': 'input named by a rule',
    'refutes': 'recorded refutation, not a dependency or a newly evaluated condition',
}
STATE_MEANINGS = {
    'moved': 'changed premise needs review',
    'falsified': 'declared condition holds on recorded values',
    'blocked': 'referenced input is missing with a recorded explanation',
    'broken': 'referenced input is missing without a recorded explanation',
    'unchecked': 'a dependency has no historical snapshot',
    'no_predicate': 'no condition this reader can decide',
    'contested': 'readable hypotheses contain competing claims',
    'question': 'recorded open question',
    'unknown': 'condition cannot currently be evaluated',
}


def review_readings(judgment, raw, ids, judgments, selected):
    """A bounded display of the ordinary reader's comparisons, not a new evaluator.

    Compare against the full record, then omit unselected current values. Missing
    records remain distinguishable from present records outside the excerpt. A
    source/rule can have a reading even when the scalar reader cannot compare it.
    """
    moves = {dep: state for dep, _, _, state in P.moved_deps(judgment, raw, ids)}
    rows = []
    deps = list(dict.fromkeys(judgment['deps']))
    for dep in deps[:PREMISE_ROWS]:
        row = {'id': dep, 'has_review': dep in judgment['snap'],
               'at_review': judgment['snap'].get(dep)}
        if dep not in ids:
            row.update(current_status='missing', comparison='unavailable')
        else:
            body = raw.get(dep)
            comparable = P.value_of(raw, ids, dep)
            explicit_null = (isinstance(body, dict) and 'v' in body and body['v'] is None
                             and body.get('quoted') is None and not body.get('rule'))
            try:
                value = comparable if comparable is not None else \
                    None if explicit_null else P.snapshot_value(dep, raw, ids, judgments, {})
            except P.Refused as error:
                value = None
                row['unavailable_reason'] = str(error)
            row.update(current_status='null' if explicit_null else
                       'unavailable' if value is None else 'shown', current=value)
            rule = body.get('rule') if isinstance(body, dict) else None
            row['current_calculated'] = isinstance(rule, dict) and comparable is not None
            old = row['at_review']
            computed = old.get('computed') if isinstance(old, dict) else None
            if isinstance(computed, dict) and 'value' in computed and 'rule' in computed:
                row['review_calculated'] = computed
                row['formula_changed'] = not P.E.same(computed['rule'], rule)
            legacy_rule = P.legacy_snapshot_rule(old, rule)
            if legacy_rule is not None:
                row['historical_formula_only'] = True
                row['formula_changed'] = not P.E.same(legacy_rule, rule)
            if isinstance(body, dict) and comparable is not None and body.get('unit'):
                row['unit'] = body['unit']
            if not row['has_review']:
                row['comparison'] = 'unreviewed'
            elif dep in moves:
                row['comparison'] = moves[dep]
            elif row['current_status'] == 'unavailable':
                row['comparison'] = 'unavailable'
            elif legacy_rule is not None:
                row['comparison'] = 'formula_only'
            elif comparable is not None and isinstance(computed, dict) and computed.get('value') is not None:
                row['comparison'] = 'same'
            elif comparable is None or isinstance(comparable, (list, dict)) \
                    or isinstance(row['at_review'], (list, dict)):
                row['comparison'] = 'not_compared'
            else:
                row['comparison'] = moves.get(dep, 'same')
        if dep in ids and dep not in selected:
            row['current_status'] = 'omitted'
            for field in ('current', 'current_calculated', 'unit', 'unavailable_reason'):
                row.pop(field, None)
        rows.append(row)
    return rows, max(0, len(deps) - PREMISE_ROWS)


def project(paths, seeds, direction='support', depth=1, max_nodes=12):
    """Use the ordinary reader's roles, values and flags; never evaluate a partial record."""
    if direction not in {'support', 'impact'}:
        raise ValueError('direction must be support or impact')
    if type(depth) is not int or not 0 <= depth <= 4:
        raise ValueError('depth must be 0..4')
    if type(max_nodes) is not int or not 1 <= max_nodes <= 32:
        raise ValueError('max-nodes must be 1..32')
    seeds = list(dict.fromkeys(seeds))
    if not 1 <= len(seeds) <= min(8, max_nodes):
        raise ValueError('supply 1..8 exact IDs; max-nodes must include every seed')
    doc = P.load(paths)
    ids, judgments, fields = P.infer(doc)
    identifiers = list(ids) + [dep for judgment in judgments.values() for dep in judgment['deps']]
    identifiers += [nid for hypothesis in doc.hypotheses.values() for nid in hypothesis.get('ids', ())]
    if any(not isinstance(nid, str) for nid in identifiers):
        raise ValueError('export requires string entry and dependency IDs; quote numeric IDs in YAML')
    raw = P.with_builtins(doc, ids, judgments, fields)
    unknown = [nid for nid in seeds if nid not in ids]
    if unknown:
        raise ValueError('unknown exact ID(s): ' + ', '.join(unknown) + '; use kpopper open or pull')
    flags = P.flags(ids, judgments, fields, raw)
    disputed = P.contested(doc)
    sections = {nid: section for section, group in P.collections_of(doc).items()
                if section != 'meta' for nid in group}
    # infer also sees metadata while inferring roles; it is not a collection to export.
    ids = {nid for nid in ids if nid in sections or P.is_builtin(nid)}
    if any(nid not in ids for nid in seeds):
        raise ValueError('seeds must name record entries, not metadata')
    edges = set()
    for nid in sorted(ids):
        body = raw.get(nid)
        if not isinstance(body, dict):
            continue
        if nid in judgments:
            edges.update((nid, 'rests_on', dep) for dep in judgments[nid]['deps'])
        source = body.get('from')
        if isinstance(source, str) and source in ids:
            edges.add((nid, 'from', source))
        if nid not in judgments:
            edges.update((nid, 'rule_reads', dep) for dep in P.rule_refs(body, ids)
                         if dep != nid)
        refutes = body.get('refutes')
        if nid.startswith('hyp.') and body.get('v') == 'refuted' and isinstance(refutes, list):
            edges.update((nid, 'refutes', dep) for dep in refutes if isinstance(dep, str))
    edges = sorted(edges)
    adjacent = defaultdict(list)
    for start, relation, end in edges:
        source, target = (start, end) if direction == 'support' else (end, start)
        adjacent[source].append(target)
    distances = dict.fromkeys(seeds, 0)
    queue = deque(seeds)
    while queue:
        nid = queue.popleft()
        if distances[nid] >= depth:
            continue
        for neighbor in sorted(set(adjacent[nid])):
            if neighbor not in distances and len(distances) < max_nodes:
                distances[neighbor] = distances[nid] + 1
                queue.append(neighbor)
    nodes = {}
    for nid in distances:
        missing = nid not in ids
        body = raw.get(nid)
        states = set(flags.get(nid, ()))
        if nid in disputed:
            states.add('contested')
        if sections.get(nid) in P.OPEN:
            states.add('question')
        nodes[nid] = {'body': body, 'missing': missing, 'states': sorted(states),
                      'kind': 'missing' if missing else 'judgment' if nid in judgments
                      else 'computed' if P.is_builtin(nid) else sections.get(nid, 'entry')}
        if nid in judgments:
            if not isinstance(body.get(fields['deps']), list):
                raise ValueError(f"{nid}: {fields['deps']} must be a list of entry IDs")
            node = nodes[nid]
            node['readings'], node['omitted_readings'] = review_readings(
                judgments[nid], raw, ids, judgments, distances)
            predicate = judgments[nid]['pred']
            node['condition'] = {'expression': predicate,
                                 'result': P.evaluate(predicate, raw, ids) if predicate else None,
                                 'undeclared_reads': sorted({ref for ref in P.predicate_refs(predicate)
                                     if ref in ids and ref not in judgments[nid]['deps']})}
        elif isinstance(body, dict) and isinstance(body.get('rule'), dict):
            nodes[nid]['calculation'] = P.E.current(raw, ids, nid)
    internal = [edge for edge in edges if edge[0] in nodes and edge[2] in nodes]
    frontier = [edge for edge in edges if (edge[0] in nodes) != (edge[2] in nodes)]
    revision_data = {'record': doc, 'hypotheses': {
        name: {'doc': hyp['doc'], 'head': hyp['head'], 'error': hyp['error']}
        for name, hyp in sorted(doc.hypotheses.items())}}
    try:
        stamp = json.dumps(revision_data, ensure_ascii=False, sort_keys=True, default=str)
    except TypeError:
        # Non-ID metadata can contain mixed JSON-compatible keys. Preserve its
        # source order rather than coercing distinct keys to one identity.
        try:
            stamp = json.dumps(revision_data, ensure_ascii=False, sort_keys=False, default=str)
        except TypeError as error:
            raise ValueError('record contains mapping keys unsupported by export') from error
    return {'nodes': nodes, 'edges': internal[:MAX_EDGES], 'seeds': seeds,
            'direction': direction, 'depth': depth, 'max_nodes': max_nodes,
            'snapshot': hashlib.sha256(stamp.encode()).hexdigest()[:16],
            'outside_nodes': len(set(ids) - set(nodes)), 'frontier_edges': len(frontier),
            'omitted_edges': max(0, len(internal) - MAX_EDGES),
            'hypotheses': len(doc.hypotheses),
            'hypothesis_errors': {name: hyp['error'] for name, hyp in doc.hypotheses.items() if hyp['error']},
            'fields': fields}


def plain(value):
    """Single-line display, with literal control characters made visible."""
    if not isinstance(value, str):
        value = json.dumps(value, ensure_ascii=False, default=str)
    value = ' '.join(value.split())
    return ''.join(c if ord(c) >= 32 and ord(c) != 127 and
                   not 0x202a <= ord(c) <= 0x202e and not 0x2066 <= ord(c) <= 0x2069
                   else '\\u%04x' % ord(c) for c in value)


def markdown(value):
    # Backslash escapes stay legible in raw Markdown and prevent HTML, links and fences.
    return re.sub(r'([\\`*_{}\[\]<>()!#|&])', r'\\\1', plain(value))


def boundaries(packet):
    return (f"Record {packet['snapshot']}; {packet['direction']} depth {packet['depth']}; "
            f"{len(packet['nodes'])} nodes shown (limit {packet['max_nodes']}), "
            f"{packet['outside_nodes']} outside selection; {packet['frontier_edges']} boundary links; "
            f"{packet['omitted_edges']} internal links omitted (cap {MAX_EDGES}).")


def clipped(value, limit=FIELD_CHARS):
    text = '""' if value == '' else plain(value)
    if len(text) <= limit:
        return markdown(text)
    return (markdown(text[:limit]) +
            f' … [{len(text) - limit} characters omitted; read the record]')


def state_legend(packet):
    states = sorted({state for node in packet['nodes'].values() for state in node['states']})
    return [state.upper() + ': ' + STATE_MEANINGS[state] for state in states if state in STATE_MEANINGS]


def reading_table(node):
    lines = ['| Dependency | `at_review` (historical) | `current` (recorded or calculated) | Comparison |',
             '|---|---|---|---|']
    comparisons = {'same': 'unchanged', 'moved': 'changed; review flag',
                   'muted': 'changed; within condition; no review flag',
                   'crossed': 'changed; declared condition holds',
                   'unreviewed': 'no historical reading', 'not_compared': 'not compared by reader',
                   'unavailable': 'not compared',
                   'formula_only': 'historical formula only; no historical result'}
    for row in node['readings']:
        old = clipped(row['at_review'], CELL_CHARS) if row['has_review'] else 'not recorded'
        if 'review_calculated' in row:
            old = clipped(reading_value(row['review_calculated']['value']), CELL_CHARS) + ' (calculated)'
        status = row['current_status']
        if status == 'missing':
            current = 'missing from record'
        elif status == 'omitted':
            current = 'not included in this excerpt'
        elif status == 'null':
            current = 'null (recorded)'
        elif status == 'unavailable':
            current = 'not evaluated here'
        else:
            current = clipped(reading_value(row['current']), CELL_CHARS)
            if row.get('current_calculated'):
                current += ' (calculated)'
            if row.get('unit'):
                current += ' ' + clipped(row['unit'], 40)
        comparison = comparisons[row['comparison']]
        if row.get('formula_changed'):
            comparison += '; formula changed'
        if row.get('historical_formula_only') and row['comparison'] != 'formula_only':
            comparison += '; historical formula only; no historical result'
        lines.append(f"| {markdown(row['id'])} | {old} | {current} | {comparison} |")
    if node['omitted_readings']:
        lines.extend(['', f"{node['omitted_readings']} dependency rows omitted by the "
                      f'{PREMISE_ROWS}-row limit; read the record for the complete dependencies.'])
    return lines


def reading_value(value):
    """Display exact numeric results without reinterpreting other recorded values."""
    return P.E.display_value(value) if isinstance(value, dict) and P.E.number(value) is not None else value


def render_markdown(packet, details=False):
    lines = ['## Knowledge excerpt', '', boundaries(packet), '']
    legend = state_legend(packet)
    if legend:
        lines.extend(['; '.join(legend) + '.', ''])
    if packet['hypotheses']:
        lines.extend([f"Base record; {packet['hypotheses']} hypotheses are not expanded. "
                      'Conflict flags compare readable hypotheses, not every possible alternative.', ''])
    for name, error in packet['hypothesis_errors'].items():
        lines.extend([f'Unreadable hypothesis {markdown(name)}: {markdown(error)}', ''])
    for nid, node in packet['nodes'].items():
        status = ', '.join(state.upper() for state in node['states'])
        lines.extend([f"### {markdown(nid)} — {markdown(node['kind'])}" + (f' · {status}' if status else ''), ''])
        if node['missing']:
            lines.extend(['Referenced ID is absent from the base record.', ''])
            continue
        body = node['body']
        body = body if isinstance(body, dict) else {'v': body}
        is_judgment = node['kind'] == 'judgment'
        roles = packet['fields']
        special = {roles.get(role) for role in ('deps', 'snapshot', 'predicate')} if is_judgment else set()
        for field, value in body.items():
            if field in special:
                continue
            label = 'current' if field == 'v' else field
            if is_judgment and field in P.REOPENED:
                label += ' (human condition; not evaluated)'
            elif is_judgment and field in P.BLOCKED:
                label += ' (recorded declaration)'
            shown = P.predicate_text(value) if field == 'rule' and isinstance(value, dict) else value
            lines.append(f'- {markdown(label)}: {clipped(shown)}')
        if 'calculation' in node:
            calculation = node['calculation']
            if calculation['value'] is None:
                lines.append('- calculated current: unavailable — ' + clipped(calculation['reason']))
            else:
                lines.append('- calculated current: ' + clipped(reading_value(calculation['value'])))
        if is_judgment:
            condition = node['condition']
            if condition['expression']:
                result = condition['result']
                outcome = ('true' if result else 'false') + ' on current recorded values' \
                    if result is not None else 'not evaluated: missing or unsupported reading/condition'
                lines.append(f"- {markdown(roles.get('predicate') or 'wrong_if')}: "
                             f"{clipped(P.predicate_text(condition['expression']))} → {outcome}.")
            else:
                lines.append('- No executable condition declared.')
            if condition['undeclared_reads']:
                lines.append('- Condition reads undeclared dependencies: ' +
                             clipped(', '.join(condition['undeclared_reads'])) +
                             '. Changes to these inputs do not follow the declared dependency links.')
            lines.extend([''] + reading_table(node))
        if details:
            lines.extend(['', 'Recorded fields:'])
            for field, value in body.items():
                label = f'historical snapshot ({field})' if is_judgment and field == roles.get('snapshot') else field
                lines.append(f'- {markdown(label)}: {clipped(value)}')
        lines.append('')
    lines.extend(['### Recorded links', '', 'Left names right; impact traversal keeps this direction.', ''])
    for start, relation, end in packet['edges']:
        lines.append(f'- {markdown(start)} → {relation} → {markdown(end)} ({RELATIONS[relation]}).')
    if not packet['edges']:
        lines.append('No links shown within this selection.')
    lines.extend(['', 'Conditions use the full base record; current readings outside this excerpt are marked. '
                  'External source contents and page-only checks are not evaluated here. '
                  'Read an entry with kpopper pull ID; kpopper export ID --details shows recorded fields. '
                  'The source YAML remains authoritative.', ''])
    return '\n'.join(lines)


def mermaid_text(value):
    # Mermaid's numeric entities protect grammar and HTML. Escape the original # first
    # (character by character), so a record cannot smuggle its own entity or directive.
    return ''.join(c if c.isalnum() or c in ' .,_-:/' else f'#{ord(c)};' for c in plain(value))


def render_mermaid(packet):
    lines = ['flowchart TB', '  accTitle: Knowledge excerpt',
             '  accDescr: Declared links only. MOVED needs review; FALSIFIED means a declared condition is true.',
             '  %% ' + boundaries(packet)]
    aliases = {nid: f'n{i}' for i, nid in enumerate(packet['nodes'])}
    clipped = 0
    for nid, node in packet['nodes'].items():
        body = node['body']
        body = body if isinstance(body, dict) else {'v': body}
        label = 'Referenced ID is absent from the base record' if node['missing'] else plain(body.get('verdict') or P.named(body) or
                      (body['v'] if 'v' in body else node['kind']))
        if not node['missing'] and 'v' in body and not (P.named(body) or body.get('verdict')) \
                and body.get('unit'):
            label += ' ' + plain(body['unit'])
        shortened = len(label) > LABEL_CHARS
        if len(label) > LABEL_CHARS:
            label = label[:LABEL_CHARS] + '…'
        status = 'MISSING' if node['missing'] else ', '.join(state.upper() for state in node['states'])
        # IDs are never shortened. A break separates LTR identity from multilingual prose.
        parts = [nid, node['kind'] + (': ' + status if status else '')]
        if not node['missing'] and 'v' in body and (P.named(body) or body.get('verdict')):
            value = plain(body['v']) + (' ' + plain(body['unit']) if body.get('unit') else '')
            shortened |= len(value) > LABEL_CHARS
            parts += textwrap.wrap('v: ' + value[:LABEL_CHARS] + ('…' if len(value) > LABEL_CHARS else ''), 36)
        clipped += int(shortened)
        parts += textwrap.wrap(label, 36)
        text = '<br/>'.join(mermaid_text(part) for part in parts)
        lines.append(f'  {aliases[nid]}["{text}"]')
    for start, relation, end in packet['edges']:
        arrow = '-.->' if relation == 'refutes' else '-->'
        lines.append(f'  {aliases[start]} {arrow}|{relation}| {aliases[end]}')
    legend = state_legend(packet)
    if legend:
        text = '<br/>'.join(mermaid_text(line) for line in legend)
        lines.append(f'  export_legend["{text}"]')
    # Visible boundaries survive a standalone .mmd -> SVG conversion, not just comments.
    note = ['Scope (not a record)', packet['snapshot'],
            f"{packet['outside_nodes']} nodes outside selection",
            f"{packet['frontier_edges']} boundary links",
            f"{packet['omitted_edges']} internal links omitted",
            f'{clipped} labels shortened ({LABEL_CHARS} chars)',
            f"{packet['hypotheses']} hypotheses not expanded",
            f"{len(packet['hypothesis_errors'])} unreadable hypotheses",
            'Details: read the text export']
    text = '<br/>'.join(mermaid_text(part) for part in note)
    lines.extend([f'  export_note["{text}"]', '  style export_note stroke-dasharray: 4 4', ''])
    return '\n'.join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(prog='kpopper export', description=__doc__)
    parser.add_argument('ids', nargs='+', help='1..8 exact record IDs; no whole-record default')
    parser.add_argument('--record', action='append', help='record path; repeat for multiple inputs')
    parser.add_argument('--format', choices=('markdown', 'markdown-mermaid', 'mermaid'),
                        default='markdown', help='markdown is readable without scripts or images (default)')
    parser.add_argument('--direction', choices=('support', 'impact'), default='support',
                        help='follow declared links, or reverse traversal; arrows always retain their meaning')
    parser.add_argument('--depth', type=int, default=1, help='0..4 hops, default 1')
    parser.add_argument('--max-nodes', type=int, default=12, help='1..32 nodes including seeds, default 12')
    parser.add_argument('--details', action='store_true', help='include original recorded fields in text output')
    args = parser.parse_args(argv)
    try:
        packet = project(args.record or P.default_paths(), args.ids, args.direction, args.depth, args.max_nodes)
        if args.details and args.format == 'mermaid':
            raise ValueError('--details needs a text format: markdown or markdown-mermaid')
        output = render_mermaid(packet) if args.format == 'mermaid' else render_markdown(packet, args.details)
        if args.format == 'markdown-mermaid':
            output += ('\n### Optional Mermaid diagram\n\n'
                       'Requires a Mermaid renderer; otherwise this is a code block. '
                       'The text above is the readable representation.\n\n```mermaid\n' +
                       render_mermaid(packet) + '```\n')
    except (ValueError, OSError, P.yaml.YAMLError) as error:
        parser.error(str(error))
    print(output, end='')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
