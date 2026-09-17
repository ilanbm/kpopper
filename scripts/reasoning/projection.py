"""Pure, deterministic projections of already-computed core findings.

This module is deliberately downstream of capture, evaluation and the history
adapter.  It accepts only supplied mappings: it never opens a record, resolves
a reference or evaluates an expression.  Consumers can therefore share these
renderings and impact edges without growing another truth implementation.
"""
import json
import keyword

from . import contract
from .language import lower


PROJECTION_VERSION = 'reasoning-projection/v1'

_OPERATORS = {
    'add': '+', 'sub': '-', 'mul': '*', 'div': '/',
    'eq': '==', 'ne': '!=', 'lt': '<', 'le': '<=', 'gt': '>', 'ge': '>=',
    'and': 'and', 'or': 'or',
}


def render_value(value):
    """Render a typed core value through the one generic value contract."""
    return contract.render_value(value)


def _render_reference(identifier):
    parts = identifier.split('.')
    if parts and all(part.isidentifier() and not keyword.iskeyword(part) for part in parts) \
            and identifier not in ('true', 'false', 'True', 'False', 'null', 'None'):
        return identifier
    return 'ref(' + json.dumps(identifier, ensure_ascii=False) + ')'


def render_expression(expression):
    """Render scalar or composition syntax without consulting any values.

    Authored ``expr`` text remains authored text (apart from surrounding
    whitespace); validation only proves that it belongs to the finite core
    language.  Structured ASTs use a fully explicit, deterministic spelling.
    """
    authored = isinstance(expression, dict) and set(expression) == {'expr'}
    tree = lower(expression)
    if authored:
        return expression['expr'].strip()

    def render(node):
        if 'ref' in node:
            return _render_reference(node['ref'])
        if 'num' in node:
            return node['num']
        if 'bool' in node:
            return 'true' if node['bool'] else 'false'
        if 'text' in node:
            return json.dumps(node['text'], ensure_ascii=False)
        if 'null' in node:
            return 'null'
        if 'list' in node:
            return '[' + ', '.join(render(item) for item in node['list']) + ']'
        if 'record' in node:
            return '{' + ', '.join(
                json.dumps(key, ensure_ascii=False) + ': ' + render(node['record'][key])
                for key in sorted(node['record'])) + '}'
        if 'field' in node:
            return 'field(' + render(node['field']) + ', ' + \
                json.dumps(node['key'], ensure_ascii=False) + ')'
        if 'if' in node:
            return '(' + render(node['then']) + ' if ' + render(node['if']) + \
                ' else ' + render(node['else']) + ')'
        if node['op'] == 'not':
            return '(not ' + render(node['args'][0]) + ')'
        return '(' + render(node['args'][0]) + ' ' + _OPERATORS[node['op']] + ' ' + \
            render(node['args'][1]) + ')'

    return render(tree)


def _mapping(value, name):
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError(name + ' must be a mapping')
    return value


def _status(value, default='unavailable'):
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        if isinstance(value.get('status'), str):
            return value['status']
        if type(value.get('complete')) is bool:
            return 'complete' if value['complete'] else 'incomplete'
    return default


def _dimension(node, state, name):
    """Read canonical top-level dimensions and tolerate state-nested adapters."""
    return node[name] if name in node else state.get(name)


def predicate_truth(status):
    """Project a three-valued falsifier result; unknown/error are not false."""
    if status == 'holds':
        return True
    if status == 'does_not_hold':
        return False
    return None


def computation_truth(computation):
    """Return a boolean only when a successful typed result supplies one."""
    computation = _mapping(computation, 'computation')
    value = computation.get('value')
    if computation.get('status') == 'ok' and isinstance(value, dict) \
            and value.get('type') == 'boolean' and type(value.get('value')) is bool:
        return value['value']
    return None


def _assurance_status(assurance, computation):
    assurance = _mapping(assurance, 'assurance')
    kinds = []
    actual = assurance.get('actual')
    if isinstance(actual, dict):
        items = [actual.get('node'), actual.get('falsifier')]
        dependencies = actual.get('dependencies')
        if isinstance(dependencies, dict):
            items.extend(dependencies[key] for key in sorted(dependencies))
        kinds.extend(item['kind'] for item in items
                     if isinstance(item, dict) and isinstance(item.get('kind'), str))
    elif isinstance(assurance.get('kind'), str):
        kinds.append(assurance['kind'])
    fallback = computation.get('assurance')
    if not kinds and isinstance(fallback, dict) and isinstance(fallback.get('kind'), str):
        kinds.append(fallback['kind'])
    unique = sorted(set(kinds))
    return unique[0] if len(unique) == 1 else 'mixed' if unique else 'not_available'


def project_node_status(node):
    """Project independent v3-like node dimensions into consumer-neutral data."""
    node = _mapping(node, 'node')
    state = _mapping(node.get('state'), 'node state')
    computation = _mapping(_dimension(node, state, 'computation'), 'node computation')
    falsifier = _mapping(state.get('falsifier'), 'falsifier')
    value_text = None
    if computation.get('status') == 'ok' and computation.get('value') is not None:
        value_text = render_value(computation['value'])
    assurance = _dimension(node, state, 'assurance')
    if assurance is None:
        assurance = computation.get('assurance')
    assurance = _mapping(assurance, 'assurance')
    coverage = _dimension(node, state, 'coverage')
    coverage_included = coverage.get('included') if isinstance(coverage, dict) \
        and type(coverage.get('included')) is bool else None
    evidence_kinds = assurance.get('recorded_evidence_kinds', [])
    if not isinstance(evidence_kinds, list) or any(not isinstance(item, str) for item in evidence_kinds):
        evidence_kinds = []
    result = {
        'acceptance': _status(_dimension(node, state, 'acceptance'), 'not_applicable'),
        'computation': {
            'status': computation.get('status', 'not_available'),
            'truth': computation_truth(computation),
            'value_text': value_text,
        },
        'basis': _status(state.get('basis'), 'not_available'),
        'falsifier': {
            'status': falsifier.get('status', 'not_available'),
            'holds': predicate_truth(falsifier.get('status')),
        },
        'contention': _status(state.get('contention'), 'not_available'),
        'integrity': _status(state.get('integrity'), 'not_available'),
        'coverage': _status(coverage, 'not_available'),
        'coverage_included': coverage_included,
        'assurance': _assurance_status(assurance, computation),
        'recorded_evidence_kinds': sorted(set(evidence_kinds)),
    }
    return result


def render_node_status(node):
    """Return deterministic text from the neutral status projection."""
    status = project_node_status(node)
    computation = status['computation']['status']
    if status['computation']['value_text'] is not None:
        computation += '=' + status['computation']['value_text']
    return '; '.join((
        'acceptance=' + status['acceptance'],
        'computation=' + computation,
        'basis=' + status['basis'],
        'falsifier=' + status['falsifier']['status'],
        'contention=' + status['contention'],
        'integrity=' + status['integrity'],
        'coverage=' + status['coverage'],
        'assurance=' + status['assurance'],
    ))


def _witness_ids(items, name):
    if items is None:
        return set()
    if not isinstance(items, list):
        raise ValueError(name + ' must be a list')
    result = set()
    for item in items:
        identifier = item.get('id') if isinstance(item, dict) else item if isinstance(item, str) else None
        if not isinstance(identifier, str) or not identifier:
            raise ValueError(name + ' contains an invalid witness')
        result.add(identifier)
    return result


def project_witnesses(computation):
    """Separate executed witnesses from complete, possibly-unexecuted reads."""
    computation = _mapping(computation, 'computation')
    potential = _witness_ids(computation.get('potential_dependencies'), 'potential_dependencies')
    potential.update(_witness_ids(computation.get('potential_ids'), 'potential_ids'))
    executed = _witness_ids(computation.get('executed_reads'), 'executed_reads')
    # A read that executed is necessarily potential.  The v3 adapter validates
    # that invariant; retaining it here also handles older supplied envelopes
    # that exposed only actual witnesses.
    potential.update(executed)
    rows = [{'id': identifier,
             'classification': 'executed' if identifier in executed else 'potential'}
            for identifier in sorted(potential)]
    return {
        'potential': sorted(potential),
        'executed': sorted(executed),
        'unexecuted': sorted(potential - executed),
        'dependencies': rows,
    }


def _node_computations(node):
    node = _mapping(node, 'node')
    result = []
    state = node.get('state')
    state = state if isinstance(state, dict) else {}
    computation = _dimension(node, state, 'computation')
    if isinstance(computation, dict):
        result.append(('value', computation))
    if not state:
        return result
    falsifier = state.get('falsifier')
    if isinstance(falsifier, dict) and isinstance(falsifier.get('computation'), dict):
        result.append(('falsifier', falsifier['computation']))
    basis = state.get('basis')
    dependencies = basis.get('dependencies') if isinstance(basis, dict) else None
    if isinstance(dependencies, dict):
        for identifier, finding in sorted(dependencies.items()):
            if isinstance(finding, dict) and isinstance(finding.get('computation'), dict):
                result.append(('basis:' + identifier, finding['computation']))
    return result


def project_node_impacts(node):
    """Project dependency classifications for every computation in one node."""
    by_identifier = {}
    for context, computation in _node_computations(node):
        for witness in project_witnesses(computation)['dependencies']:
            item = by_identifier.setdefault(witness['id'], [])
            item.append({'context': context, 'classification': witness['classification']})
    return [
        {'id': identifier,
         'classification': ('executed' if any(
             item['classification'] == 'executed' for item in by_identifier[identifier]) else 'potential'),
         'witnesses': sorted(by_identifier[identifier], key=lambda item: (
             item['context'], item['classification']))}
        for identifier in sorted(by_identifier)
    ]


def project_impacts(nodes):
    """Build deterministic dependency-to-consumer edges from supplied findings."""
    nodes = _mapping(nodes, 'nodes')
    edges = []
    for consumer, node in sorted(nodes.items()):
        if not isinstance(consumer, str) or not isinstance(node, dict):
            raise ValueError('nodes must map string IDs to node mappings')
        for impact in project_node_impacts(node):
            if impact['id'] == consumer:
                continue
            edges.append({'from': impact['id'], 'to': consumer,
                          'classification': impact['classification'],
                          'witnesses': impact['witnesses']})
    return edges


def project_findings(report):
    """Project canonical findings, intentionally ignoring display and attention.

    ``display_selection``, clipping metadata, ``attention`` and envelope bytes
    are consumer policy.  They cannot alter the status or impact projection.
    """
    report = _mapping(report, 'report')
    nodes = _mapping(report.get('nodes'), 'report nodes')
    projected = {}
    for identifier, node in sorted(nodes.items()):
        projected[identifier] = {
            'status': project_node_status(node),
            'status_text': render_node_status(node),
            'dependencies': project_node_impacts(node),
        }
    return {
        'version': PROJECTION_VERSION,
        'snapshot_id': report.get('snapshot_id'),
        'findings_revision': report.get('findings_revision'),
        'nodes': projected,
        'impacts': project_impacts(nodes),
    }
