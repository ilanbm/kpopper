"""Followup readings from one captured core assessment, without evaluation or IO."""
import copy

from .reasoning.contract import digest, validate_value
from .reasoning.projection import project_node_status


READER_REFUSALS = frozenset({
    'unsupported_capability: use core/v1 consumer',
    'history_reader_unsupported: use explicit committed-history snapshot capture',
})


def _computation(result):
    if result is None:
        return None
    return {key: copy.deepcopy(result.get(key)) for key in
            ('status', 'value', 'basis', 'diagnostics', 'potential_dependencies')}


def _evidence(node):
    """Keep semantic evidence, excluding machine/capture identity and execution cost."""
    state = copy.deepcopy(node['state'])
    for dependency in state['basis']['dependencies'].values():
        if 'computation' in dependency:
            dependency['computation'] = _computation(dependency['computation'])
    if 'computation' in state['falsifier']:
        state['falsifier']['computation'] = _computation(state['falsifier']['computation'])
    return digest({**{key: node[key] for key in
                      ('body', 'fields', 'acceptance', 'history', 'support', 'coverage')},
                   'state': state, 'computation': _computation(node['computation'])})


def project(context):
    """Return graph's existing tuple using only a validated CapturedAssessment."""
    report = context.assessment
    values, maintenance = {}, []
    for identifier, node in sorted(report['nodes'].items()):
        findings = project_node_status(node)
        available = (node['coverage']['complete']
                     and not node['state']['integrity']['issues']
                     and node['state']['contention']['status'] == 'none_detected'
                     and node['acceptance']['status'] in ('accepted', 'not_applicable'))
        computation = node['computation']
        value = None
        if computation is not None:
            available = available and computation['status'] == 'ok'
            if computation['status'] == 'ok':
                value = validate_value(computation['value'])
        else:
            # A judgment's reading is its authored verdict, never its acceptance
            # or falsifier truth. No prose is executed to manufacture a reading.
            body = node['body']
            verdict = body.get('verdict', body.get('title')) if isinstance(body, dict) else None
            if isinstance(verdict, str):
                value = {'type': 'text', 'value': verdict}
            else:
                available = False
        values[identifier] = {'core': {
            'version': 1, 'available': available, 'value': value,
            'evidence': _evidence(node), 'findings': findings,
        }}
        reasons = sorted({reason['code'] for action in node['attention']
                          for reason in action['reasons']})
        if not available:
            reasons.append('reading_unavailable')
        if reasons or node['support']['status'] == 'reserved':
            maintenance.append({'id': identifier, 'reasons': reasons or ['support_reserved'],
                                'findings': findings, 'snapshot_id': context.snapshot_id,
                                'findings_revision': context.findings_revision})
    # Subjects with no selected computational head remain visible and unavailable.
    for identifier, subject in sorted(report['history_subjects'].items()):
        if identifier not in values:
            values[identifier] = {'core': {
                'version': 1, 'available': False, 'value': None,
                'evidence': digest(subject), 'findings': copy.deepcopy(subject),
            }}
            maintenance.append({'id': identifier, 'reasons': ['reading_unavailable'],
                                'findings': copy.deepcopy(subject),
                                'snapshot_id': context.snapshot_id,
                                'findings_revision': context.findings_revision})
    return values, maintenance, None


def capture(record):
    from .reasoning.context import CapturedAssessment
    context = CapturedAssessment.capture([record])
    data = context.snapshot.to_data()
    declaration = data['document'].get('meta', {}).get('reasoning', {})
    if declaration.get('profile') != 'core/v1' and 'history' not in data['context']:
        raise ValueError('record profile changed during followup read; retry')
    return project(context)


def envelope(value):
    """Recognize only our versioned persisted baseline, not an authored value map."""
    if not isinstance(value, dict) or set(value) != {'core'}:
        return None
    item = value['core']
    if not isinstance(item, dict) or set(item) != {
            'version', 'available', 'value', 'evidence', 'findings'}:
        return None
    if type(item['version']) is not int or item['version'] != 1 \
            or type(item['available']) is not bool \
            or not isinstance(item['evidence'], str) or len(item['evidence']) != 64 \
            or not isinstance(item['findings'], dict):
        raise ValueError('invalid core followup reading')
    if item['available']:
        validate_value(item['value'])
    return item
