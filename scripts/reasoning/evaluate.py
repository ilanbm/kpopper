"""One public evaluation result over one immutable, explicit snapshot."""
import copy
import subprocess

from .contract import (PROFILE, CapabilityError, capabilities, digest,
                       resource_limits, operational_bounds, OperationalLimit, OutputBudget,
                       COMPOSITION_LIMITS)
from .language import closure, lower, references, required_modules
from .runtime import Runtime, RuntimeUnavailable
from .snapshot import SnapshotError, SnapshotView
from .modules import module


def _diagnostic(code, related=()):
    return {'code': code, 'related_ids': sorted(set(related))}


def _query_diagnostic(code, related=(), locations=()):
    return {'code': code, 'related_ids': sorted(set(related)),
            'locations': sorted(locations,
                                key=lambda item: (item['candidate'], item['column'], item['phase']))}


class Evaluator:
    """Memoize within a captured snapshot; never reuse observations across snapshots."""
    def __init__(self, snapshot, *, runtime=None, limits=None, operational_limits=None):
        self.snapshot = snapshot
        self.data = snapshot.to_data()
        self._requested_limits = {} if limits is None else copy.deepcopy(limits)
        if not isinstance(self._requested_limits, dict):
            raise ValueError('resource limits must be a mapping')
        self.limits = resource_limits({key: value for key, value in self._requested_limits.items()
                                       if key in ('steps', 'depth', 'digits')})
        self.operational_limits = operational_bounds(operational_limits)
        if isinstance(runtime, Runtime):
            self.operational_limits = {key: min(value, runtime.operational_limits[key])
                                       for key, value in self.operational_limits.items()}
        self.runtime = runtime
        self._basis = {}

    def basis(self, nid):
        if nid not in self._basis:
            self._basis[nid] = self.snapshot._input_basis().basis(nid)
        return copy.deepcopy(self._basis[nid])

    @staticmethod
    def _query_cost(*, candidates=0, field_reads=0, preflight_steps=0,
                    steps=0, node_evaluations=None, evaluated_field_reads=0):
        return {'steps': steps, 'preflight_steps': preflight_steps,
                'node_evaluations': {} if node_evaluations is None else dict(node_evaluations),
                'candidates': candidates, 'field_reads': field_reads,
                'evaluated_field_reads': evaluated_field_reads}

    def _query_result(self, resources, *, modules=None):
        return {'schema_version': 2, 'profile': PROFILE,
                'modules': sorted(set(modules or ('query/v1',))),
                'snapshot_id': self.snapshot.snapshot_id, 'computation_id': None,
                'implementation': None, 'resource_profile': copy.deepcopy(resources),
                'operational_limits': dict(self.operational_limits),
                'status': 'unknown', 'value': None, 'diagnostics': [],
                'executed_reads': [], 'potential_dependencies': [], 'potential_ids': [],
                'basis': None, 'query_counts': None,
                'assurance': {'kind': 'computed', 'formal_scope': []},
                'cost': self._query_cost()}

    def _prepare_query(self, authored, declared, *, root_id=None):
        """Bind one direct or stored query to a complete immutable scope capture."""
        from . import query

        try:
            resources = query.resources_v4(self._requested_limits)
        except (ValueError, TypeError, RecursionError):
            resources = query.resources_v4()
            result = self._query_result(resources)
            result.update(status='error', diagnostics=[_query_diagnostic('invalid_limits')])
            return None, result
        result = self._query_result(resources)
        cap = capabilities(self.data['document'], profile=PROFILE)
        if cap.get('experimental_override'):
            result['interpretation'] = {'declared_profile': cap['declared_profile'],
                                        'explicit_override': PROFILE}

        query_body = authored.get('query') if isinstance(authored, dict) else None
        scope_id = query_body.get('scope') if isinstance(query_body, dict) else None
        root_witness = None
        required_declared = set()
        if root_id is None:
            if isinstance(scope_id, str):
                required_declared.add(scope_id)
        else:
            required_declared.add(root_id)
            summary = self.snapshot._input_basis().summary(root_id)
            root_witness = {'kind': 'node', 'id': root_id,
                            'fingerprint': summary['fingerprint']}
            result.update(potential_dependencies=[copy.deepcopy(root_witness)],
                          potential_ids=[root_id])
            if summary['status'] != 'available':
                result.update(status='error',
                              diagnostics=[_query_diagnostic(code, [root_id])
                                           for code in summary['diagnostics']])
                return None, result

        undeclared = required_declared - set(declared)
        if undeclared:
            result.update(status='error',
                          diagnostics=[_query_diagnostic('undeclared_dependency', undeclared)])
            return None, result
        if not isinstance(scope_id, str) or not scope_id:
            result.update(status='error',
                          diagnostics=[_query_diagnostic('invalid_expression')])
            return None, result

        try:
            capture = self.snapshot.capture_query_scope(scope_id)
            captured_dependencies = ([] if root_witness is None else [copy.deepcopy(root_witness)]) \
                + [capture.witness]
            result.update(potential_dependencies=captured_dependencies,
                          potential_ids=sorted(([] if root_id is None else [root_id]) + [scope_id]))
            capture_data = capture.to_data()
            normalized = query.lower(authored, capture_data['definition']['fields'])
            closure_modules = query.required_modules(normalized)
            result['modules'] = closure_modules
            missing_modules = set(closure_modules) - set(cap['requires'])
            if missing_modules:
                raise CapabilityError('unsupported_capability',
                                      'undeclared modules: ' + ', '.join(sorted(missing_modules)))
            request_id = digest({
                'version': 1, 'snapshot_id': self.snapshot.snapshot_id,
                'root_witness': root_witness, 'scope_witness': capture.witness,
                'operation': normalized, 'resources': resources,
            })
            prepared = module('query/v1').prepare(
                capture, authored, request_id=request_id, root_witness=root_witness,
                declared_capabilities=cap['requires'], limits=resources)
            prepared = module('query/v1').validate(prepared)
            result.update(modules=prepared['required_modules'],
                          resource_profile=copy.deepcopy(prepared['request']['resources']),
                          potential_dependencies=copy.deepcopy(prepared['potential_dependencies']),
                          potential_ids=sorted(prepared['potential_ids']))
            preflight = prepared['basis_template']['preflight_cost']
            result['cost'] = self._query_cost(
                candidates=preflight['candidates'], field_reads=preflight['field_reads'],
                preflight_steps=preflight['preflight_steps'],
                node_evaluations={} if root_id is None else {root_id: 1})
            return prepared, result
        except CapabilityError as error:
            result.update(status='unsupported_capability',
                          diagnostics=[_query_diagnostic(error.code, [scope_id])])
        except SnapshotError as error:
            code = error.code
            if code == 'limit':
                detail = str(error)
                code = 'field_read_limit' if 'field' in detail else 'candidate_limit'
            result.update(status='limit' if code.endswith('_limit') else 'error',
                          diagnostics=[_query_diagnostic(code, [scope_id])])
        except (ValueError, TypeError, RecursionError, SyntaxError) as error:
            detail = str(error)
            if detail == 'transport_limit':
                raise OperationalLimit('batch_input_limit') from None
            semantic_limits = {'candidate_limit', 'field_read_limit', 'depth_limit',
                               'value_limit', 'digit_limit', 'step_limit'}
            code = detail if detail in semantic_limits else (
                'unsupported_capability' if detail.startswith('undeclared modules:')
                else 'invalid_expression')
            result.update(status='limit' if code in semantic_limits else
                          'unsupported_capability' if code == 'unsupported_capability' else 'error',
                          diagnostics=[_query_diagnostic(code, [scope_id])])
        return None, result

    def prepare(self, expression, declared):
        if not isinstance(declared, list) or any(not isinstance(nid, str) for nid in declared):
            raise ValueError('declared dependencies must be exact string IDs')
        if isinstance(expression, dict) and set(expression) == {'query'}:
            return self._prepare_query(expression, declared)
        if isinstance(expression, dict) and set(expression) == {'ref'} \
                and isinstance(expression['ref'], str):
            node = self.data['nodes'].get(expression['ref'])
            body = node.get('body') if isinstance(node, dict) else None
            rule = body.get('rule') if isinstance(body, dict) else None
            if isinstance(rule, dict) and set(rule) == {'query'}:
                return self._prepare_query(rule, declared, root_id=expression['ref'])
        if set(self._requested_limits) - {'steps', 'depth', 'digits'}:
            raise ValueError('unknown resource limits')
        cap = capabilities(self.data['document'], profile=PROFILE)
        tree = lower(expression)
        direct = references(tree)
        undeclared = set(direct) - set(declared)
        component = closure(self.data, direct)
        modules = sorted(set(required_modules(tree, *component['nodes'].values())) |
                         set(self.snapshot._input_basis().modules(direct)))
        if set(modules) - set(cap['requires']):
            raise CapabilityError('unsupported_capability', 'undeclared modules: ' + ', '.join(sorted(set(modules) - set(cap['requires']))))
        composed = 'composition/v1' in modules
        limits = {**self.limits, **(COMPOSITION_LIMITS if composed else {})}
        resources = {'version': 'resources/v3' if composed else 'resources/v2', **limits}
        witnesses = self.snapshot._input_basis().dependencies(direct)
        potential_ids = [item['id'] for item in witnesses]
        basis = {'version': 1, 'recipe': 'merkle-inputs/v1', 'profile': PROFILE, 'modules': modules,
                 'as_of': self.data.get('as_of'),
                 'expression': tree, 'dependencies': witnesses}
        basis['digest'] = digest(basis)
        identity = {'snapshot_id': self.snapshot.snapshot_id, 'expression': tree,
                    'profile': PROFILE, 'modules': modules, 'declared': sorted(set(declared)),
                    'as_of': self.data.get('as_of'), 'resources': resources}
        result = {'schema_version': 1, 'profile': PROFILE, 'modules': modules,
                  'snapshot_id': self.snapshot.snapshot_id, 'computation_id': digest(identity),
                  'implementation': None, 'resource_profile': resources,
                  'operational_limits': dict(self.operational_limits),
                  'status': 'unknown', 'value': None, 'diagnostics': [], 'executed_reads': [],
                  'potential_dependencies': witnesses, 'potential_ids': potential_ids,
                  'basis': basis, 'assurance': {'kind': 'computed', 'formal_scope': []},
                  'cost': {'steps': 0, 'preflight_steps': 0, 'node_evaluations': {}}}
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
        request = module('composition/v1' if composed else 'arithmetic/v1').prepare(
            view, tree, component['nodes'], potential_ids, limits)
        return request, result

    def evaluate_many(self, requests):
        prepared = []
        output_budget = OutputBudget(self.operational_limits['output_bytes'])
        input_budget = OutputBudget(self.operational_limits['input_bytes'], 'batch_input_limit')
        for expression, declared in requests:
            if len(prepared) >= self.operational_limits['batch_requests']:
                raise OperationalLimit('batch_request_limit')
            try:
                item = self.prepare(expression, declared)
            except OperationalLimit:
                raise
            except (ValueError, TypeError, RecursionError, SyntaxError) as error:
                code = error.code if isinstance(error, CapabilityError) else 'invalid_expression'
                item = (None, {
                    'schema_version': 1, 'profile': PROFILE, 'modules': [],
                    'snapshot_id': self.snapshot.snapshot_id,
                    'computation_id': None,
                    'status': 'unsupported_capability' if code == 'unsupported_capability' else 'error',
                    'value': None, 'diagnostics': [_diagnostic(code)],
                    'executed_reads': [], 'potential_dependencies': [], 'potential_ids': [],
                    'basis': None, 'implementation': None,
                    'resource_profile': {'version': 'resources/v2', **self.limits},
                    'operational_limits': dict(self.operational_limits),
                    'assurance': {'kind': 'computed', 'formal_scope': []},
                    'cost': {'steps': 0, 'preflight_steps': 0, 'node_evaluations': {}}})
            # Charge each complete envelope before retaining the next closure.
            # This bounds aggregate preparation even when many roots share a DAG.
            output_budget.add(item[1])
            input_budget.add(item[0])
            prepared.append(item)
        active = [(request, result) for request, result in prepared if request is not None]
        if active:
            try:
                if self.runtime is None:
                    self.runtime = Runtime(operational_limits=self.operational_limits)
                wire_requests = [request['request'] if request.get('protocol') == 'KP4' else request
                                 for request, _ in active]
                responses = self.runtime.request_many(wire_requests, operational_limits=self.operational_limits) \
                    if isinstance(self.runtime, Runtime) else self.runtime.request_many(wire_requests)
                if len(responses) != len(active):
                    raise RuntimeUnavailable('native response count mismatch')
                for (request, result), response in zip(active, responses):
                    if request.get('protocol') == 'KP4':
                        module('query/v1').decode_response(response, request)
                    elif not set(response['executed_reads']) <= set(request['declared']) \
                            or not set(response['potential_reads']) <= set(result['potential_ids']):
                        raise RuntimeUnavailable('native dependency witness disagrees with captured closure')
                for (request, result), response in zip(active, responses):
                    implementation = self.runtime.implementation_for(request) if callable(
                        getattr(self.runtime, 'implementation_for', None)) else self.runtime.implementation
                    if request.get('protocol') == 'KP4':
                        response = module('query/v1').decode_response(response, request)
                        basis = module('query/v1').finalize_basis(request, response)
                        result.update(
                            status=response['status'], value=copy.deepcopy(response['value']),
                            diagnostics=copy.deepcopy(response['diagnostics']),
                            query_counts=copy.deepcopy(response['query_counts']),
                            executed_reads=copy.deepcopy(response['executed_reads']),
                            cost=copy.deepcopy(response['cost']), basis=basis,
                            computation_id=None if basis is None else digest({
                                'snapshot_id': self.snapshot.snapshot_id,
                                'basis': basis,
                                'resources': result['resource_profile'],
                            }),
                            implementation=copy.deepcopy(implementation))
                        result['assurance']['implementation'] = digest(implementation)
                        continue
                    result.update(status=response['status'], value=response['value'],
                                  diagnostics=[_diagnostic(code) for code in response['diagnostics']],
                                  executed_reads=[{'kind': 'node', 'id': nid,
                                      'fingerprint': self.snapshot._input_basis().summary(nid)['fingerprint']}
                                      for nid in response['executed_reads']],
                                  cost={'steps': response['steps'], 'preflight_steps': response['preflight_steps'],
                                        'node_evaluations': response['node_evaluations']},
                                  implementation=copy.deepcopy(implementation))
                    result['assurance']['implementation'] = digest(implementation)
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
                if isinstance(error, OperationalLimit):
                    raise
                code = 'runtime_timeout' if isinstance(error, subprocess.TimeoutExpired) else 'runtime_unavailable'
                for request, result in active:
                    result.update(status='operational_error', value=None,
                                  diagnostics=[_query_diagnostic(code)]
                                  if request.get('protocol') == 'KP4' else [_diagnostic(code)],
                                  executed_reads=[], basis=None, computation_id=None,
                                  cost=self._query_cost()
                                  if request.get('protocol') == 'KP4' else
                                  {'steps': 0, 'preflight_steps': 0, 'node_evaluations': {}},
                                  assurance={'kind': 'computed', 'formal_scope': []})
        results = [result for _, result in prepared]
        OutputBudget(self.operational_limits['output_bytes']).add(results)
        return results

    def evaluate(self, expression, *, declared):
        return self.evaluate_many([(expression, declared)])[0]


def evaluate(snapshot, expression, *, declared, limits=None, runtime=None, operational_limits=None):
    return Evaluator(snapshot, runtime=runtime, limits=limits,
                     operational_limits=operational_limits).evaluate(expression, declared=declared)


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
