"""One immutable core assessment context for every downstream consumer."""
import copy
import json

from .contract import digest
from .history_assessment import assess as assess_history, validate as validate_assessment
from .projection import project_findings
from .snapshot import Snapshot


CONTEXT_VERSION = 1
FAILURE_VERSION = 1
CONSUMER_VIEW_VERSION = 'reasoning-projection/v1'


def capture_failure(error, *, stage='capture'):
    """Return the versioned no-findings envelope for a pre-assessment failure."""
    detail = str(error) or error.__class__.__name__
    code = getattr(error, 'code', None)
    if not isinstance(code, str) or not code:
        code = detail.split(':', 1)[0].strip().lower().replace(' ', '_')
    if not code:
        code = 'capture_failed'
    envelope = {'schema_version': FAILURE_VERSION, 'kind': 'assessment_capture_failure',
                'assessment_profile': 'core/v1', 'stage': stage, 'code': code,
                'detail': detail, 'findings': None}
    envelope['failure_revision'] = digest(envelope)
    return envelope


def validate_failure(envelope):
    expected = {'schema_version', 'kind', 'assessment_profile', 'stage', 'code',
                'detail', 'findings', 'failure_revision'}
    if not isinstance(envelope, dict) or set(envelope) != expected \
            or envelope['schema_version'] != FAILURE_VERSION \
            or envelope['kind'] != 'assessment_capture_failure' \
            or envelope['assessment_profile'] != 'core/v1' \
            or not all(isinstance(envelope[key], str) and envelope[key]
                       for key in ('stage', 'code', 'detail')) \
            or envelope['findings'] is not None:
        raise ValueError('invalid assessment capture failure')
    preimage = {key: value for key, value in envelope.items() if key != 'failure_revision'}
    if envelope['failure_revision'] != digest(preimage):
        raise ValueError('noncanonical assessment capture failure')
    return copy.deepcopy(envelope)


class CaptureError(ValueError):
    def __init__(self, envelope):
        self.envelope = validate_failure(envelope)
        super().__init__(self.envelope['code'] + ': ' + self.envelope['detail'])


class CapturedAssessment:
    """Validated Snapshot, v3 findings and pure view retained as one unit."""
    def __init__(self, snapshot, assessment):
        if not isinstance(snapshot, Snapshot):
            raise ValueError('captured assessment requires a Snapshot')
        report = validate_assessment(assessment)
        if report['snapshot_id'] != snapshot.snapshot_id:
            raise ValueError('assessment belongs to a different snapshot')
        view = project_findings(report)
        if view['snapshot_id'] != snapshot.snapshot_id \
                or view['findings_revision'] != report['findings_revision']:
            raise ValueError('consumer view belongs to a different assessment')
        self._snapshot = snapshot
        self._assessment = report
        self._view = view

    @classmethod
    def from_snapshot(cls, snapshot, selection=None, *, policy='focused-review/v1', runtime=None,
                      operational_limits=None, display_selection=None):
        return cls(snapshot, assess_history(
            snapshot, selection, policy=policy, runtime=runtime,
            operational_limits=operational_limits, display_selection=display_selection))

    @classmethod
    def capture(cls, paths, selection=None, *, policy='focused-review/v1', as_of=None,
                runtime=None, operational_limits=None, display_selection=None):
        try:
            snapshot = Snapshot.capture(paths, as_of=as_of)
            return cls.from_snapshot(
                snapshot, selection, policy=policy, runtime=runtime,
                operational_limits=operational_limits, display_selection=display_selection)
        except CaptureError:
            raise
        except Exception as error:
            raise CaptureError(capture_failure(error)) from error

    @property
    def snapshot(self):
        return Snapshot.from_json(self._snapshot.to_json())

    @property
    def snapshot_id(self):
        return self._snapshot.snapshot_id

    @property
    def findings_revision(self):
        return self._assessment['findings_revision']

    @property
    def assessment(self):
        return copy.deepcopy(self._assessment)

    @property
    def view(self):
        return copy.deepcopy(self._view)

    def session_revision(self, project_identity):
        payload = {'version': 1, 'project_identity': copy.deepcopy(project_identity),
                   'snapshot_id': self.snapshot_id,
                   'findings_revision': self.findings_revision,
                   'consumer_view_version': CONSUMER_VIEW_VERSION}
        return digest(payload)

    def to_data(self):
        payload = {'version': CONTEXT_VERSION, 'snapshot': self._snapshot.to_json(),
                   'assessment': self.assessment, 'view': self.view}
        payload['context_revision'] = digest(payload)
        return payload

    def to_json(self):
        return json.dumps(self.to_data(), ensure_ascii=False, sort_keys=True,
                          separators=(',', ':'))

    @classmethod
    def from_data(cls, payload):
        expected = {'version', 'snapshot', 'assessment', 'view', 'context_revision'}
        if not isinstance(payload, dict) or set(payload) != expected \
                or payload['version'] != CONTEXT_VERSION \
                or not isinstance(payload['snapshot'], str):
            raise ValueError('invalid captured assessment context')
        preimage = {key: value for key, value in payload.items() if key != 'context_revision'}
        if payload['context_revision'] != digest(preimage):
            raise ValueError('noncanonical captured assessment context')
        context = cls(Snapshot.from_json(payload['snapshot']), payload['assessment'])
        if context.view != payload['view']:
            raise ValueError('captured consumer view does not match findings')
        return context

    @classmethod
    def from_json(cls, value):
        try:
            payload = json.loads(value)
        except (TypeError, ValueError) as error:
            raise ValueError('invalid captured assessment JSON') from error
        return cls.from_data(payload)
