"""Read-only branch compatibility, captured and delivered outside the writing session.

Configuration belongs to a Git project; results belong to a checkout and a record.
No comparison path writes a record, fetches Git objects, or folds a hypothesis.
"""
import argparse
import contextlib
import copy
import io
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import subprocess
import sys
import tempfile
import time
import uuid

try:
    from . import ingestion as I, consolidate as C, workspace
except ImportError:
    import ingestion as I
    import consolidate as C
    import workspace
P = C.P
MAX_BYTES = 4 * 1024 * 1024
MAX_FILES = 128


def git(cwd, *args):
    result = subprocess.run(['git', '-C', str(cwd), *args], capture_output=True, text=True,
                            encoding='utf-8', timeout=10)
    if result.returncode:
        raise ValueError('Git could not read ' + ' '.join(args[:2]) + ': ' + result.stderr.strip()[:300])
    return result.stdout.strip()


def digest(value):
    return I._body_hash(value)


def _safe_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or not path.parts or \
            PureWindowsPath(name).drive or '\\' in name:
        raise ValueError('record pointer must use a portable path inside its checkout: ' + name)
    return path.as_posix()


def _records(root, entry, sha=None):
    """Materialize only the bounded record/pointer/hypothesis closure, never source files."""
    import posixpath
    # Each ref keeps the entry name and companion layout it was committed with.  A
    # checkout can therefore read GROUNDING.yaml while its merge base still has
    # PROVENANCE.yaml (or the reverse).
    if sha and PurePosixPath(entry).name in P.ENTRY_NAMES:
        directory = PurePosixPath(entry).parent

        def exists(name):
            result = subprocess.run(['git', '-C', str(root), 'cat-file', '-e', sha + ':' + name],
                                    capture_output=True, timeout=10)
            return result.returncode == 0

        if not exists(entry):
            for candidate in P.ENTRY_NAMES:
                alternate = str(directory / candidate)
                if alternate != entry and exists(alternate):
                    entry = alternate
                    break
    files, queue = {}, [entry]
    hdir = str(PurePosixPath(entry).parent / P.hypotheses_rel(entry))
    if sha:
        listing = git(root, 'ls-tree', '-r', '--name-only', sha, '--', hdir + '/')
        queue += [p for p in listing.splitlines() if p.endswith(('.yaml', '.yml'))]
    else:
        folder = root / hdir
        if folder.is_dir():
            queue += [p.relative_to(root).as_posix() for p in sorted(folder.iterdir())
                      if p.suffix in ('.yaml', '.yml')]
    while queue:
        name = _safe_path(queue.pop(0))
        if name in files:
            continue
        if len(files) >= MAX_FILES:
            raise ValueError('record closure exceeds 128 files')
        if sha:
            size = int(git(root, 'cat-file', '-s', sha + ':' + name))
            if size > MAX_BYTES:
                raise ValueError('record file exceeds 4 MiB')
            # Read blob bytes; git show text.strip() would erase meaningful YAML whitespace.
            r = subprocess.run(['git', '-C', str(root), 'show', sha + ':' + name],
                               capture_output=True, timeout=10)
            if r.returncode:
                raise ValueError('record unavailable at ' + sha[:12] + ':' + name)
            data = r.stdout
        else:
            path = root / name
            path.resolve().relative_to(root.resolve())
            if path.stat().st_size > MAX_BYTES:
                raise ValueError('record file exceeds 4 MiB')
            data = path.read_bytes()
        if len(data) > MAX_BYTES or sum(len(v.encode('utf-8')) for v in files.values()) + len(data) > MAX_BYTES:
            raise ValueError('record closure exceeds 4 MiB')
        text = data.decode('utf-8')
        body = P.yaml.safe_load(text)
        if not isinstance(body, dict):
            raise ValueError('record is not a mapping: ' + name)
        files[name] = text
        for pointer in C._pointers(body):
            queue.append(_safe_path(posixpath.normpath(posixpath.join(posixpath.dirname(name), pointer))))
    with tempfile.TemporaryDirectory() as directory:
        for name, text in files.items():
            path = Path(directory) / name
            path.resolve().relative_to(Path(directory).resolve())
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding='utf-8')
        doc = P.load([str(Path(directory) / entry)])
        hyps = []
        for name, h in doc.hypotheses.items():
            if h['error']:
                raise ValueError('unreadable hypothesis ' + name + ': ' + h['error'])
            hyps.append({'name': name, 'doc': h['doc'], 'head': h['head']})
    return {'doc': dict(doc), 'hypotheses': hyps, 'hash': digest(files)}


def _entries(doc):
    result = {}
    for collection, entries in P.collections_of(doc).items():
        if collection == 'meta':
            continue
        for key, body in entries.items():
            if key in result:
                raise ValueError('ambiguous entry ID: ' + key)
            result[key] = (collection, body)
    return result


def _doc(value):
    doc = P.Record(copy.deepcopy(value))
    doc.hypotheses = {}
    return doc


def same_claim(a, b):
    return P._same_claim(P.claim_of(a), P.claim_of(b)) and \
        (a.get('scope') if isinstance(a, dict) else None) == (b.get('scope') if isinstance(b, dict) else None)


def uncertain(doc):
    ids, judgments, fields, raw = C._view(doc)
    flags = P.flags(ids, judgments, fields, raw)
    result = {}
    for key, judgment in judgments.items():
        reasons = sorted(flags[key] & {'blocked', 'unchecked', 'broken', 'no_predicate'})
        if judgment['pred'] and P.evaluate(judgment['pred'], raw, ids) is None:
            reasons.append('predicate cannot currently be evaluated: ' + judgment['pred'])
        elif not judgment['pred'] and P._blocked_text(judgment['body']):
            reasons.append('declared blocked condition')
        if reasons:
            result[key] = reasons
    return result


def compare(snapshot):
    """Apply only the authored merge-base delta to main; retain conflicts as findings."""
    old = _entries(snapshot['ancestor']['doc'])
    local = _entries(snapshot['working']['doc'])
    main = _entries(snapshot['main']['doc'])
    base = _doc(snapshot['main']['doc'])
    findings, delta, changed = [], {}, []

    def finding(kind, key, reason):
        findings.append({'kind': kind, 'id': key, 'reason': reason})

    # Shared facts enter a view, never overwrite a same-ID branch reading implicitly.
    shared = snapshot.get('shared')
    if shared:
        for key, (col, body) in _entries(shared['doc']).items():
            if key in main and not same_claim(main[key][1], body):
                finding('collision', key, key + ': shared and main readings disagree; reconcile their scopes')
            elif key not in main:
                base.setdefault(col, {})[key] = copy.deepcopy(body)
        fail_before, _ = C._check_of(_doc(snapshot['main']['doc']))
        fail_shared, _ = C._check_of(base)
        for line in fail_shared:
            if line not in fail_before:
                finding('shared', line.split(':', 1)[0], line)
    for key in sorted(set(old) | set(local)):
        if old.get(key) == local.get(key):
            continue
        changed.append(key)
        if key in main and main.get(key) != old.get(key) and main.get(key) != local.get(key):
            if key not in local or not same_claim(main[key][1], local[key][1]):
                finding('collision', key, key + ': main and this worktree changed the same entry differently')
        elif key in old and key not in main and key in local:
            finding('collision', key, key + ': main removed the entry this worktree changed')
        if key not in local:
            for entries in base.values():
                if isinstance(entries, dict):
                    entries.pop(key, None)
        else:
            col, body = local[key]
            if shared and key in _entries(shared['doc']) and key not in main:
                finding('collision', key, key + ': the worktree uses an ID owned by the shared record')
            delta.setdefault(col, {})[key] = copy.deepcopy(body)
    if snapshot['working']['doc'].get('schema') != snapshot['ancestor']['doc'].get('schema'):
        finding('uncheckable', 'schema', 'The worktree changed the record schema; automatic compatibility is incomplete')
    hyps = [C.hypothesis('working-change', delta, {'folds': 'never'})]
    previous = {h['name']: h for h in snapshot['ancestor']['hypotheses']}
    for h in snapshot['working']['hypotheses']:
        body = {}
        if h != previous.get(h['name']):
            prior = _entries(previous.get(h['name'], {}).get('doc', {}))
            for key, (col, value) in _entries(h['doc']).items():
                if prior.get(key) != (col, value):
                    body.setdefault(col, {})[key] = value
        # Main can invalidate a standing hypothesis's condition even when its
        # authored entries did not change. Never replay its inherited values.
        hyps.append(C.hypothesis(h['name'], body, h['head']))
    # Compare deletion failures to untouched main, not to the already-deleted base.
    baseline = C._check_of(_doc(snapshot['main']['doc']))
    try:
        union = C.union_of(base, hyps, base_check=baseline)
    except (ValueError, SystemExit) as exc:
        finding('uncheckable', 'record', str(exc))
        union = C.Consolidation()
    for kind, lines in [('falsified', union.falsified), ('uncheckable', union.holes)]:
        for line in lines:
            finding(kind, line.split(':', 1)[0], line)
    for key, _, reason in union.refused:
        finding('contested', key, key + ': ' + reason)
    for key in union.contested:
        finding('collision', key, key + ': local hypotheses disagree')
    for h, predicate in union.head_falsified:
        finding('falsified', h['name'], h['name'] + ': ' + predicate)
    if union.doc is not None:
        prior_uncertain = uncertain(_doc(snapshot['main']['doc']))
        for key, reasons in uncertain(union.doc).items():
            if prior_uncertain.get(key) != reasons:
                finding('uncheckable', key, key + ': ' + '; '.join(reasons))
    unique = {digest(f): dict(f, fingerprint=digest(f)) for f in findings}
    return {'state': 'attention' if unique else 'clear', 'findings': list(unique.values()),
            'changed': changed, 'versions': snapshot['versions'], 'identity': snapshot['identity']}


def launch(watch):
    """The calling command only launches a detached processor, never waits for its analysis."""
    with open(os.devnull, 'rb') as inp, open(os.devnull, 'ab') as out:
        subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--workspace', str(watch.cwd),
                          '_process'], stdin=inp, stdout=out, stderr=out,
                         start_new_session=True, close_fds=True)


@contextlib.contextmanager
def try_lock(path):
    import fcntl
    I._private_dir(path.parent)
    fd = os.open(str(path), os.O_CREAT | os.O_RDWR, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        yield True
    finally:
        os.close(fd)


class Watch:
    def __init__(self, directory=None):
        self.cwd = Path(directory or os.getcwd()).resolve()
        self.tree = Path(git(self.cwd, 'rev-parse', '--show-toplevel')).resolve()
        common = Path(git(self.cwd, 'rev-parse', '--git-common-dir'))
        self.common = (self.cwd / common).resolve()
        found = workspace.locate(str(self.cwd))
        self.record = Path(found['record']).resolve()
        self.entry = self.record.relative_to(self.tree).as_posix()
        # Configuration and durable reports stay anchored to the first configured
        # supported entry name.  self.entry remains this checkout's discovered path,
        # so old- and new-name worktrees can safely share that storage.
        configured_entry = self.entry
        candidates = [self.entry]
        if self.record.name in workspace.NAMES:
            candidates += [self.record.with_name(name).relative_to(self.tree).as_posix()
                           for name in workspace.NAMES if name != self.record.name]
        config_dir = self.common / 'kpopper-watch'
        paths = [(candidate, config_dir / (digest(candidate) + '.json')) for candidate in candidates]
        self.config_path = paths[0][1]
        existing = [(candidate, path) for candidate, path in paths if path.exists()]
        if len(existing) > 1:
            names = ', '.join(candidate for candidate, _ in existing)
            raise ValueError('multiple watch configurations exist for supported record names (' + names +
                             '); reconcile their configuration and retained state before using watch')
        if existing:
            configured_entry, self.config_path = existing[0]
        if found['status'] != 'found' and self.config_path.exists():
            self.record = self.tree / configured_entry
            self.entry = configured_entry
        if found['status'] != 'found' and not self.config_path.exists():
            raise ValueError('watch needs an existing project record')
        self.key = digest(configured_entry)
        state = os.environ.get('XDG_STATE_HOME', '')
        home = Path(state) if os.path.isabs(state) else Path.home() / '.local/state'
        self.project_state = home / 'kpopper/watch' / digest([str(self.common), configured_entry])
        self.state = self.project_state / 'worktrees' / digest(str(self.tree))

    def config(self):
        config = I._load(self.config_path)
        # `entry` describes the checkout being operated on.  A serialized value can
        # be stale when another worktree is on the other side of the rename.
        return dict(config, entry=self.entry) if config else config

    def setup(self, base_ref=None, shared_record=None, shared_private=False):
        try:
            import fcntl  # noqa: F401 — refuse before creating even a private destination
        except ImportError as exc:
            raise ValueError('watch configuration requires POSIX file locking') from exc
        if not self.record.is_file():
            raise ValueError('configured branch record is unavailable; it was not recreated')
        config = self.config() or {}
        ref = base_ref or config.get('base_ref')
        if ref is None:
            for candidate in ('refs/remotes/origin/HEAD', 'refs/remotes/origin/main', 'refs/heads/main'):
                try:
                    git(self.tree, 'rev-parse', '--verify', candidate + '^{commit}')
                    ref = candidate
                    break
                except ValueError:
                    continue
        if not ref or ref.startswith('-'):
            raise ValueError('choose an existing base ref with --base-ref')
        resolved = git(self.tree, 'rev-parse', '--symbolic-full-name', '--verify', ref)
        if not resolved.startswith('refs/'):
            raise ValueError('base must name a branch or tracking ref, not a fixed commit')
        git(self.tree, 'rev-parse', '--verify', resolved + '^{commit}')
        if shared_record and shared_private:
            raise ValueError('choose one shared record destination')
        shared = str(Path(shared_record).expanduser().resolve()) if shared_record else config.get('shared_record')
        if shared_private:
            shared = str(self.project_state / 'shared/PROVENANCE.yaml')
        if shared:
            path = Path(shared)
            if path == self.record or self.tree in path.parents or self.common in path.parents:
                raise ValueError('shared facts must live outside Git checkouts and Git metadata')
            if config.get('shared_record') and config['shared_record'] != shared:
                raise ValueError('shared destination is already pinned; reconcile its queued reports before relocating')
            if config.get('shared_record') and not path.is_file():
                raise ValueError('configured shared record is unavailable; it was not recreated')
            if shared_record and not path.is_file():
                raise ValueError('the selected shared record must already exist; use --shared-private for a new destination')
            path.parent.mkdir(parents=True, exist_ok=True)
            try:
                other_common = Path(git(path.parent, 'rev-parse', '--git-common-dir'))
            except ValueError:
                pass
            else:
                if (path.parent / other_common).resolve() == self.common:
                    raise ValueError("shared facts must live outside this project's checkouts")
            with P._locked(str(path)):
                if not path.is_file() and (config.get('shared_record') or not shared_private):
                    raise ValueError('configured shared record is unavailable; it was not recreated')
                if not path.exists() and shared_private:
                    I._atomic(path, b'meta:\n  name: Shared external facts\nsources: {}\nknown: {}\n')
        with I._file_lock(self.config_path.with_suffix('.lock')):
            current = self.config() or {}
            config = dict(current, base_ref=resolved, shared_record=shared, enabled=True,
                          entry=self.entry, schema=1)
            I._save(self.config_path, config)
        return {'configured': True, **config, 'freshness': 'local Git objects; no implicit fetch'}

    def snapshot(self):
        config = self.config()
        if not config or not config.get('enabled'):
            raise ValueError('watch is not enabled; use kpopper watch setup')
        main = git(self.tree, 'rev-parse', '--verify', config['base_ref'] + '^{commit}')
        head = git(self.tree, 'rev-parse', '--verify', 'HEAD^{commit}')
        ancestor = git(self.tree, 'merge-base', head, main)
        working = _records(self.tree, self.entry)
        shared = None
        if config.get('shared_record'):
            p = Path(config['shared_record'])
            shared = _records(p.parent, p.name)
            try:
                from . import watch_shared as S
            except ImportError:
                import watch_shared as S
            inbox = digest(S.receipts(self))
        else:
            inbox = None
        versions = {'base_ref': config['base_ref'], 'main': main, 'head': head, 'merge_base': ancestor,
                    'working': working['hash'], 'shared': shared['hash'] if shared else None,
                    'inbox': inbox,
                    'config': digest(config), 'freshness': 'local Git objects; remote freshness not verified'}
        return {'working': working, 'ancestor': _records(self.tree, self.entry, ancestor),
                'main': _records(self.tree, self.entry, main), 'shared': shared,
                'versions': versions, 'identity': digest(versions)}

    def request(self):
        config = self.config()
        if not config or not config.get('enabled'):
            return {'state': 'disabled'}
        with I._file_lock(self.state / 'request.lock'):
            I._save(self.state / 'request.json', {'id': uuid.uuid4().hex, 'at': time.time()})
            I._save(self.state / 'location.json', {'cwd': str(self.cwd), 'tree': str(self.tree), 'entry': self.entry})
            active = I._load(self.state / 'active.json') or {}
            if active.get('until', 0) <= time.time():
                I._save(self.state / 'active.json', {'until': time.time() + 30})
                launch(self)
        return {'state': 'pending', 'workspace': str(self.cwd)}

    def request_all(self):
        results = [self.request()]
        for path in (self.project_state / 'worktrees').glob('*/location.json'):
            try:
                location = I._load(path)
                if not isinstance(location, dict) or location.get('tree') == str(self.tree):
                    continue
                peer = Watch(location['cwd'])
                if peer.common == self.common and peer.config_path == self.config_path:
                    results.append(peer.request())
            except (Exception, SystemExit) as exc:
                results.append({'state': 'unavailable', 'location': str(path), 'reason': str(exc)[:300]})
        return {'worktrees': results}

    def process(self):
        if not self.config() or not self.config().get('enabled'):
            return {'state': 'disabled'}
        with try_lock(self.state / 'processor.lock') as acquired:
            if not acquired:
                return {'state': 'busy'}
            for _ in range(3):
                with I._file_lock(self.state / 'request.lock'):
                    ticket = I._load(self.state / 'request.json')
                    I._save(self.state / 'active.json', {'until': time.time() + 30})
                before = None
                try:
                    if self.config().get('shared_record'):
                        try:
                            from . import watch_shared as S
                        except ImportError:
                            import watch_shared as S
                        if S.process(self):
                            self.request_all()
                    before = self.snapshot()
                    existing = I._load(self.state / 'result.json') or {}
                    result = existing if existing.get('identity') == before['identity'] and \
                        existing.get('state') in ('clear', 'attention') else compare(before)
                    if self.config().get('shared_record'):
                        result = dict(result, findings=[f for f in result['findings'] if f['kind'] != 'shared_review'])
                        for report in S.receipts(self):
                            if report['state'] == 'needs_review' and report['origin'] == str(self.tree):
                                f = {'kind': 'shared_review', 'id': report['target'], 'reason': report['reason'],
                                     'report': report['id']}
                                f['fingerprint'] = digest(f)
                                if f not in result['findings']:
                                    result['findings'].append(f)
                        result['state'] = 'attention' if result['findings'] else 'clear'
                    if self.snapshot()['identity'] != before['identity']:
                        continue
                except (Exception, SystemExit) as exc:
                    result = {'state': 'unavailable', 'reason': str(exc)[:1000], 'findings': [],
                              'identity': before['identity'] if before else None,
                              'versions': before['versions'] if before else None}
                previous = I._load(self.state / 'result.json') or {}
                result['episode'] = previous.get('episode') if previous.get('state') == result['state'] else uuid.uuid4().hex
                I._save(self.state / 'result.json', dict(result, checked_at=time.time()))
                with I._file_lock(self.state / 'request.lock'):
                    if I._load(self.state / 'request.json') == ticket:
                        I._save(self.state / 'active.json', {'until': 0})
                        return result
            I._save(self.state / 'active.json', {'until': 0})
            return {'state': 'pending'}

    def status(self):
        if not self.config() or not self.config().get('enabled'):
            return {'state': 'disabled'}
        result = I._load(self.state / 'result.json') or {'state': 'pending'}
        try:
            if self.config().get('shared_record'):
                try:
                    from . import watch_shared as S
                except ImportError:
                    import watch_shared as S
                if any(r['state'] == 'captured' for r in S.receipts(self)):
                    return {'state': 'pending', 'reason': 'shared reports are still being processed'}
            current = self.snapshot()
        except (Exception, SystemExit) as exc:
            return {'state': 'unavailable', 'reason': str(exc)[:1000]}
        if result.get('identity') != current['identity']:
            return {'state': 'pending', 'versions': current['versions']}
        return result

    def poll_result(self, previous=None):
        """Poll cheap private receipts; validate graph freshness once per completed pass."""
        raw = I._load(self.state / 'result.json')
        signature = raw.get('checked_at') if raw else None
        if signature is not None and signature != previous:
            return signature, self.status()
        return previous, None

    def offer(self, session, consume=True, result=None):
        result = self.status() if result is None else result
        if result['state'] not in ('attention', 'unavailable'):
            if result['state'] == 'clear' and consume:
                with I._file_lock(self.state / 'delivery.lock'):
                    I._save(self.state / 'delivery' / (digest(session) + '.json'), {})
            return None
        findings = result.get('findings') or [{'kind': 'unavailable', 'reason': result['reason'],
                                              'fingerprint': digest(result['reason'])}]
        path = self.state / 'delivery' / (digest(session) + '.json')
        with I._file_lock(self.state / 'delivery.lock'):
            receipt = I._load(path, {})
            offered = receipt.get('ids', []) if receipt.get('episode') == result.get('episode') else []
            fresh = [f for f in findings if f['fingerprint'] not in offered]
            current_ids = {f['fingerprint'] for f in findings}
            delivered = (set(offered) & current_ids) | {f['fingerprint'] for f in fresh[:8]}
            if consume:
                I._save(path, {'ids': sorted(delivered), 'episode': result.get('episode')})
            if not fresh:
                return None
        return {'workspace': str(self.cwd), 'versions': result.get('versions'), 'findings': fresh[:8],
                'remaining': max(0, len(fresh) - 8)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace')
    sub = parser.add_subparsers(dest='command', required=True)
    setup = sub.add_parser('setup')
    setup.add_argument('--base-ref')
    setup.add_argument('--shared-record')
    setup.add_argument('--shared-private', action='store_true')
    for name in ('status', '_process', 'pause'):
        sub.add_parser(name)
    scan = sub.add_parser('scan')
    scan.add_argument('--all', action='store_true', help='also queue previously registered worktrees')
    scan.add_argument('--notify-task')
    shared = sub.add_parser('share')
    shared.add_argument('--file', required=True)
    shared.add_argument('--notify-task')
    sub.add_parser('shared')
    resolve = sub.add_parser('resolve')
    resolve.add_argument('event_id')
    resolve.add_argument('--evidence', required=True)
    wait = sub.add_parser('wait-delivery')
    wait.add_argument('job_id')
    wait.add_argument('--timeout', type=float, default=100)
    complete = sub.add_parser('complete-delivery')
    complete.add_argument('job_id')
    complete.add_argument('--token', required=True)
    complete.add_argument('--outcome', choices=('sent', 'failed', 'unknown'), required=True)
    args = parser.parse_args(argv)
    try:
        watch = Watch(args.workspace)
        if getattr(args, 'notify_task', None):
            if args.notify_task != (os.environ.get('CODEX_SESSION_ID') or os.environ.get('CODEX_THREAD_ID')):
                raise ValueError('--notify-task must match the current host task identity')
            if not watch.config() or not watch.config().get('enabled'):
                raise ValueError('enable watch before reserving native delivery')
        if args.command == 'setup':
            result = watch.setup(args.base_ref, args.shared_record, args.shared_private)
        elif args.command == 'pause':
            with I._file_lock(watch.config_path.with_suffix('.lock')):
                config = watch.config() or {}
                I._save(watch.config_path, dict(config, enabled=False))
            result = {'state': 'disabled'}
        elif args.command in ('wait-delivery', 'complete-delivery'):
            try:
                from . import watch_delivery as D
            except ImportError:
                import watch_delivery as D
            result = D.wait(watch, args.job_id, args.timeout) if args.command == 'wait-delivery' else D.complete(watch, args.job_id, args.token, args.outcome)
        elif args.command in ('share', 'shared', 'resolve'):
            try:
                from . import watch_shared as S
            except ImportError:
                import watch_shared as S
            result = S.capture(watch, S.load_report(args.file)) if args.command == 'share' else \
                S.resolve(watch, args.event_id, args.evidence) if args.command == 'resolve' else S.read(watch)
        elif args.command == '_process':
            result = watch.process()
        else:
            result = (watch.request_all() if args.all else watch.request()) if args.command == 'scan' else watch.status()
        if getattr(args, 'notify_task', None):
            try:
                from . import watch_delivery as D
            except ImportError:
                import watch_delivery as D
            result['delivery_job'] = D.reserve(watch, args.notify_task)
        print(json.dumps(result, ensure_ascii=False, default=str, indent=2))
        return 0
    except (Exception, SystemExit) as exc:
        print(json.dumps({'error': str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
