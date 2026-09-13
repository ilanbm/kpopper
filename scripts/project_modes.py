"""One local project policy, shared by every checkout of a Git repository.

Configuration is private Git administration, never a branch-local preference. Reading
an unconfigured project has no side effects. A transition never moves record content.
"""
import base64
import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import time

try:
    from . import ingestion as I
except ImportError:
    import ingestion as I

MODES = ('simple', 'advanced')
PENDING_REF = 'refs/kpopper/pending_grounding'


def git(cwd, *args, data=None, check=True, env=None):
    proc = subprocess.run(['git', '-C', str(cwd), *args], input=data,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=30, env=env)
    if check and proc.returncode:
        raise ValueError(proc.stderr.decode('utf-8', 'replace').strip() or 'Git operation failed')
    return proc


def relative_path(value):
    """Portable Git/evidence paths; never an option, escape or administrative file."""
    if not isinstance(value, str) or not value or '\\' in value or any(ord(c) < 32 for c in value):
        raise ValueError('expected a portable relative path')
    p = PurePosixPath(value)
    if p.is_absolute() or str(p) != value or any(x in ('.', '..', '.git') for x in p.parts):
        raise ValueError('path must stay inside the project, outside .git')
    return value


class Project:
    def __init__(self, cwd=None):
        self.cwd = Path(cwd or Path.cwd()).expanduser().resolve()
        if not self.cwd.is_dir():
            raise ValueError('project directory is unavailable')
        result = git(self.cwd, 'rev-parse', '--show-toplevel', check=False)
        self.git = result.returncode == 0
        if self.git:
            self.root = Path(os.fsdecode(result.stdout).strip()).resolve()
            common = git(self.root, 'rev-parse', '--git-common-dir').stdout
            self.common = (self.root / os.fsdecode(common).strip()).resolve()
            self.state = self.common / 'kpopper' / 'project'
        else:
            # The first existing project policy/record in our ancestry owns Simple.
            self.root = self.cwd
            for path in (self.cwd, *self.cwd.parents):
                if path == Path.home() or path == Path(path.anchor):
                    break
                if any((path / x).exists() for x in ('.kpopper/project.json', 'GROUNDING.yaml', 'PROVENANCE.yaml')):
                    self.root = path
                    break
            self.common = None
            self.state = self.root / '.kpopper'
        self.config_path = self.state / 'project.json'

    def lock(self):
        return I._file_lock(self.state / 'project.lock')

    def worktrees(self):
        if not self.git:
            return [self.root]
        result = git(self.root, 'worktree', 'list', '--porcelain', '-z').stdout
        return [Path(os.fsdecode(field[9:])) for field in result.split(b'\0')
                if field.startswith(b'worktree ') and Path(os.fsdecode(field[9:])).is_dir()]

    def defaults(self):
        roots = self.worktrees()
        name = next((name for name in ('GROUNDING.yaml', 'PROVENANCE.yaml')
                     if any((root / name).exists() for root in roots)), 'GROUNDING.yaml')
        record = name
        mode = 'advanced' if self.git else 'simple'
        if self.git and (self.common / 'kpopper-record').exists():
            record = (self.common / 'kpopper-record').read_text(encoding='utf-8').strip()
            if not record:
                raise ValueError('registered record path is empty; restore it before configuring')
            registered = (self.root / Path(record).expanduser()).resolve()
            if any((root / name).exists() and (root / name).resolve() != registered for root in roots):
                raise ValueError('branch records and a different registered record both exist; reconcile their routing explicitly')
            if Path(record).expanduser().is_absolute():
                # An absolute registered pointer already makes all checkouts share
                # one record. Preserve that behavior, including an unavailable file.
                record = str(Path(record).expanduser().resolve())
                mode = 'simple'
            else:
                try:
                    relative_path(record)
                except ValueError as error:
                    raise ValueError('registered relative path is not portable; reconcile registration before configuring') from error
        if not self.git:
            record = str((self.root / record).resolve())
        return {'version': 1, 'mode': mode,
                'record': record, 'publication': None, 'generation': 0}

    def config(self):
        value = I._load(self.config_path)
        if value is None:
            return self.defaults()
        if not isinstance(value, dict) or type(value.get('version')) is not int or value.get('version') != 1 \
                or value.get('mode') not in MODES or type(value.get('generation')) is not int \
                or value['generation'] < 0 or not isinstance(value.get('record'), str) or not value['record'].strip():
            raise ValueError('unrecognized project configuration; it was not replaced')
        if value['mode'] == 'advanced':
            if not self.git:
                raise ValueError('Advanced mode requires Git')
            relative_path(value['record'])
        publication = value.get('publication')
        if publication is not None:
            if not isinstance(publication, dict) or set(publication) != {
                    'remote', 'repository', 'target', 'branch', 'standing_permission'} \
                    or type(publication['standing_permission']) is not bool \
                    or any(not isinstance(publication[k], str) or not publication[k].strip()
                           for k in ('remote', 'repository', 'target', 'branch')):
                raise ValueError('invalid publication configuration; no authority was inferred')
        return value

    def record(self, config=None):
        cfg = config or self.config()
        return (self.root / Path(cfg['record']).expanduser()).resolve()

    def _settled_pending(self, proof, config):
        if not proof or proof.get('verified') is not True or proof.get('unresolved'):
            return False
        if __package__:
            from . import pending_grounding as G, pending_publication as U
        else:
            import pending_grounding as G
            import pending_publication as U
        snapshot = G.Store(self).snapshot()
        scope = config.get('publication')
        return (proof.get('ledger_ref') == snapshot['ref']
                and proof.get('generation') == config['generation']
                and proof.get('scope') == (U.scope_identity(scope) if scope else None)
                and set(proof.get('terminal', {})) == set(snapshot['bundles'])
                and all(state in {'accepted', 'withdrawn', 'rejected', 'superseded', 'closed'}
                        for state in proof['terminal'].values()))

    def transition_report(self, mode, record=None, *, _pending_proof=None):
        if mode not in MODES or (not self.git and mode != 'simple'):
            raise ValueError('mode needs a compatible Git or Simple project')
        current = self.config()
        destination = record or current['record']
        if self.git and mode != 'simple':
            relative_path(destination)
        elif mode == 'simple':
            destination = str((self.root / Path(destination).expanduser()).resolve())
            if self.git and (mode != current['mode'] or destination != current['record']):
                for root in self.worktrees():
                    try:
                        Path(destination).relative_to(root)
                    except ValueError:
                        continue
                    raise ValueError('Simple in Git needs an external shared record; checkout files belong to their branch')
        blockers, snapshots = [], {}
        for root in self.worktrees():
            path = (root / Path(current['record']).expanduser()).resolve()
            if path.is_file():
                snapshots[str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
            elif path.exists():
                blockers.append('record is not a file: ' + str(path))
            else:
                snapshots[str(path)] = None
            layout = I.P.layout_of([str(path)])
            hyp = layout.get('hypotheses')
            if hyp and Path(hyp).is_dir() and any(p.suffix in ('.yaml', '.yml') for p in Path(hyp).iterdir()):
                blockers.append('hypotheses need reconciliation: ' + str(path))
        if len(set(snapshots.values())) > 1:
            blockers.append('worktrees have different or missing records; reconcile before changing mode')
        if self.git and git(self.root, 'rev-parse', '--verify', PENDING_REF, check=False).returncode == 0 \
                and not self._settled_pending(_pending_proof, current):
            blockers.append('durable contributions exist; reconcile their publication before changing mode')
        changed = mode != current['mode'] or destination != current['record']
        targets = {(root / destination).resolve() for root in self.worktrees()}
        old_hashes = {h for h in snapshots.values() if h is not None}
        if changed:
            for target in sorted(targets):
                if not target.is_file():
                    blockers.append('destination record is unavailable; prepare it before changing mode: ' + str(target))
                    snapshots.setdefault(str(target), None)
                    continue
                target_hash = hashlib.sha256(target.read_bytes()).hexdigest()
                if old_hashes and target_hash not in old_hashes:
                    blockers.append('destination record differs; reconcile its knowledge before changing mode: ' + str(target))
                snapshots.setdefault(str(target), target_hash)
                hypothesis_dir = Path(I.P.layout_of([str(target)])['hypotheses'])
                if hypothesis_dir.is_dir() and any(p.suffix in ('.yaml', '.yml') for p in hypothesis_dir.iterdir()):
                    blockers.append('destination hypotheses need reconciliation: ' + str(target))
        if changed:
            for path, digest in snapshots.items():
                if digest:
                    doc = I.P.yaml.safe_load(Path(path).read_bytes())
                    if isinstance(doc, dict) and any(doc.get(k) for k in ('record', 'also')):
                        blockers.append('multi-file record needs explicit reconciliation before changing mode: ' + path)
        return {'from': current, 'mode': mode, 'record': destination, 'changed': changed,
                'blockers': blockers if changed else [], 'snapshots': snapshots}

    def _transition_guard(self, mode, record):
        current = self.config()
        changed = (mode is not None and mode != current['mode']) or \
                  (record is not None and record != current['record'])
        guard = contextlib.nullcontext(None)
        if changed and self.git and git(self.root, 'rev-parse', '--verify', PENDING_REF, check=False).returncode == 0:
            if __package__:
                from .pending_publication import Publisher
            else:
                from pending_publication import Publisher
            # Remote verification finishes before the policy lock is acquired.
            # Retain publisher ownership so a terminal decision cannot be resumed
            # between verification and the local transition.
            guard = Publisher(self).transition_guard()
        return guard

    def preview_transition(self, mode=None, record=None):
        with self._transition_guard(mode, record) as proof:
            with self.lock():
                return self.transition_report(mode or self.config()['mode'], record, _pending_proof=proof)

    def configure(self, mode=None, record=None, expected_generation=None):
        with self._transition_guard(mode, record) as proof:
            return self._configure(mode, record, expected_generation, proof)

    def _configure(self, mode, record, expected_generation, proof):
        with self.lock():
            current = self.config()
            if expected_generation is not None and current['generation'] != expected_generation:
                raise ValueError('project configuration changed; inspect the current policy')
            paths = [(root / Path(current['record']).expanduser()).resolve() for root in self.worktrees()]
            if record:
                paths.extend((root / Path(record).expanduser()).resolve() for root in self.worktrees())
            # Lock order for routed writers: project policy, then record directories.
            # Existing file writers already honor the latter. Keep them out between
            # checking equality, retaining rollback bytes and committing the policy.
            with contextlib.ExitStack() as locks:
                for directory in sorted({path.parent for path in paths if path.parent.is_dir()}):
                    locks.enter_context(I.P._locked(str(directory / 'record')))
                report = self.transition_report(mode or current['mode'], record, _pending_proof=proof)
                if report['blockers']:
                    raise ValueError('; '.join(report['blockers']))
                if report['changed']:
                    # Rollback contains original bytes, not just hashes, and never enters Git.
                    backup = {'config': current, 'encoding': 'base64',
                              'records': {p: base64.b64encode(Path(p).read_bytes()).decode('ascii') if h else None
                                                            for p, h in report['snapshots'].items()}}
                    I._save(self.state / 'mode-history' / (str(time.time_ns()) + '.json'), backup)
                value = dict(current, mode=report['mode'], record=report['record'],
                             generation=current['generation'] + 1)
                I._save(self.config_path, value)
                return value

    def publication_config(self, remote, target, branch='pending_grounding', grant=False):
        """Persist exact publication scope. Calling this does not push or start a worker."""
        if not self.git:
            raise ValueError('publication needs Git')
        if type(grant) is not bool:
            raise ValueError('publication permission must be explicitly true or false')
        for ref in (target, branch):
            if git(self.root, 'check-ref-format', '--branch', ref, check=False).returncode:
                raise ValueError('invalid publication branch')
        if target == branch:
            raise ValueError('publication branch must differ from its target')
        if remote not in git(self.root, 'remote').stdout.decode().splitlines():
            raise ValueError('publication remote is not configured in this repository')
        urls = git(self.root, 'remote', 'get-url', '--push', '--all', remote).stdout.decode().splitlines()
        if len(urls) != 1:
            raise ValueError('publication needs exactly one push destination')
        scope = {'remote': remote, 'repository': urls[0], 'target': target, 'branch': branch,
                 'standing_permission': bool(grant)}
        with self.lock():
            cfg = self.config()
            old = cfg.get('publication')
            if old and old != scope and git(self.root, 'rev-parse', '--verify', PENDING_REF, check=False).returncode == 0:
                # Grant/revocation is allowed; destination changes require reconciliation.
                if any(old[k] != scope[k] for k in ('remote', 'repository', 'target', 'branch')):
                    raise ValueError('pending store belongs to the existing publication scope')
            I._save(self.config_path, dict(cfg, publication=scope, generation=cfg['generation'] + 1))
        return scope
