"""Versioned recorded findings, independent of display and attention policy.

The ordinary reader remains the evaluator. This module preserves its findings as data;
consumers do not interpret prose or evaluate expressions to select relevant attention.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path

if 'P' not in globals():  # A reader loaded by file path can bind its own functions.
    try:
        from . import provenance as P
    except ImportError:
        import provenance as P

SCHEMA_VERSION = 1
PROFILE = 'ordinary-reader/v1'
POLICY = 'focused-review/v1'
LOADED_SOURCE_HASH = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def _digest(value):
    try:
        text = json.dumps(value, ensure_ascii=False, default=str, sort_keys=True)
    except TypeError:
        try:
            text = json.dumps(value, ensure_ascii=False, default=str)
        except TypeError as error:
            raise ValueError('record contains mapping keys unsupported by assessment') from error
    return hashlib.sha256(text.encode()).hexdigest()


def _side(status, **fields):
    return dict(status=status, **fields)


def _references(predicate):
    """Complete reads for the supported syntax, not identifiers inside literals."""
    if predicate is None or predicate == '':
        return []
    if P.why_undecided(predicate):
        return None
    if isinstance(predicate, dict):
        return sorted(set(P.E.refs(predicate)))
    match = P.CMP.match(predicate)
    if not match:
        return None
    left, _, right = match.groups()
    return sorted({left, right.strip()} if P.ID.fullmatch(right.strip()) else {left})


def judgment_state(judgment, raw, ids, fields, judgments=None):
    """Assess independent dimensions without treating absence as false or validity."""
    body = judgment['body']
    judgments = judgments or {}
    declared = body.get(fields['deps'])
    valid_deps = isinstance(declared, list) and all(isinstance(d, str) for d in declared)
    issues = []
    if not valid_deps:
        issues.append({'code': 'invalid_dependencies', 'field': fields['deps']})
    snapshot = body.get(fields.get('snapshot')) if fields.get('snapshot') else None
    invalid_snapshot = snapshot is not None and not isinstance(snapshot, dict)
    if invalid_snapshot:
        issues.append({'code': 'invalid_snapshot', 'field': fields['snapshot']})
    deps = list(dict.fromkeys(declared)) if valid_deps else []
    # Preserve the ordinary comparator, including its formula-change behavior. A
    # disposition such as muted is not the comparison result exposed to consumers.
    moves = {d: (old, now, disposition) for d, old, now, disposition in
             P.moved_deps(judgment, raw, ids)} if valid_deps else {}
    readings = {}
    for dep in deps:
        old = judgment['snap'].get(dep)
        prior = _side('unavailable', reason='invalid_snapshot') if invalid_snapshot else \
            _side('recorded', value=copy.deepcopy(old)) if dep in judgment['snap'] else _side('missing')
        current = _side('missing')
        comparable = None
        current_rule = None
        if dep in ids:
            entry = raw.get(dep)
            current_rule = entry.get('rule') if isinstance(entry, dict) else None
            comparable = P.value_of(raw, ids, dep)
            explicit_null = (isinstance(entry, dict) and 'v' in entry and entry['v'] is None
                             and entry.get('quoted') is None and not current_rule)
            try:
                value = comparable if comparable is not None else None if explicit_null else \
                    P.snapshot_value(dep, raw, ids, judgments, {})
                current = _side('recorded', value=copy.deepcopy(value)) if value is not None or explicit_null \
                    else _side('unavailable', reason='reading_unavailable')
            except P.Refused as error:
                current = _side('unavailable', reason='calculation_unavailable', detail=str(error))
        computed = old.get('computed') if isinstance(old, dict) else None
        computed = computed if isinstance(computed, dict) and 'value' in computed and 'rule' in computed else None
        legacy_rule = P.legacy_snapshot_rule(old, current_rule)
        rule_changed = None
        if computed is not None or legacy_rule is not None:
            rule_changed = not P.E.same(computed['rule'] if computed else legacy_rule, current_rule)
        reasons = []
        if prior['status'] == 'missing':
            reasons.append('baseline_missing')
        elif prior['status'] == 'unavailable':
            reasons.append(prior['reason'])
        if current['status'] == 'missing':
            reasons.append('current_missing')
        elif current['status'] == 'unavailable':
            reasons.append(current['reason'])
        elif legacy_rule is not None:
            reasons.append('historical_formula_only')
        elif comparable is None or (isinstance(comparable, (list, dict)) and P.E.number(comparable) is None) \
                or (isinstance(old, (list, dict)) and computed is None):
            reasons.append('not_compared_by_reader')
        comparison = 'unknown'
        if not reasons:
            if computed is not None:
                previous = computed['value']
                a, b = P.E.number(previous), P.E.number(comparable)
                equal = a == b if a is not None and b is not None else previous == comparable
                comparison = 'unknown' if previous is None or comparable is None else 'same' if equal else 'changed'
                if comparison == 'unknown':
                    reasons.append('historical_value_unavailable')
            else:
                comparison = 'changed' if dep in moves else 'same'
        readings[dep] = {'current': current, 'at_review': prior, 'comparison': comparison,
                         'reasons': reasons, 'rule_changed': rule_changed}
        if computed:
            readings[dep]['historical_calculation'] = {key: copy.deepcopy(computed[key])
                                                      for key in ('value', 'rule')}
        if legacy_rule is not None:
            readings[dep]['historical_formula_only'] = True
        if dep not in ids:
            issues.append({'code': 'missing_dependency', 'field': fields['deps'], 'related_ids': [dep]})
        if prior['status'] == 'missing':
            issues.append({'code': 'missing_snapshot', 'field': fields.get('snapshot'), 'related_ids': [dep]})
    pred = judgment['pred']
    declared_predicate = pred is not None and pred != ''
    reads = _references(pred)
    result = P.evaluate(pred, raw, ids) if declared_predicate else None
    reason = P.why_undecided(pred) if declared_predicate else 'not_declared'
    error = P.computation_error(pred, raw, ids) if declared_predicate else ''
    status = 'not_declared' if not declared_predicate else 'error' if error else \
        'holds' if result is True else 'does_not_hold' if result is False else 'unknown'
    falsifier = {'status': status, 'expression': copy.deepcopy(pred), 'reads': reads}
    if status in {'unknown', 'error'}:
        falsifier['reason'] = 'evaluation_error' if error else 'unsupported_predicate' if reason else 'reading_unavailable'
        if error or reason:
            falsifier['detail'] = error or reason
    authored_predicate = body.get(fields.get('predicate'))
    if authored_predicate is not None and not isinstance(authored_predicate, (str, dict)):
        falsifier.update(status='error', reason='invalid_predicate_type', reads=None,
                         expression=copy.deepcopy(authored_predicate))
        issues.append({'code': 'invalid_predicate_type', 'field': fields.get('predicate')})
    elif authored_predicate == {}:
        falsifier.update(status='error', reason='invalid_predicate_shape', reads=None)
        issues.append({'code': 'invalid_predicate_shape', 'field': fields.get('predicate')})
    if falsifier['status'] == 'unknown' and falsifier.get('reason') == 'unsupported_predicate':
        # These are textual clues, not a claimed complete parse of executable reads.
        mentions = sorted({ref for ref in P.predicate_refs(pred) if ref in ids and ref not in deps})
        issues.append({'code': 'unsupported_predicate', 'field': fields.get('predicate'),
                       'related_ids': mentions})
    if reads is not None and valid_deps:
        undeclared = sorted(set(reads) - set(deps))
        if undeclared:
            issues.append({'code': 'undeclared_predicate_dependencies', 'field': fields.get('predicate'),
                           'related_ids': undeclared})
    return {'basis': {'status': 'assessed' if valid_deps else 'error', 'dependencies': readings,
                      **({} if valid_deps else {'reason': 'invalid_dependencies'})},
            'falsifier': falsifier,
            'contention': {'status': 'unassessed', 'reason': 'hypotheses_not_supplied',
                           'witnesses': [], 'alternatives': []},
            'integrity': {'status': 'assessed', 'checks': ['dependency_shape', 'snapshot_shape',
                'dependency_presence', 'snapshot_presence', 'predicate_type',
                'predicate_shape', 'predicate_dependencies'], 'issues': issues}}


def attention(state, policy=POLICY):
    """Pure policy over findings. No source reads, evaluator calls, or record mutation."""
    if policy not in (POLICY, 'falsifiers-only/v1'):
        raise ValueError('unknown attention policy: ' + str(policy))
    reasons = []
    falsifier = state['falsifier']
    if falsifier['status'] == 'holds':
        reasons.append({'code': 'falsifier_holds'})
    if policy == POLICY:
        reads = set(falsifier.get('reads') or [])
        for dep, finding in state['basis'].get('dependencies', {}).items():
            if finding.get('rule_changed') is True:
                reasons.append({'code': 'formula_changed', 'related_ids': [dep]})
            elif finding['comparison'] == 'changed' and not (
                    falsifier['status'] == 'does_not_hold' and dep in reads):
                reasons.append({'code': 'premise_changed', 'related_ids': [dep]})
    items = [{'action': 'review', 'reasons': reasons}] if reasons else []
    if policy == POLICY:
        repairs = [copy.deepcopy(issue) for issue in state['integrity']['issues']
                   if issue['code'] in {'invalid_dependencies', 'invalid_snapshot', 'invalid_predicate_type',
                                        'invalid_predicate_shape',
                                        'unsupported_predicate',
                                        'undeclared_predicate_dependencies'}]
        if repairs:
            items.append({'action': 'repair_record', 'reasons': repairs})
        gaps = [copy.deepcopy(issue) for issue in state['integrity']['issues']
                if issue['code'] == 'missing_dependency']
        if gaps:
            items.append({'action': 'resolve_gap', 'reasons': gaps})
        if state['contention']['status'] == 'detected':
            items.append({'action': 'inspect_alternatives', 'reasons': [{'code': 'competing_hypotheses'}]})
    return items


def reader_flags(state, judgment, raw, ids, fields, defer_counts=False):
    """Compatibility policy for existing selectors, counters and check/open output."""
    flags = set()
    blocked = P._blocked_text(judgment['body'])
    for dep in judgment['deps']:
        if dep not in ids:
            flags.add('blocked' if blocked else 'broken')
        elif fields['snapshot'] and dep not in judgment['seen']:
            flags.add('unchecked')
    if P.formula_only_snapshots(judgment, raw):
        flags.add('unchecked')
    named = bool([ref for ref in P.predicate_refs(judgment['pred']) if ref in ids]) \
        and not P.why_undecided(judgment['pred'])
    status = state['falsifier']['status']
    if not named and not blocked and not P._decided(judgment):
        flags.add('no_predicate')
    elif named and status == 'holds':
        flags.add('falsified')
    elif named and status in {'unknown', 'error'} and not (
            defer_counts and P.pending_counts(judgment['pred'], raw, ids)):
        flags.add('unknown')
    if any(disposition == 'moved' for _, _, _, disposition in P.moved_deps(judgment, raw, ids)):
        flags.add('moved')
    if P.reversal_pending(judgment['body']) and not P.is_arrangement(judgment, raw):
        flags.add('reversed')
    return flags


def selected_attention(report, ids=None, actions=None):
    """Select relevant actions without re-evaluating or changing assessment scope."""
    if 'nodes' in report:
        items = {nid: node['attention'] for nid, node in report['nodes'].items()}
        available = set(items)
    elif 'attention' in report:
        items = report['attention']
        available = set(report.get('selection', items))
    else:
        raise ValueError('report has neither findings nor attention')
    selected = available if ids is None else set(ids)
    if selected - available:
        raise ValueError('attention selection contains IDs outside this assessment')
    wanted = None if actions is None else set(actions)
    return {nid: [copy.deepcopy(item) for item in attention_items
                  if wanted is None or item['action'] in wanted]
            for nid, attention_items in items.items() if nid in selected and
            any(wanted is None or item['action'] in wanted for item in attention_items)}


def assess(doc, ids, judgments, fields, raw, policy=POLICY):
    """Assess the supplied base and disclose hypotheses inspected beside it."""
    P._peer('reasoning.contract').capabilities(doc, profile=PROFILE)
    entries = {nid for section, members in P.collections_of(doc).items()
               if section != 'meta' for nid in members}
    ids = {nid for nid in ids if nid in entries or P.is_builtin(nid)}
    hypotheses = getattr(doc, 'hypotheses', {})
    skipped = {name: hyp['error'] for name, hyp in sorted(hypotheses.items()) if hyp['error']}
    contributions = sorted(name for name, hyp in hypotheses.items()
                           if hyp.get('kind') == 'contribution' and name not in skipped)
    conflicts = getattr(doc, 'knowledge_conflicts', {})
    target = getattr(doc, 'knowledge_target', None)
    target_error = getattr(doc, 'target_unavailable', None)
    read_mode = getattr(doc, 'read_mode', 'supplied')
    disputed = P.contested(doc)
    nodes = {}
    for nid in sorted(ids):
        if nid in judgments:
            state = judgment_state(judgments[nid], raw, ids, fields, judgments)
        else:
            state = {'basis': {'status': 'not_applicable', 'dependencies': {}},
                     'falsifier': {'status': 'not_applicable', 'expression': None, 'reads': []},
                     'integrity': {'status': 'unassessed', 'reason': 'non_judgment_checks_not_run',
                                   'checks': [], 'issues': []}}
        alternatives = [{'hypothesis': name, 'id': nid} for name, hyp in sorted(hypotheses.items())
                        if not hyp['error'] and nid in hyp['ids']]
        state['contention'] = {'status': 'detected' if nid in disputed else 'none_detected',
                               'method': 'readable_hypothesis_pairs_and_knowledge_identity' if nid in conflicts
                                         else 'readable_hypothesis_pairs',
                               'alternatives': alternatives,
                               'witnesses': [{'hypothesis': name, 'claim': copy.deepcopy(claim)}
                                             for name, claim in disputed.get(nid, [])]}
        nodes[nid] = {'body': copy.deepcopy(raw.get(nid)), 'state': state,
                      'attention': attention(state, policy)}
    scope = {'base': 'supplied_record',
             'hypotheses_checked': sorted(set(hypotheses) - set(skipped) - set(contributions)),
             'hypotheses_skipped': skipped, 'external_sources_fetched': False,
             'coverage': 'partial' if skipped or target_error else 'supplied_record',
             'integrity_checks': 'listed per node; not a complete check or page verification'}
    scope['knowledge'] = {'read_mode': read_mode, 'contributions_checked': contributions,
        'conflict_sources': {nid: [name for name, _ in variants] for nid, variants in sorted(conflicts.items())},
        'target': {'status': 'unavailable' if target_error else 'observed' if target else 'unassessed',
                   'ref': target.get('ref') if target else None,
                   'revision': target.get('revision') if target else None, 'reason': target_error}}
    revision_data = {'record': doc, 'hypotheses': {
        name: {key: hyp[key] for key in ('doc', 'head', 'error')}
        for name, hyp in sorted(hypotheses.items())}}
    # Record attributes carry live knowledge context outside the authored YAML.
    # Bind complete typed conflict bodies, while the public scope exposes only
    # holder names and immutable target identity, never omitted evidence bodies.
    revision_data['knowledge_context'] = P._peer('pending_grounding').identity({
        'read_mode': read_mode,
        'conflicts': {nid: [list(variant) for variant in variants] for nid, variants in conflicts.items()},
        'target': target, 'target_unavailable': target_error})
    revision = _digest(revision_data)
    return {'schema_version': SCHEMA_VERSION, 'assessment_profile': PROFILE, 'attention_policy': policy,
            'record_revision': revision, 'scope': scope, 'nodes': nodes,
            'assessment_revision': _digest({'record_revision': revision, 'profile': PROFILE, 'scope': scope,
                'states': {nid: node['state'] for nid, node in nodes.items()}})}


def load(paths, policy=POLICY, *, profile=PROFILE, as_of=None, selection=None,
         history=False, display_selection=None):
    if profile == 'core/v1':
        snapshot = P._peer('reasoning.snapshot').Snapshot.capture(paths, as_of=as_of)
        if history:
            combined = P._peer('reasoning.history_assessment')
            return combined.assess(snapshot, selection, policy=policy,
                                   display_selection=display_selection)
        core = P._peer('reasoning.assessment')
        return core.assess(snapshot, selection, policy=policy)
    if history:
        raise ValueError('--history requires --profile core/v1')
    doc = P.load(paths)
    P._peer('reasoning.contract').capabilities(doc, profile=profile)
    if as_of is not None:
        raise ValueError('--as-of is available on the explicit core/v1 assessment profile')
    try:
        ids, judgments, fields = P.infer(doc)
    except TypeError as error:
        raise ValueError('cannot interpret record structure: ' + str(error)) from error
    if any(not isinstance(nid, str) for nid in ids):
        raise ValueError('assessment requires string entry IDs')
    raw = P.with_builtins(doc, ids, judgments, fields)
    return assess(doc, ids, judgments, fields, raw, policy)


def main(argv=None):
    parser = argparse.ArgumentParser(prog='kpop assess', description=__doc__)
    parser.add_argument('ids', nargs='+', help='exact IDs to assess; evaluation uses the full supplied record')
    parser.add_argument('--record', action='append')
    parser.add_argument('--policy', choices=[POLICY, 'falsifiers-only/v1'], default=POLICY)
    parser.add_argument('--profile', choices=[PROFILE, 'core/v1'], default=PROFILE,
                        help='core/v1 is an experimental read-only interpretation')
    parser.add_argument('--as-of', help='explicit ISO date or timezone-aware timestamp for core/v1')
    parser.add_argument('--history', action='store_true',
                        help='return the history-aware schema-v3 core assessment')
    parser.add_argument('--attention-only', action='store_true', help='only relevant actions from this assessment')
    args = parser.parse_args(argv)
    try:
        if args.history and args.attention_only:
            raise ValueError('--attention-only is not yet a versioned schema-v3 projection')
        report = load(args.record or P.default_paths(), args.policy, profile=args.profile,
                      as_of=args.as_of, selection=args.ids, history=args.history,
                      display_selection=args.ids if args.history else None)
        if set(args.ids) - set(report['nodes']):
            raise ValueError('unknown assessment ID; use open or pull to find an entry')
        if not args.history:
            report['selection'] = args.ids
            if args.attention_only:
                report['attention'] = selected_attention(report, args.ids)
                del report['nodes']
            else:
                report['nodes'] = {nid: report['nodes'][nid] for nid in dict.fromkeys(args.ids)}
        if args.profile == 'core/v1':
            # Account for the final selection/attention projection too. Compact
            # output keeps emitted UTF-8 within the compact ASCII JSON budget.
            contract = P._peer('reasoning.contract')
            if not args.history:
                contract.OutputBudget(report['operational_limits']['output_bytes'] - 1).add(report)
            print(json.dumps(report, ensure_ascii=False, default=str, separators=(',', ':')))
        else:
            print(json.dumps(report, ensure_ascii=False, default=str, indent=2))
    except (ValueError, OSError, P.yaml.YAMLError) as error:
        parser.error(str(error))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
