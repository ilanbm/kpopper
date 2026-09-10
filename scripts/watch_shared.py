"""Explicit external observations, one canonical shared record, asynchronous writes.

Captured scope is an assertion by the authorized caller, not a classification guessed
from branch text. Ambiguous/conflicting reports stay in the durable inbox for review.
"""
import contextlib
import datetime
import io
import json
import math
from pathlib import Path
import re
import tempfile
import time
import uuid
import sys
try:
    from . import watch as W
except ImportError:
    import watch as W
I, P = W.I, W.P


def load_report(path):
    if path == '-':
        text = sys.stdin.read(65537)
    else:
        with open(path, encoding='utf-8') as source:
            text = source.read(65537)
    if len(text.encode('utf-8')) > 65536:
        raise ValueError('shared report exceeds 64 KiB')
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('duplicate report field: ' + key)
            result[key] = value
        return result
    return json.loads(text, object_pairs_hook=unique)


def layout(watch, require_enabled=True):
    config = watch.config() or {}
    if (require_enabled and not config.get('enabled')) or not config.get('shared_record'):
        raise ValueError('configure a shared destination with watch setup before sharing facts')
    record = Path(config['shared_record'])
    # Queue ownership follows the canonical destination, even across different worktrees.
    root = watch.project_state / 'shared-inbox' / W.digest(str(record))
    return record, root


def validate(report):
    required = {'id', 'name', 'value', 'date', 'scope', 'source', 'source_quote'}
    if not isinstance(report, dict) or set(report) - required - {'event_id'} or required - set(report):
        raise ValueError('shared report needs id, name, value, date, scope, source and source_quote')
    if not isinstance(report['id'], str) or not re.fullmatch(r'[A-Za-z][A-Za-z0-9_.-]{1,159}', report['id']):
        raise ValueError('invalid shared fact ID')
    for key in ('name', 'source_quote'):
        if not isinstance(report[key], str) or not report[key].strip():
            raise ValueError(key + ' must be nonempty text')
    value = report['value']
    if type(value) not in (str, int, float, bool) or isinstance(value, float) and not math.isfinite(value):
        raise ValueError('shared observations need a finite scalar value')
    if not isinstance(report['date'], str) or datetime.date.fromisoformat(report['date']).isoformat() != report['date']:
        raise ValueError('date must be YYYY-MM-DD')
    scope = report['scope']
    if not isinstance(scope, dict) or set(scope) != {'kind', 'environment'} or scope['kind'] != 'external' \
            or not isinstance(scope['environment'], str) or not scope['environment'].strip():
        raise ValueError('scope must explicitly name kind=external and its environment; branch/code claims cannot be shared')
    source = report['source']
    if not isinstance(source, dict) or set(source) - {'url', 'file', 'at'} or not source.get('at') \
            or len(set(source) & {'url', 'file'}) != 1 or not all(isinstance(v, str) and v.strip() for v in source.values()):
        raise ValueError('source needs one URL or absolute file path, and an exact at location')
    if source.get('file') and not Path(source['file']).is_absolute():
        raise ValueError('shared source file paths must be absolute')
    if source.get('url') and not source['url'].startswith(('https://', 'http://')):
        raise ValueError('source URL must be HTTP(S)')
    if len(I._json_bytes(report)) > 65536:
        raise ValueError('shared report exceeds 64 KiB')
    if 'event_id' in report and (not isinstance(report['event_id'], str) or
            not re.fullmatch(r'[A-Za-z0-9_-]{1,120}', report['event_id'])):
        raise ValueError('invalid event_id')


def capture(watch, report):
    validate(report)
    record, root = layout(watch)
    if not record.is_file():
        raise ValueError('configured shared record is unavailable; it was not recreated')
    eid = report.get('event_id') or uuid.uuid4().hex
    path = root / 'events' / (eid + '.json')
    with I._file_lock(root / 'capture.lock'):
        existing = I._load(path)
        if existing:
            if existing['report'] != report:
                raise ValueError('event_id was already used for a different report')
        else:
            with P._locked(str(record)):
                doc = P.load([str(record)])
                target = P.bodies(doc).get(report['id'])
            I._save(root / 'sources' / (eid + '.json'), report)
            I._save(path, {'id': eid, 'report': report, 'report_hash': W.digest(report), 'record': str(record), 'state': 'captured',
                          'captured_at': time.time_ns(), 'target_before': W.digest(target),
                          'origin': str(watch.tree), 'captured_from': str(watch.cwd)})
    watch.request()
    return {'state': 'captured', 'event_id': eid, 'record': str(record), 'receipt': str(path)}


def _apply(watch, record, root, event):
    report = event['report']
    validate(report)
    source_id = 's.shared_' + event['id'].replace('-', '_')
    journal_path = root / 'journals' / (event['id'] + '.json')
    source_path = root / 'sources' / (event['id'] + '.json')
    if W.digest(report) != event['report_hash'] or I._load(source_path) != report:
        raise ValueError('retained shared source failed its integrity check')
    for _ in range(3):
        with P._locked(str(record)):
            before = record.read_bytes()
            doc = P.load([str(record)])
            if len(P._files_of([str(record)])) != 1 or doc.get('record') or doc.get('also') or doc.hypotheses:
                raise ValueError('shared write needs a single canonical record; existing pointers/hypotheses need explicit review')
            target = P.bodies(doc).get(report['id'])
            journal = I._load(journal_path)
            if journal and W.digest(target) == journal['target_after']:
                source = P.bodies(doc).get(source_id)
                if source and source.get('file') == str(source_path) and target.get('from') == source_id:
                    return {'state': 'applied', 'recovered': True, 'target_after': W.digest(target)}
            if W.digest(target) != event['target_before']:
                raise ValueError('shared target changed after capture; retained for review')
            if target is not None and (not isinstance(target, dict) or 'v' not in target or
                                       target.get('scope') != report['scope']):
                raise ValueError('existing target has another scope or is not a shared scalar observation')
        with tempfile.TemporaryDirectory() as folder:
            shadow = Path(folder) / record.name
            shadow.write_text(re.sub(r'^(sources|known):[ \t]*\{\}[ \t]*$', r'\1:',
                                     before.decode('utf-8'), flags=re.MULTILINE))
            lines = shadow.read_text().split('\n')
            for collection in ('sources', 'known'):
                P._ensure_collection(lines, collection)
            shadow.write_text('\n'.join(lines))
            source_body = {'name': report['name'] + ' — source report', 'file': str(source_path),
                           'read': report['date'], 'origin': report['source']}
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                if not W._entries(doc):
                    # Bootstrap from the actual retained source; no invented judgment is
                    # needed to make an empty private destination into a readable record.
                    initial = {k: v for k, v in doc.items() if v != {}}
                    initial['sources'] = {source_id: source_body}
                    shadow.write_text(P.yaml.safe_dump(initial, sort_keys=False, allow_unicode=True) + 'known:\n')
                else:
                    P.apply([str(shadow)], {'kind': 'add', 'id': source_id, 'into': 'sources', 'as_of': report['date'],
                        'body': source_body})
                if target is None:
                    P.apply([str(shadow)], {'kind': 'add', 'id': report['id'], 'into': 'known', 'as_of': report['date'],
                        'body': {'name': report['name'], 'v': report['value'], 'of': report['date'],
                                 'scope': report['scope'], 'from': source_id, 'at': report['source']['at']}})
                else:
                    P.apply([str(shadow)], {'kind': 'set', 'id': report['id'], 'value': report['value'],
                        'as_of': report['date'], 'source': source_id, 'at': report['source']['at']})
            after = shadow.read_bytes()
            updated = P.bodies(P.load([str(shadow)]))[report['id']]
            if type(updated['v']) is not type(report['value']) or updated['v'] != report['value'] \
                    or updated['from'] != source_id:
                raise ValueError('shared update did not preserve the value and source')
            # A newly falsified existing judgment is a finding, not grounds to hide a fact.
            before_fail = W.C._check_of(doc)[0] if W._entries(doc) else []
            after_fail, _ = W.C._check_of(P.load([str(shadow)]))
            I._save(journal_path, {'before': W.digest(before.hex()), 'target_after': W.digest(updated)})
            with I._file_lock(watch.config_path.with_suffix('.lock')):
                config = watch.config() or {}
                if not config.get('enabled') or config.get('shared_record') != str(record):
                    raise ValueError('watch was paused or its destination changed before commit')
                with P._locked(str(record)):
                    if record.read_bytes() != before:
                        continue
                    I._atomic(root / 'backups' / (event['id'] + '.yaml'), before)
                    I._replace_record(record, after)
            return {'state': 'applied', 'target_after': W.digest(updated),
                    'findings': [line for line in after_fail if line not in before_fail]}
    raise ValueError('shared record kept changing; report retained for review')


def process(watch):
    record, root = layout(watch)
    with W.try_lock(root / 'processor.lock') as acquired:
        if not acquired:
            return False
        applied = False
        events = [I._load(p) for p in (root / 'events').glob('*.json')]
        for event in sorted(events, key=lambda e: e['captured_at']):
            if event['state'] != 'captured':
                continue
            try:
                if event['record'] != str(record):
                    raise ValueError('shared destination changed')
                outcome = _apply(watch, record, root, event)
            except (Exception, SystemExit) as exc:
                outcome = {'state': 'needs_review', 'reason': str(exc)[:1200]}
            I._save(root / 'events' / (event['id'] + '.json'), dict(event, **outcome))
            applied = applied or outcome['state'] == 'applied'
        return applied


def read(watch):
    record, root = layout(watch, require_enabled=False)
    doc = P.load([str(record)])
    return {'record': str(record), 'entries': P.bodies(doc),
            'reports': [I._load(p) for p in sorted((root / 'events').glob('*.json'))]}


def receipts(watch):
    _, root = layout(watch, require_enabled=False)
    return [dict({k: event.get(k) for k in ('id', 'state', 'origin', 'reason', 'resolution')},
                 target=event.get('report', {}).get('id'))
            for event in [I._load(p) for p in sorted((root / 'events').glob('*.json'))] if event]


def resolve(watch, event_id, evidence):
    if not isinstance(evidence, str) or not evidence.strip() or len(evidence) > 4000:
        raise ValueError('record the evidence for resolving this retained report')
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,120}', event_id):
        raise ValueError('invalid event_id')
    _, root = layout(watch, require_enabled=False)
    with I._file_lock(root / 'processor.lock'):
        path = root / 'events' / (event_id + '.json')
        event = I._load(path)
        if not event or event['state'] != 'needs_review':
            raise ValueError('only a retained review report can be resolved')
        event.update(state='resolved', resolution=evidence)
        I._save(path, event)
    watch.request()
    return {'state': 'resolved', 'event_id': event_id}
