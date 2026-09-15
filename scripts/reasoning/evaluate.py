"""One public evaluation result over one immutable, explicit snapshot."""
import copy
import subprocess

from .contract import (PROFILE, CapabilityError, capabilities, digest,
                       resource_limits)
from .language import closure, lower, references
from .runtime import Runtime, RuntimeUnavailable
from .snapshot import SnapshotView
from .modules import module


def _diagnostic(code, related=()):
    return {'code': code, 'related_ids': sorted(set(related))}


class Evaluator:
    """Memoize within a captured snapshot; never reuse observations across snapshots."""
    def __init__(self, snapshot, *, runtime=None, limits=None):
        self.snapshot = snapshot
        self.data = snapshot.to_data()
        self.limits = resource_limits(limits)
        self.runtime = runtime
        self._basis = {}

    def basis(self, nid):
        if nid not in self._basis:
            self._basis[nid] = self.snapshot._input_basis().basis(nid)
        return copy.deepcopy(self._basis[nid])

    def prepare(self, expression, declared):
        if not isinstance(declared, list) or any(not isinstance(nid, str) for nid in declared):
            raise ValueError('declared dependencies must be exact string IDs')
        cap = capabilities(self.data['document'], profile=PROFILE)
        tree = lower(expression)
        direct = references(tree)
        undeclared = set(direct) - set(declared)
        component = closure(self.data, direct)
        witnesses = self.snapshot._input_basis().dependencies(direct)
        potential_ids = [item['id'] for item in witnesses]
        basis = {'version': 1, 'recipe': 'merkle-inputs/v1', 'profile': PROFILE, 'modules': cap['requires'],
                 'as_of': self.data.get('as_of'),
                 'expression': tree, 'dependencies': witnesses}
        basis['digest'] = digest(basis)
        identity = {'snapshot_id': self.snapshot.snapshot_id, 'expression': tree,
                    'profile': PROFILE, 'modules': cap['requires'], 'declared': sorted(set(declared)),
                    'as_of': self.data.get('as_of'), 'resources': {'version': 'resources/v1', **self.limits}}
        result = {'schema_version': 1, 'profile': PROFILE, 'modules': cap['requires'],
                  'snapshot_id': self.snapshot.snapshot_id, 'computation_id': digest(identity),
                  'implementation': None, 'resource_profile': {'version': 'resources/v1', **self.limits},
                  'status': 'unknown', 'value': None, 'diagnostics': [], 'executed_reads': [],
                  'potential_dependencies': witnesses, 'potential_ids': potential_ids,
                  'basis': basis, 'assurance': {'kind': 'computed', 'formal_scope': []},
                  'cost': {'steps': 0, 'node_evaluations': {}}}
        if cap.get('experimental_override'):
            result['interpretation'] = {'declared_profile': cap['declared_profile'], 'explicit_override': PROFILE}
        if undeclared:
            result.update(status='error', diagnostics=[_diagnostic('undeclared_dependency', undeclared)])
            return None, result
        if component['errors']:
            result.update(status='error', diagnostics=[_diagnostic(code, [nid])
                          for nid, code in sorted(component['errors'].items())])
            return None, result
        view = SnapshotView(self.snapshot, nodes=potential_ids)
        request = module('arithmetic/v1').prepare(view, tree, component['nodes'], potential_ids, self.limits)
        return request, result

    def evaluate_many(self, requests):
        prepared = []
        for expression, declared in requests:
            try:
                prepared.append(self.prepare(expression, declared))
            except (ValueError, TypeError, RecursionError, SyntaxError) as error:
                code = error.code if isinstance(error, CapabilityError) else 'invalid_expression'
                prepared.append((None, {
                    'schema_version': 1, 'profile': PROFILE, 'modules': [],
                    'snapshot_id': self.snapshot.snapshot_id,
                    'computation_id': None,
                    'status': 'unsupported_capability' if code == 'unsupported_capability' else 'error',
                    'value': None, 'diagnostics': [_diagnostic(code)],
                    'executed_reads': [], 'potential_dependencies': [], 'potential_ids': [],
                    'basis': None, 'implementation': None,
                    'resource_profile': {'version': 'resources/v1', **self.limits},
                    'assurance': {'kind': 'computed', 'formal_scope': []},
                    'cost': {'steps': 0, 'node_evaluations': {}}}))
        active = [(request, result) for request, result in prepared if request is not None]
        if active:
            try:
                if self.runtime is None:
                    self.runtime = Runtime()
                responses = self.runtime.request_many([item[0] for item in active])
                if len(responses) != len(active):
                    raise RuntimeUnavailable('native response count mismatch')
                for (request, result), response in zip(active, responses):
                    if not set(response['executed_reads']) <= set(request['declared']) \
                            or not set(response['potential_reads']) <= set(result['potential_ids']):
                        raise RuntimeUnavailable('native dependency witness disagrees with captured closure')
                    result.update(status=response['status'], value=response['value'],
                                  diagnostics=[_diagnostic(code) for code in response['diagnostics']],
                                  executed_reads=[{'kind': 'node', 'id': nid} for nid in response['executed_reads']],
                                  cost={'steps': response['steps'], 'node_evaluations': response['node_evaluations']},
                                  implementation=copy.deepcopy(self.runtime.implementation))
                    result['assurance']['implementation'] = digest(self.runtime.implementation)
                    pending = [request['expression']]
                    closed_rational = True
                    while pending:
                        expr = pending.pop()
                        if 'num' not in expr and expr.get('op') not in ('add', 'sub', 'mul', 'div'):
                            closed_rational = False
                            break
                        pending.extend(expr.get('args', []))
                    if closed_rational and result['status'] == 'ok':
                        result['assurance']['formal_scope'] = ['Kpopper.Proof.evaluate_closedRat_sound']
            except (ValueError, OSError, subprocess.SubprocessError, KeyError, TypeError) as error:
                for _, result in active:
                    result.update(status='operational_error', value=None,
                                  diagnostics=[_diagnostic('runtime_unavailable')])
        return [result for _, result in prepared]

    def evaluate(self, expression, *, declared):
        return self.evaluate_many([(expression, declared)])[0]


def evaluate(snapshot, expression, *, declared, limits=None, runtime=None):
    return Evaluator(snapshot, runtime=runtime, limits=limits).evaluate(expression, declared=declared)


def compare_basis(current, historical):
    if historical is None:
        return 'not_recorded'
    if not isinstance(current, dict) or not isinstance(historical, dict):
        return 'unavailable'
    for basis in (current, historical):
        if basis.get('version') != 1 or basis.get('profile') != PROFILE \
                or not isinstance(basis.get('digest'), str) \
                or digest({key: value for key, value in basis.items() if key != 'digest'}) != basis['digest']:
            return 'unavailable'
    return 'same' if current['digest'] == historical['digest'] else 'changed'
