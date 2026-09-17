"""Pure identity contract for page-local, secondary assessment inputs.

Page coverage and arrangement are derived from a captured brief and a captured
consumer projection.  They are deliberately not part of the canonical schema-v3
assessment.  This module binds those secondary inputs to the exact v3 identities
without loading a brief, evaluating a node, or copying canonical findings into a
page-owned envelope.
"""
import copy
import hashlib
import re

from .contract import OperationalLimit, OutputBudget, digest, operational_bounds
from .history_assessment import validate as validate_assessment


PAGE_ASSESSMENT_SCHEMA_VERSION = 1
PAGE_ASSESSMENT_PROFILE = 'page-secondary/v1'
PAGE_PROJECTION_VERSION = 1

_IDENTITY = re.compile(r'^[0-9a-f]{64}$')
_RESERVED_PAGE_INPUTS = frozenset({
    'snapshot_id', 'findings_revision', 'assessment_revision',
    'envelope_revision', 'page_assessment_revision',
})


def _identity(value, field):
    if not isinstance(value, str) or not _IDENTITY.fullmatch(value):
        raise ValueError('invalid ' + field)
    return value


def _canonical_assessment(report):
    """Validate the v3 envelope identity needed by the secondary contract.

    The v3 envelope owns its semantic validation.  The page boundary nevertheless
    checks its public profile and canonical whole-envelope digest so that mutated
    or presentation-only projections cannot be presented as canonical findings.
    """
    try:
        report = validate_assessment(report)
    except (ValueError, OperationalLimit) as error:
        raise ValueError('page assessment requires a canonical core/v1 v3 assessment: '
                         + str(error)) from None
    snapshot_id = _identity(report.get('snapshot_id'), 'snapshot_id')
    findings_revision = _identity(report.get('findings_revision'), 'findings_revision')
    envelope_revision = _identity(report.get('envelope_revision'), 'envelope_revision')
    preimage = {key: value for key, value in report.items() if key != 'envelope_revision'}
    if digest(preimage) != envelope_revision:
        raise ValueError('noncanonical v3 envelope_revision')
    return snapshot_id, findings_revision


def capture_brief(content, *, operational_limits=None):
    """Return an identity for already-captured brief bytes (or an absent brief).

    Exact bytes are intentional: comments, ordering, and spelling can affect the
    page even when a YAML decoder would produce the same mapping.  Reading those
    bytes remains the caller's capture responsibility.
    """
    bounds = operational_bounds(operational_limits)
    if content is None:
        return {'status': 'absent'}
    if isinstance(content, str):
        content = content.encode('utf-8')
    if not isinstance(content, bytes):
        raise ValueError('captured brief must be bytes, text, or None')
    if len(content) > bounds['input_bytes']:
        raise OperationalLimit('page_brief_input_limit')
    return {'status': 'captured', 'byte_length': len(content),
            'sha256': hashlib.sha256(content).hexdigest()}


def _validate_brief_identity(identity, maximum_bytes=None):
    if not isinstance(identity, dict):
        raise ValueError('invalid brief identity')
    if identity.get('status') == 'absent':
        if set(identity) != {'status'}:
            raise ValueError('invalid absent brief identity')
    elif identity.get('status') == 'captured':
        if set(identity) != {'status', 'byte_length', 'sha256'} \
                or type(identity['byte_length']) is not int or identity['byte_length'] < 0:
            raise ValueError('invalid captured brief identity')
        if maximum_bytes is not None and identity['byte_length'] > maximum_bytes:
            raise OperationalLimit('page_brief_input_limit')
        _identity(identity['sha256'], 'brief sha256')
    else:
        raise ValueError('invalid brief identity status')
    return copy.deepcopy(identity)


def capture_page_inputs(canonical_assessment, values, *, operational_limits=None):
    """Bind already-derived page inputs to one canonical v3 assessment.

    ``values`` may contain normalized page nodes, edges, coverage inputs, and
    arrangement inputs.  It cannot restate any canonical revision: those are
    supplied only by the validated v3 envelope.
    """
    snapshot_id, findings_revision = _canonical_assessment(canonical_assessment)
    if not isinstance(values, dict):
        raise ValueError('derived page inputs must be a mapping')
    overlap = _RESERVED_PAGE_INPUTS.intersection(values)
    if overlap:
        raise ValueError('derived page inputs cannot overwrite canonical identities: '
                         + ', '.join(sorted(overlap)))
    bounds = operational_bounds(operational_limits)
    copied = copy.deepcopy(values)
    OutputBudget(bounds['input_bytes'], 'page_input_limit').add(copied)
    bound = {'snapshot_id': snapshot_id, 'findings_revision': findings_revision,
             'values': copied}
    bound['page_inputs_revision'] = digest(bound)
    return bound


def _validate_page_inputs(bound, snapshot_id, findings_revision, bounds):
    if not isinstance(bound, dict) or set(bound) != {
            'snapshot_id', 'findings_revision', 'values', 'page_inputs_revision'}:
        raise ValueError('invalid derived page input envelope')
    _identity(bound.get('snapshot_id'), 'page input snapshot_id')
    _identity(bound.get('findings_revision'), 'page input findings_revision')
    if bound['snapshot_id'] != snapshot_id or bound['findings_revision'] != findings_revision:
        raise ValueError('derived page inputs belong to a different canonical assessment')
    if not isinstance(bound['values'], dict):
        raise ValueError('derived page input values must be a mapping')
    overlap = _RESERVED_PAGE_INPUTS.intersection(bound['values'])
    if overlap:
        raise ValueError('derived page inputs cannot overwrite canonical identities: '
                         + ', '.join(sorted(overlap)))
    _identity(bound.get('page_inputs_revision'), 'page_inputs_revision')
    copied = copy.deepcopy(bound)
    OutputBudget(bounds['input_bytes'], 'page_input_limit').add(copied['values'])
    supplied = copied.pop('page_inputs_revision')
    if digest(copied) != supplied:
        raise ValueError('noncanonical page_inputs_revision')
    return copy.deepcopy(bound)


def build(canonical_assessment, brief_identity, page_inputs, *,
          page_projection_version=PAGE_PROJECTION_VERSION, operational_limits=None):
    """Build the bounded page-secondary identity envelope.

    Canonical nodes/history/support never enter this envelope.  Consumers retain
    the validated v3 assessment separately and use the two immutable identifiers
    below to prove which findings their page-local checks accompany.
    """
    bounds = operational_bounds(operational_limits)
    snapshot_id, findings_revision = _canonical_assessment(canonical_assessment)
    brief = _validate_brief_identity(brief_identity, bounds['input_bytes'])
    inputs = _validate_page_inputs(page_inputs, snapshot_id, findings_revision, bounds)
    if type(page_projection_version) is not int or page_projection_version != PAGE_PROJECTION_VERSION:
        raise ValueError('unsupported page projection version')
    envelope = {
        'schema_version': PAGE_ASSESSMENT_SCHEMA_VERSION,
        'assessment_profile': PAGE_ASSESSMENT_PROFILE,
        'snapshot_id': snapshot_id,
        'findings_revision': findings_revision,
        'brief_identity': brief,
        'page_inputs': {
            'revision': inputs['page_inputs_revision'],
            'values': inputs['values'],
        },
        'page_projection_version': page_projection_version,
    }
    envelope['page_assessment_revision'] = digest(envelope)
    OutputBudget(bounds['output_bytes'], 'page_assessment_output_limit').add(envelope)
    return envelope


def validate(envelope, canonical_assessment, *, operational_limits=None):
    """Validate a replayed page envelope against its retained v3 assessment."""
    if not isinstance(envelope, dict) or set(envelope) != {
            'schema_version', 'assessment_profile', 'snapshot_id', 'findings_revision',
            'brief_identity', 'page_inputs', 'page_projection_version',
            'page_assessment_revision'}:
        raise ValueError('invalid page assessment envelope')
    if envelope.get('schema_version') != PAGE_ASSESSMENT_SCHEMA_VERSION \
            or envelope.get('assessment_profile') != PAGE_ASSESSMENT_PROFILE:
        raise ValueError('unsupported page assessment envelope')
    snapshot_id, findings_revision = _canonical_assessment(canonical_assessment)
    if envelope.get('snapshot_id') != snapshot_id \
            or envelope.get('findings_revision') != findings_revision:
        raise ValueError('page assessment belongs to a different canonical assessment')
    bounds = operational_bounds(operational_limits)
    brief = _validate_brief_identity(envelope.get('brief_identity'), bounds['input_bytes'])
    page = envelope.get('page_inputs')
    if not isinstance(page, dict) or set(page) != {'revision', 'values'}:
        raise ValueError('invalid page inputs')
    bound = {'snapshot_id': snapshot_id, 'findings_revision': findings_revision,
             'values': page['values'], 'page_inputs_revision': page['revision']}
    inputs = _validate_page_inputs(bound, snapshot_id, findings_revision, bounds)
    rebuilt = build(canonical_assessment, brief, inputs,
                    page_projection_version=envelope.get('page_projection_version'),
                    operational_limits=bounds)
    if rebuilt != envelope:
        raise ValueError('noncanonical page_assessment_revision')
    return copy.deepcopy(envelope)
