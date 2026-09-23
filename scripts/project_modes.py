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
    from . import ingestion as I, workspace as W
except ImportError:
    import ingestion as I
    import workspace as W

MODES = ('simple', 'advanced')
PENDING_REF = 'refs/kpopper/pending_grounding'


def git(cwd, *args, data=None, check=True, env=None):
    proc = subprocess.run(['git', '-C', str(cwd), *args], input=data,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          timeout=30, env=W.git_environment(env))
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

    def _migration_evidence(self, destination, receipt, *, rollback=False):
        """Recompute the permitted conversion; a receipt alone cannot authorize it."""
        if __package__:
            from . import core_migration as C, pending_grounding as G
        else:
            C, G = I.P._peer('core_migration'), I.P._peer('pending_grounding')
        authoring = I.P._peer('reasoning.authoring')
        current = self.config()
        snapshots, signatures, plans, receipts = {}, set(), [], []
        for root in self.worktrees():
            source = (root / Path(current['record']).expanduser()).resolve()
            target = (root / Path(destination).expanduser()).resolve()
            if not source.is_file() or not target.is_file():
                raise ValueError('migration destination is unavailable in a participating worktree')
            original, candidate = (target, source) if rollback else (source, target)
            plan = C.prepare(original, route=False, read_mode='frozen' if rollback else 'live')
            if plan.problems:
                raise ValueError('migration cannot be validated: ' + '; '.join(map(str, plan.problems)))
            expected_entry = (candidate.parent / plan.mapping[str(plan.record)]).resolve()
            if candidate != expected_entry:
                raise ValueError('migration destination must be the complete mapped entry record')
            witness = Path(receipt) if root == self.root else candidate.parent / C.ARTIFACTS / 'receipt.json'
            if rollback:
                self._migration_reverse(plan, candidate.parent, witness)
            else:
                plan.validate_destination(candidate.parent, witness)
            # Reconciliation is separate from equivalence and remains required,
            # including accepted revisions which still attach to the live overlay.
            signatures.add(G.identity({plan.mapping[path]: hashlib.sha256(data).hexdigest()
                                       for path, data in plan.source_files.items()}))
            for path, data in plan.source_files.items():
                snapshots[str(path)] = hashlib.sha256(data).hexdigest()
            for path, data in plan.files.items():
                actual = candidate.parent / path
                snapshots[str(actual)] = hashlib.sha256(actual.read_bytes()).hexdigest()
            artifacts = candidate.parent / C.ARTIFACTS
            for path in artifacts.rglob('*'):
                if path.is_file():
                    snapshots[str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
            receipts.append(hashlib.sha256(witness.read_bytes()).hexdigest())
            plans.append(plan)
        if len(signatures) != 1:
            raise ValueError('worktrees have different migration source closures; reconcile before changing mode')
        for plan in plans:
            plan.source.verify()
        return {'snapshots': snapshots, 'receipts': receipts, 'rollback': rollback}

    @staticmethod
    def _migration_reverse(plan, candidate, witness):
        """Re-prove an exact restoration against retained originals and conversion.

        The old observation remains evidence of that time. Current configuration
        is validated by the surrounding transition, and can legitimately differ.
        No new core write or unretained original is silently downgraded.
        """
        if __package__:
            from . import core_migration as C, pending_grounding as G
            from .reasoning.snapshot import Snapshot
        else:
            C, G = I.P._peer('core_migration'), I.P._peer('pending_grounding')
            Snapshot = I.P._peer('reasoning.snapshot').Snapshot
        try:
            manifest = json.loads(Path(witness).read_bytes())
            for key in ('version', 'transformation', 'state', 'publication_authority', 'record', 'complete'):
                if G.identity(manifest[key]) != G.identity(plan.manifest[key]):
                    raise ValueError('rollback migration contract differs')
            originals = {row['path']: row['sha256'] for row in manifest['source']}
            expected = {plan.mapping[path]: hashlib.sha256(data).hexdigest() for path, data in plan.source_files.items()}
            if originals != expected or len(originals) != len(manifest['source']):
                raise ValueError('rollback original closure changed')
            inventory = manifest['destination']
            for name in inventory:
                relative_path(name)
            actual = {path.relative_to(candidate).as_posix(): path for path in candidate.rglob('*')
                      if path.is_file() or path.is_symlink()}
            if set(actual) != set(inventory) | {C.ARTIFACTS + '/receipt.json'}:
                raise ValueError('rollback candidate inventory changed')
            if actual[C.ARTIFACTS + '/receipt.json'].read_bytes() != Path(witness).read_bytes():
                raise ValueError('rollback receipt differs from the candidate receipt')
            for name, digest in inventory.items():
                if actual[name].is_symlink() or hashlib.sha256(actual[name].read_bytes()).hexdigest() != digest:
                    raise ValueError('rollback candidate bytes changed')
            for path, data in plan.source_files.items():
                if (candidate / C.ARTIFACTS / 'originals' / plan.mapping[path]).read_bytes() != data:
                    raise ValueError('rollback original evidence changed')
            for name, data in plan.files.items():
                if not name.startswith(C.ARTIFACTS + '/') and (candidate / name).read_bytes() != data:
                    raise ValueError('rollback is unavailable after further core authoring')
            original = Snapshot.from_json((candidate / C.ARTIFACTS / 'original.json').read_bytes())
            transformed = Snapshot.from_json((candidate / C.ARTIFACTS / 'candidate.json').read_bytes())
            if original.snapshot_id != manifest['source_snapshot_id'] or transformed.snapshot_id != manifest['candidate_snapshot_id']:
                raise ValueError('rollback snapshot identity differs')
            for retained, recomputed in ((original, plan.original), (transformed, plan.candidate)):
                old, now = retained.to_data(), recomputed.to_data()
                if any(G.identity(old[key]) != G.identity(now[key]) for key in ('document', 'hypotheses')):
                    raise ValueError('rollback snapshots differ from recomputed conversion')
            plan.source.verify()
        except (KeyError, TypeError, OSError, json.JSONDecodeError) as error:
            raise ValueError('invalid rollback migration evidence') from error

    def transition_report(self, mode, record=None, *, _pending_proof=None, migration_receipt=None, rollback=False):
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
        migration = None
        if rollback and not migration_receipt:
            raise ValueError('rollback requires a verified migration receipt')
        if changed and migration_receipt:
            migration = self._migration_evidence(destination, migration_receipt, rollback=rollback)
        targets = {(root / destination).resolve() for root in self.worktrees()}
        old_hashes = {h for h in snapshots.values() if h is not None}
        if changed:
            for target in sorted(targets):
                if not target.is_file():
                    blockers.append('destination record is unavailable; prepare it before changing mode: ' + str(target))
                    snapshots.setdefault(str(target), None)
                    continue
                target_hash = hashlib.sha256(target.read_bytes()).hexdigest()
                if old_hashes and target_hash not in old_hashes and migration is None:
                    blockers.append('destination record differs; reconcile its knowledge before changing mode: ' + str(target))
                snapshots.setdefault(str(target), target_hash)
                hypothesis_dir = Path(I.P.layout_of([str(target)])['hypotheses'])
                if hypothesis_dir.is_dir() and any(p.suffix in ('.yaml', '.yml') for p in hypothesis_dir.iterdir()):
                    blockers.append('destination hypotheses need reconciliation: ' + str(target))
        if changed:
            for path, digest in snapshots.items():
                if digest:
                    doc = I.P.yaml.safe_load(Path(path).read_bytes())
                    if isinstance(doc, dict) and any(doc.get(k) for k in ('record', 'also')) and migration is None:
                        blockers.append('multi-file record needs explicit reconciliation before changing mode: ' + path)
            authoring = I.P._peer('reasoning.authoring')
            for target in sorted(targets):
                if target.is_file():
                    document = authoring.load(I.P, [str(target)], read_mode='frozen')
                    I.P._peer('pending_grounding').document_capabilities(document)
                    authoring.pending_compatible(I.P, [str(self.record())], document, project=self,
                        mode='advanced' if 'advanced' in (mode, current['mode']) else 'simple')
        if migration is not None:
            snapshots.update(migration['snapshots'])
        return {'from': current, 'mode': mode, 'record': destination, 'changed': changed,
                'blockers': blockers if changed else [], 'snapshots': snapshots,
                'migration': {k: v for k, v in migration.items() if k != 'snapshots'} if migration else None}

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
            @contextlib.contextmanager
            def verified_guard():
                try:
                    with Publisher(self).transition_guard() as proof:
                        yield proof
                except (ValueError, RuntimeError) as error:
                    raise ValueError('durable contributions need reconciliation: ' + str(error)) from error
            guard = verified_guard()
        return guard

    def preview_transition(self, mode=None, record=None, *, migration_receipt=None, rollback=False):
        with self._transition_guard(mode, record) as proof:
            with self.lock():
                return self.transition_report(mode or self.config()['mode'], record, _pending_proof=proof,
                                              migration_receipt=migration_receipt, rollback=rollback)

    def configure(self, mode=None, record=None, expected_generation=None, *, migration_receipt=None, rollback=False):
        with self._transition_guard(mode, record) as proof:
            return self._configure(mode, record, expected_generation, proof, migration_receipt, rollback)

    def _configure(self, mode, record, expected_generation, proof, migration_receipt=None, rollback=False):
        with self.lock():
            current = self.config()
            if expected_generation is not None and current['generation'] != expected_generation:
                raise ValueError('project configuration changed; inspect the current policy')
            paths = [(root / Path(current['record']).expanduser()).resolve() for root in self.worktrees()]
            if record:
                paths.extend((root / Path(record).expanduser()).resolve() for root in self.worktrees())
            if migration_receipt:
                preliminary = self.transition_report(mode or current['mode'], record, _pending_proof=proof,
                                                     migration_receipt=migration_receipt, rollback=rollback)
                paths.extend(Path(path) for path in preliminary['snapshots'])
            # Lock order for routed writers: project policy, then record directories.
            # Existing file writers already honor the latter. Keep them out between
            # checking equality, retaining rollback bytes and committing the policy.
            directories = {str(path.parent) for path in paths if path.parent.is_dir()}
            # Acquire the complete set through the common canonical ordering.
            # pathlib's component ordering can disagree with the lock protocol's
            # string ordering for adjacent names such as .kpopper and
            # .kpopper-migration, even when both sets look individually sorted.
            with I.P._peer('history_transaction').directory_guards(directories, exclusive=True):
                report = self.transition_report(mode or current['mode'], record, _pending_proof=proof,
                                                migration_receipt=migration_receipt, rollback=rollback)
                if report['blockers']:
                    raise ValueError('; '.join(report['blockers']))
                if report['changed']:
                    # Rollback contains original bytes, not just hashes, and never enters Git.
                    backup = {'config': current, 'encoding': 'base64', 'migration': report['migration'],
                              'records': {p: base64.b64encode(Path(p).read_bytes()).decode('ascii') if h else None
                                                            for p, h in report['snapshots'].items()}}
                    I._save(self.state / 'mode-history' / (str(time.time_ns()) + '.json'), backup)
                if any((hashlib.sha256(Path(path).read_bytes()).hexdigest() if Path(path).is_file() else None) != expected
                       for path, expected in report['snapshots'].items()):
                    raise ValueError('record closure changed before configuration commit')
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
