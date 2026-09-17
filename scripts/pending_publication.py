"""Recoverable, scoped publication of immutable knowledge contributions.

Capture remains independent of provider latency. All remote facts are re-read;
receipts describe past observations, never proof of current acceptance. A managed
PR head is recyclable; refs/kpopper/pending_grounding is never changed here.
"""
import contextlib
import copy
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import time
import uuid

try:
    from . import pending_grounding as G, project_modes as M
    from .publication_provider import GitHubProvider, ProviderConflict, UnknownOutcome
except ImportError:
    import pending_grounding as G
    import project_modes as M
    from publication_provider import GitHubProvider, ProviderConflict, UnknownOutcome

TERMINAL = {'withdrawn', 'rejected', 'superseded'}
MAX_FAILURES = 5


class Busy(RuntimeError):
    pass


class Attention(ValueError):
    pass


def scope_identity(scope):
    return hashlib.sha256(G.json_bytes({k: scope[k] for k in ('remote', 'repository', 'target', 'branch')})).hexdigest()


class Publisher:
    def __init__(self, project, provider=None, clock=time.time):
        self.project = project if isinstance(project, M.Project) else M.Project(project)
        self.store = G.Store(self.project)
        self.provider = provider
        self.clock = clock
        self.path = self.project.state / 'publication.json'

    @contextlib.contextmanager
    def lock(self):
        try:
            import fcntl
        except ImportError as error:
            raise RuntimeError('publication writes require fcntl file locking; local status remains available') from error
        # Kernel ownership recovers on process death; a stale PID never grants a
        # second owner. This is local coordination, not a distributed remote lock.
        M.I._private_dir(self.project.state)
        with open(self.project.state / 'publisher.lock', 'a+b') as handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise Busy('another local publisher is active') from error
            try:
                yield
            finally:
                fcntl.flock(handle, fcntl.LOCK_UN)

    def _load(self):
        state = M.I._load(self.path)
        if state is None:
            return {'version': 1, 'scope': None, 'decisions': {}, 'receipts': [], 'expected_head': None,
                    'pr': None, 'cycle': 0, 'proposed': [], 'intent': None, 'failures': 0,
                    'retry_at': 0, 'paused': False, 'states': {}}
        if state.get('version') != 1:
            raise Attention('unknown publication receipt version')
        return state

    def _save(self, state, event, **details):
        state['receipts'].append({'event': event, 'at': self.clock(), **details})
        M.I._save(self.path, state)

    def authority(self):
        scope = self.project.config().get('publication')
        return {'configured': scope is not None, 'standing_permission': bool(scope and scope['standing_permission']),
                'scope': scope, 'actions': ['push_managed_branch', 'create_pull_request', 'update_pull_request']}

    def _scope(self):
        config = self.project.config()
        if config['mode'] != 'advanced':
            raise Attention('publication is inactive in Simple mode')
        scope = config.get('publication')
        if not scope:
            raise Attention('publication destination is not configured')
        urls = M.git(self.project.root, 'remote', 'get-url', '--push', '--all', scope['remote']).stdout.decode().splitlines()
        if scope['repository'].startswith('-') or urls != [scope['repository']]:
            raise Attention('configured push destination changed; reconcile the publication scope')
        for branch in (scope['target'], scope['branch']):
            if M.git(self.project.root, 'check-ref-format', '--branch', branch, check=False).returncode:
                raise Attention('invalid configured publication branch')
        if scope['target'] == scope['branch']:
            raise Attention('managed branch cannot be the target')
        M.relative_path(self.project.config()['record'])
        return scope

    def _authority_check(self, scope, authorized):
        current = self._scope()
        if scope_identity(current) != scope_identity(scope) or not (authorized or current['standing_permission']):
            raise Attention('publication authority was revoked or its destination changed')

    def _provider(self, scope):
        return self.provider or GitHubProvider(scope['repository'])

    def _remote(self, scope):
        try:
            result = M.git(self.project.root, 'ls-remote', '--refs', scope['repository'],
                           'refs/heads/' + scope['target'], 'refs/heads/' + scope['branch'])
        except (ValueError, subprocess.TimeoutExpired) as error:
            raise UnknownOutcome('remote heads are unavailable') from error
        heads = {line.split()[1]: line.split()[0] for line in result.stdout.decode().splitlines()}
        target = heads.get('refs/heads/' + scope['target'])
        if not target:
            raise Attention('configured target branch is unavailable')
        branch = heads.get('refs/heads/' + scope['branch'])
        # Pin exact read objects to durable recovery refs before using them. Never
        # fetch into the permanent capture ledger or a checkout branch.
        for label, oid in (('target', target), ('head', branch)):
            if oid:
                ref = 'refs/kpopper/publication/observed/' + label + '/' + oid
                try:
                    M.git(self.project.root, 'fetch', '--no-tags', scope['repository'], oid)
                    M.git(self.project.root, 'update-ref', ref, oid)
                except (ValueError, subprocess.TimeoutExpired) as error:
                    raise UnknownOutcome('remote observation changed or could not be fetched') from error
        return target, branch

    def _files(self, commit):
        result = {}
        for line in M.git(self.project.root, 'ls-tree', '-r', '-z', commit).stdout.split(b'\0'):
            if line:
                info, raw_path = line.split(b'\t', 1)
                mode, kind, oid = info.decode().split()
                result[raw_path.decode()] = (mode, kind, oid)
        return result

    def _blob_at(self, files, path, absent=None):
        if path not in files:
            return absent
        mode, kind, oid = files[path]
        if mode not in ('100644', '100755') or kind != 'blob':
            raise Attention('record or evidence path is not a regular file: ' + path)
        return self.store.blob(oid)

    def _history(self, files):
        """Validate the pinned target's complete committed history without Git checkout reads."""
        B = G.P._peer('history_bundle')
        layout = G.P.layout(self.project.record())
        paths = {role: Path(layout[role]).relative_to(self.project.root).as_posix()
                 for role in ('history_authority', 'history', 'history_commits')}
        total = 0
        def read(path, maximum=B.C.MAX_REQUEST_BYTES):
            nonlocal total
            if path not in files:
                return None
            size = int(M.git(self.project.root, 'cat-file', '-s', files[path][2]).stdout)
            total += size
            if size > maximum or total > B.MAX_BYTES:
                raise Attention('history target exceeds captured byte limit')
            return self._blob_at(files, path)
        marker_raw = read(paths['history_authority'], B.C.MAX_OBJECT_BYTES)
        if marker_raw is None:
            return None
        marker = B.C.validate_authority(B.C.decode_document(marker_raw))
        if marker['authority'] != 'history':
            return None
        commits, objects = {}, {}
        for path in files:
            if path.startswith(paths['history_commits'] + '/'):
                name = path[len(paths['history_commits']) + 1:]
                if '/' in name or not name.endswith('.yaml'):
                    raise Attention('invalid target history commit membership')
                commits[name[:-5]] = read(path)
            elif path.startswith(paths['history'] + '/'):
                parts = path[len(paths['history']) + 1:].split('/')
                if len(parts) != 2 or not parts[1].endswith('.yaml'):
                    raise Attention('invalid target history object membership')
                objects[(parts[0], parts[1][:-5])] = read(path, B.C.MAX_OBJECT_BYTES)
        selected = B.C.committed_objects(marker, commits, objects)
        raw = {'authority.yaml': marker_raw,
               'entry.yaml': read(self.project.config()['record'])}
        raw.update({'commits/' + operation + '.yaml': data for operation, data in commits.items()})
        raw.update({'objects/' + obj['subject'] + '/' + version + '.yaml': objects[(obj['subject'], version)]
                    for version, obj in selected.items()})
        B._files(raw)
        captured = B._capture(raw, None)
        # Imported source bytes remain audit evidence, never active graph inputs.
        imported = B.A.from_store_capture(captured).document.get('meta', {}).get('history_import')
        if imported is not None:
            parent = PurePosixPath(self.project.config()['record']).parent
            for item in imported['members']:
                name = B.C.relative_path(item['path'])
                content = read(str(parent / name))
                if content is None or B.C.sha256(content) != item['sha256']:
                    raise Attention('retained target history evidence is unavailable or changed: ' + name)
        return captured

    def _equivalent(self, bundle, doc, files):
        if bundle['manifest']['version'] != 3:
            if isinstance(doc.get('meta'), dict) and 'history' in doc['meta']:
                captured = self._history(files)
                if captured is None:
                    return False
                doc = G.P._peer('history_adapter').from_store_capture(captured).document
                doc['meta'].pop('history')
            return G.equivalent(bundle, doc, self._evidence(files, bundle))
        captured = self._history(files)
        if captured is None:
            return False
        observation = G.P._peer('knowledge_views').history_evidence(captured)
        return G.equivalent(bundle, doc, self._evidence(files, bundle), history=observation)

    def _graph(self, files):
        record = M.relative_path(self.project.config()['record'])
        raw = self._blob_at(files, record, b'{}')
        doc = G.P.yaml.safe_load(raw) or {}
        if not isinstance(doc, dict):
            raise Attention('target record is not a mapping')
        captured = self._history(files)
        if captured is not None:
            doc = G.P._peer('history_adapter').from_store_capture(captured).document
            self._validate_graph(files, doc)
            return doc
        # Publication is deliberately fail-closed for split records. Reading only
        # one part would incorrectly acknowledge acceptance or overwrite another.
        if doc.get('record') or doc.get('also'):
            raise Attention('split target record requires explicit publication reconciliation')
        self._validate_graph(files, doc)
        return doc

    def _target_layers(self, files):
        record = self.project.config()['record']
        directory = PurePosixPath(record).parent / G.P.hypotheses_rel(record)
        for path in sorted(files):
            if PurePosixPath(path).parent != directory or not path.endswith(('.yaml', '.yml')):
                continue
            layer = G.P.parse(text=self._blob_at(files, path).decode('utf-8')) or {}
            if not isinstance(layer, dict):
                raise Attention('target hypothesis is not a mapping: ' + path)
            layer.pop('hypothesis', None)
            if layer.get('record') or layer.get('also'):
                raise Attention('split target hypothesis requires explicit publication reconciliation')
            yield layer

    def _validate_graph(self, files, document):
        if isinstance(document.get('meta'), dict) and 'history' in document['meta']:
            captured = self._history(files)
            if captured is None or not G.P._peer('history_bundle').matches_document(captured, document):
                raise Attention('target history document lacks matching immutable evidence')
            document = copy.deepcopy(document)
            document['meta'].pop('history')
        G.document_capabilities(document)
        for layer in self._target_layers(files):
            G.document_capabilities(layer)
            base = G.P.Record()
            base.update(copy.deepcopy(document))
            G.document_capabilities(G.P.layered(base, {'doc': layer}))

    def _merge_capabilities(self, graph, incoming, files, snapshot, decisions):
        current = G.document_capabilities(graph)
        added = G.document_capabilities(incoming)
        if current['profile'] != added['profile']:
            if added['profile'] != 'core/v1':
                raise Attention('pending_profile_reconciliation_required: legacy content cannot inherit the target profile')
            authoring = G.P._peer('reasoning.authoring')
            if any(authoring._promotion_blockers(G.P, layer, graph)
                   for layer in [graph, *self._target_layers(files)]
                   if G.document_capabilities(layer)['profile'] != 'core/v1'):
                raise Attention('requires explicit target migration: existing executable legacy fields')
            for revision, bundle in snapshot['bundles'].items():
                if decisions.get(revision, {}).get('state') in TERMINAL:
                    continue
                if G.meaning_capabilities(bundle['manifest']['document']) != G.meaning_capabilities(incoming):
                    raise Attention('pending_profile_reconciliation_required: ' + revision)
        if added['profile'] == 'core/v1':
            declaration = copy.deepcopy(added)
            if current['profile'] == 'core/v1':
                declaration['version'] = max(current['version'], added['version'])
                declaration['requires'] = sorted(set(current['requires']) | set(added['requires']))
            meta = graph.get('meta')
            if meta is not None and not isinstance(meta, dict):
                raise Attention('requires explicit target migration: metadata is not a mapping')
            graph.setdefault('meta', {})['reasoning'] = declaration

    def _evidence(self, files, bundle):
        parent = PurePosixPath(self.project.config()['record']).parent
        return {name: data for name in bundle['manifest']['evidence']
                if (data := self._blob_at(files, str(parent / name))) is not None}

    def _accepted(self, snapshot, files, doc):
        decisions = self._load()['decisions']
        return {revision for revision, bundle in snapshot['bundles'].items()
                if decisions.get(revision, {}).get('state') not in TERMINAL
                if self._equivalent(bundle, doc, files)}

    @staticmethod
    def _marker(scope):
        return '<!-- kpopper-publication:' + scope_identity(scope) + ' -->'

    def _prs(self, scope):
        rows = self._provider(scope).list(scope)
        if not isinstance(rows, list) or any(row.get('state') not in ('open', 'closed', 'merged', 'rejected') for row in rows):
            raise UnknownOutcome('provider returned an incomplete publication state')
        active = [row for row in rows if row['state'] == 'open']
        if len(active) > 1:
            raise Attention('multiple pull requests use the configured managed branch')
        if active and self._marker(scope) not in active[0].get('body', ''):
            raise Attention('the managed branch belongs to an unrecognized pull request')
        return rows, active[0] if active else None

    def _reconcile(self, state, snapshot, scope):
        target, head = self._remote(scope)
        files = self._files(target)
        doc = self._graph(files)
        accepted = self._accepted(snapshot, files, doc)
        rows, active = self._prs(scope)
        previous = next((row for row in rows if str(row['id']) == str(state['pr'])), None)
        if state['pr'] and previous is None:
            raise UnknownOutcome('previous pull request is missing from the complete provider listing')
        if previous and previous['state'] in ('closed', 'rejected'):
            for revision in state['proposed']:
                if revision not in accepted and revision not in state['decisions']:
                    state['decisions'][revision] = {'state': previous['state'], 'pr': str(previous['id']),
                                                    'reason': 'provider reported ' + previous['state']}
        # Recover uncertain creation by the unique marker plus the intended cycle.
        intent = state.get('intent')
        if intent and intent['kind'] == 'create':
            matches = [r for r in rows if intent['marker'] in r.get('body', '')]
            if len(matches) > 1:
                raise Attention('multiple pull requests match an uncertain creation receipt')
            if matches:
                found = matches[0]
                state['pr'] = str(found['id'])
                state['proposed'] = intent['revisions']
                state['intent'] = None
                self._save(state, 'create_reconciled', pr=state['pr'])
                # Process terminal outcome on a subsequent bounded run, without a
                # new creation or treating a closed recovered PR as open.
                if found['state'] != 'open':
                    raise UnknownOutcome('recovered a completed pull request; reconcile its contributions on the next run')
            else:
                raise UnknownOutcome('pull-request creation remains uncertain; no second create was sent')
        if active and str(active['id']) != str(state['pr']):
            raise Attention('another publisher owns the active pull request')
        if intent and intent['kind'] == 'push':
            if head == intent['commit']:
                state['expected_head'] = head
                state['proposed'] = intent['revisions']
                state['intent'] = None
                self._save(state, 'push_reconciled', head=head)
            elif head != intent['expected']:
                raise Attention('remote branch changed during an uncertain push')
            # If still at the expected head the exact immutable push is safe to
            # retry with the same remote CAS, subject to normal retry bounds.
        if intent and intent['kind'] == 'update':
            state['intent'] = None
            self._save(state, 'update_reconciled', pr=intent['pr'])
        ended = previous and previous['state'] in ('merged', 'closed', 'rejected')
        if head != state['expected_head']:
            if not (ended and head is None):
                raise Attention('remote managed head changed; it was not overwritten')
        if ended:
            state['pr'] = None
            state['expected_head'] = head
            state['proposed'] = []
        states = {}
        for revision in snapshot['bundles']:
            decision = state['decisions'].get(revision, {})
            states[revision] = ('accepted' if revision in accepted else decision.get('state') or
                                ('paused' if state['paused'] else
                                 'proposed' if active and revision in state['proposed'] else 'captured'))
        state['states'] = states
        state['verified'] = {'target': target, 'head': head, 'ledger_ref': snapshot['ref'],
                             'scope': scope_identity(scope), 'at': self.clock()}
        self._save(state, 'reconciled', **state['verified'])
        return target, head, files, doc, active, accepted

    def _commit_history(self, target, files, doc, snapshot, revisions):
        """Propose an exact union within one authority, without inventing acts."""
        from dataclasses import replace
        B = G.P._peer('history_bundle')
        captured = self._history(files)
        if captured is None:
            raise Attention('history contribution requires explicit target history migration')
        commits, objects = dict(captured.commits), dict(captured.object_bytes)
        parent = PurePosixPath(self.project.config()['record']).parent
        layout = G.P.layout(self.project.record())
        additions = {}

        def add(path, raw):
            path = str(path)
            existing = additions.get(path, self._blob_at(files, path))
            if existing is not None and existing != raw:
                raise Attention('immutable history or evidence collision: ' + path)
            additions[path] = raw

        for revision in revisions:
            bundle = snapshot['bundles'][revision]
            G.validate_bundle(bundle)
            if bundle['manifest']['version'] != 3:
                raise Attention('history publication requires versioned history contributions')
            artifact = B.from_contribution(bundle)
            if artifact['manifest']['version'] == 2:
                raise Attention('scoped history contribution requires explicit adoption choices')
            incoming = B.validate(artifact)
            if G.identity(incoming.marker) != G.identity(captured.marker):
                raise Attention('different history authority requires explicit adoption')
            if G.identity(incoming.state['rules']) != G.identity(captured.state['rules']):
                raise Attention('history contribution rules disagree with target')
            for operation, raw in incoming.commits.items():
                if operation in commits and commits[operation] != raw:
                    raise Attention('immutable history operation collision: ' + operation)
                commits[operation] = raw
            for key, raw in incoming.object_bytes.items():
                if key in objects and objects[key] != raw:
                    raise Attention('immutable history object collision: ' + key[1])
                objects[key] = raw
            archive = parent / '.kpopper-contributions' / revision
            add(archive / 'manifest.json', G.json_bytes(G._encode(bundle['manifest'])))
            for name, raw in bundle['files'].items():
                add(archive / 'evidence' / name, raw)
                if not name.startswith(B.PREFIX):
                    add(parent / name, raw)
        selected = B.C.committed_objects(captured.marker, commits, objects)
        state = B.H.reduce(selected, rules=captured.state['rules'])
        merged = replace(captured, commits=commits, object_bytes=objects, objects=selected,
                         state=state, baseline=B.H.baseline(captured.marker, commits, state))
        rendered = object.__new__(B.H.Store).render(merged)
        generated = B.C.decode_document(rendered)
        merged = replace(merged, entry_bytes=rendered, document=generated)
        observation = G.P._peer('knowledge_views').history_evidence(merged)
        for operation, raw in commits.items():
            add(Path(layout['history_commits']).relative_to(self.project.root) / (operation + '.yaml'), raw)
        for (subject, version), raw in objects.items():
            add(Path(layout['history']).relative_to(self.project.root) / subject / (version + '.yaml'), raw)
        record = self.project.config()['record']
        if record in additions:
            raise Attention('evidence collides with history target entry')
        additions[record] = rendered
        adapted = B.A.from_store_capture(merged).document
        for revision in revisions:
            bundle = snapshot['bundles'][revision]
            evidence = {name: additions.get(str(parent / name), self._blob_at(files, str(parent / name)))
                        for name in bundle['manifest']['evidence'] if not name.startswith(B.PREFIX)}
            if not G.equivalent(bundle, adapted, evidence, history=observation):
                raise Attention('combined history does not retain a complete contribution')
        decisions = self._load()['decisions']
        for revision, prior in snapshot['bundles'].items():
            if revision in revisions or decisions.get(revision, {}).get('state') in TERMINAL:
                continue
            if self._equivalent(prior, doc, files):
                evidence = {name: additions.get(str(parent / name), self._blob_at(files, str(parent / name)))
                            for name in prior['manifest']['evidence'] if not name.startswith(B.PREFIX)}
                if prior['manifest']['version'] == 3:
                    retained = G.equivalent(prior, adapted, evidence, history=observation)
                else:
                    plain = copy.deepcopy(adapted)
                    plain['meta'].pop('history')
                    retained = G.equivalent(prior, plain, evidence)
                if not retained:
                    raise Attention('history union changes an accepted contribution')
        return self._write_commit(target, additions)

    def _commit(self, target, files, doc, snapshot, revisions):
        if any(snapshot['bundles'][revision]['manifest']['version'] == 3 for revision in revisions) or \
                isinstance(doc.get('meta'), dict) and 'history' in doc['meta']:
            return self._commit_history(target, files, doc, snapshot, revisions)
        self._validate_graph(files, doc)
        graph = copy.deepcopy(doc)
        current = G.entries(graph)
        additions = {}
        decisions = self._load()['decisions']
        replaceable = {}
        for old, decision in decisions.items():
            replacement = decision.get('replacement')
            if decision.get('state') == 'superseded' and replacement in revisions and old in snapshot['bundles']:
                prior = snapshot['bundles'][old]
                if G.equivalent(prior, doc, self._evidence(files, prior)):
                    replaceable.setdefault(replacement, []).append(prior)
        parent = PurePosixPath(self.project.config()['record']).parent
        # Conflicting revisions are obligations, not a sequence-based election.
        for revision in revisions:
            bundle = snapshot['bundles'][revision]
            G.validate_bundle(bundle)
            incoming = bundle['manifest']['document']
            self._merge_capabilities(graph, incoming, files, snapshot, decisions)
            # Empty collections are captured scope authority too. They have no
            # entries to reach the body merge, but must remain present in a target.
            for collection, members in incoming.items():
                if members == {} and collection not in ('meta', 'schema', 'record', 'also'):
                    graph.setdefault(collection, {})
            if 'schema' in incoming:
                if 'schema' in graph and G.identity(graph['schema']) != G.identity(incoming['schema']):
                    raise Attention('pending contribution conflicts with target schema')
                graph['schema'] = copy.deepcopy(incoming['schema'])
            for name, value in G.entries(incoming).items():
                collection, body = value
                if name in current and G.identity(list(current[name])) != G.identity(list(value)):
                    if not any(name in G.entries(prior['manifest']['document']) and
                               G.identity(list(current[name])) == G.identity(list(G.entries(prior['manifest']['document'])[name]))
                               for prior in replaceable.get(revision, [])):
                        raise Attention('pending revisions conflict; explicitly reconcile entry ' + name)
                graph.setdefault(collection, {})[name] = copy.deepcopy(body)
                current[name] = value
            for name, data in bundle['files'].items():
                path = str(parent / name)
                if path == self.project.config()['record']:
                    raise Attention('evidence collides with the target record')
                existing = additions.get(path, self._blob_at(files, path))
                if existing is not None and existing != data:
                    if not any(prior['files'].get(name) == existing for prior in replaceable.get(revision, [])):
                        raise Attention('pending evidence conflicts with target file: ' + path)
                additions[path] = data
        self._validate_graph(files, graph)
        record = self.project.config()['record']
        additions[record] = G.P.yaml.safe_dump(graph, allow_unicode=True, sort_keys=False).encode()
        for revision in revisions:
            bundle = snapshot['bundles'][revision]
            if not G.equivalent(bundle, graph, {name: additions[str(parent / name)] for name in bundle['files']}):
                raise Attention('combined graph changes a contribution meaning; reconcile before publishing')
        for old, prior in snapshot['bundles'].items():
            if old in revisions or decisions.get(old, {}).get('state') in TERMINAL:
                continue
            if not G.equivalent(prior, doc, self._evidence(files, prior)):
                continue
            evidence = {name: additions.get(str(parent / name), self._blob_at(files, str(parent / name)))
                        for name in prior['files']}
            if not G.equivalent(prior, graph, evidence):
                raise Attention('replacement would change another accepted contribution; reconcile its revision explicitly')
        return self._write_commit(target, additions)

    def _write_commit(self, target, additions):
        # A temporary index preserves modes, symlinks and submodules of the target;
        # no source checkout files or feature code enter the publication commit.
        index = self.project.state / ('publication-index-' + uuid.uuid4().hex)
        env = os.environ.copy()
        env['GIT_INDEX_FILE'] = str(index)
        try:
            M.git(self.project.root, 'read-tree', target, env=env)
            for path, data in additions.items():
                oid = self.store._write_blob(data)
                M.git(self.project.root, 'update-index', '--add', '--cacheinfo', '100644,' + oid + ',' + path, env=env)
            tree = M.git(self.project.root, 'write-tree', env=env).stdout.decode().strip()
            for role in ('AUTHOR', 'COMMITTER'):
                env['GIT_' + role + '_NAME'] = 'Knowledge record'
                env['GIT_' + role + '_EMAIL'] = 'knowledge@localhost'
            commit = M.git(self.project.root, 'commit-tree', '--no-gpg-sign', tree, '-p', target,
                           data=b'Propose pending project knowledge\n', env=env).stdout.decode().strip()
            M.git(self.project.root, 'update-ref', 'refs/kpopper/publication/prepared/' + commit, commit)
            return commit
        finally:
            index.unlink(missing_ok=True)
            index.with_name(index.name + '.lock').unlink(missing_ok=True)

    def _push(self, state, scope, intent, authorized):
        self._authority_check(scope, authorized)
        self._save(state, 'push_intended', **intent)
        expected = intent['expected'] or ''
        try:
            M.git(self.project.root, 'push', '--porcelain',
                  '--force-with-lease=refs/heads/' + scope['branch'] + ':' + expected,
                  scope['repository'], intent['commit'] + ':refs/heads/' + scope['branch'])
        except (ValueError, subprocess.TimeoutExpired) as error:
            raise UnknownOutcome('push result is unknown; the expected remote head will be reconciled') from error
        state['expected_head'] = intent['commit']
        state['proposed'] = intent['revisions']
        state['intent'] = None
        self._save(state, 'pushed', head=intent['commit'], revisions=intent['revisions'])

    def run(self, *, authorized=False, force_retry=False):
        """One bounded attempt. `authorized` is an explicit command's scoped grant.

        Default hooks never pass authorized=True. No merge, branch deletion,
        protection change, scheduler or authentication configuration is performed.
        """
        try:
            with self.lock():
                state = self._load()
                if not force_retry and (state['failures'] >= MAX_FAILURES or self.clock() < state['retry_at']):
                    return self._status(state, outcome='backoff')
                snapshot = self.store.snapshot()
                try:
                    scope = self._scope()
                    key = scope_identity(scope)
                    if state['scope'] not in (None, key):
                        raise Attention('publication receipts belong to another configured scope')
                    state['scope'] = key
                    target, head, files, doc, active, accepted = self._reconcile(state, snapshot, scope)
                    pending = [revision for revision in snapshot['bundles']
                               if revision not in accepted and revision not in state['decisions']]
                    remove_decided = bool(active and set(state['proposed']) & set(state['decisions']))
                    if state['paused'] or (not pending and not remove_decided) or not (authorized or scope['standing_permission']):
                        state['failures'], state['retry_at'] = 0, 0
                        self._save(state, 'idle')
                        return self._status(state, outcome='paused' if state['paused'] else 'idle')
                    if state.get('intent') and state['intent']['kind'] == 'push':
                        self._push(state, scope, state['intent'], authorized)
                    # Target+pending only, even when continuing the same cumulative PR.
                    # Skip an identical proposal to avoid moving heads on every scan.
                    if set(state['proposed']) != set(pending) or not head or not active:
                        commit = self._commit(target, files, doc, snapshot, pending)
                        state['intent'] = {'kind': 'push', 'commit': commit,
                                           'expected': state['expected_head'], 'revisions': pending}
                        self._push(state, scope, state['intent'], authorized)
                    marker = self._marker(scope)
                    title = 'Incorporate pending project knowledge'
                    body = marker + '\n\nPortable contributions for review:\n' + ''.join('- `' + r + '`\n' for r in pending)
                    self._authority_check(scope, authorized)
                    if active:
                        cycle_line = next((line for line in active.get('body', '').splitlines() if line.startswith('<!-- cycle:')), '')
                        if cycle_line:
                            body = body.replace(marker, marker + '\n' + cycle_line, 1)
                        state['intent'] = {'kind': 'update', 'pr': str(active['id'])}
                        self._save(state, 'update_intended', pr=str(active['id']))
                        row = self._provider(scope).update(scope, active['id'], title=title, body=body)
                        if row.get('state') != 'open':
                            raise UnknownOutcome('pull request ended during update; verify target acceptance')
                        state['intent'] = None
                        self._save(state, 'updated', pr=str(active['id']))
                    else:
                        state['cycle'] += 1
                        cycle_marker = marker + '\n<!-- cycle:' + str(state['cycle']) + ' -->'
                        body = body.replace(marker, cycle_marker, 1)
                        state['intent'] = {'kind': 'create', 'marker': cycle_marker, 'revisions': pending,
                                           'request_id': uuid.uuid4().hex}
                        self._save(state, 'create_intended', **state['intent'])
                        row = self._provider(scope).create(scope, title=title, body=body,
                                                           request_id=state['intent']['request_id'])
                        if row.get('state') != 'open':
                            raise UnknownOutcome('created pull request is already terminal; reconcile before proceeding')
                        state['pr'], state['intent'] = str(row['id']), None
                        self._save(state, 'created', pr=state['pr'])
                    state['states'].update({revision: 'proposed' for revision in pending})
                    state['failures'], state['retry_at'] = 0, 0
                    self._save(state, 'attempt_complete')
                    return self._status(state, outcome='proposed')
                except (UnknownOutcome, subprocess.TimeoutExpired, OSError) as error:
                    state['failures'] += 1
                    state['retry_at'] = self.clock() + min(300, 2 ** state['failures'])
                    self._save(state, 'unknown', detail=str(error))
                    return self._status(state, outcome='attention' if state['failures'] >= MAX_FAILURES else 'unknown', detail=str(error))
                except (Attention, ProviderConflict, ValueError) as error:
                    self._save(state, 'attention', detail=str(error))
                    return self._status(state, outcome='attention', detail=str(error))
        except Busy:
            return {'outcome': 'busy', 'verified': False}

    def _status(self, state, **extra):
        snapshot = self.store.snapshot()
        states = {revision: state['states'].get(revision, state['decisions'].get(revision, {}).get('state', 'captured'))
                  for revision in snapshot['bundles']}
        return dict(states=states, decisions=copy.deepcopy(state['decisions']), paused=state['paused'],
                    pr=state['pr'], expected_head=state['expected_head'], ledger_ref=snapshot['ref'],
                    retry_at=state['retry_at'], failures=state['failures'], intent=copy.deepcopy(state['intent']),
                    verified=False, last_verified=state.get('verified'), **extra)

    def status(self):
        """Local cached observations, explicitly not a fresh acceptance proof."""
        return self._status(self._load())

    def action(self, action, *, revisions=None, reason='', replacement=None):
        """Explicit local decisions. No remote PR is closed/deleted by this API."""
        if action not in ('pause', 'resume', 'withdraw', 'reject', 'supersede', 'retry'):
            raise ValueError('unknown publication action')
        with self.lock(), self.project.lock():
            state = self._load()
            snapshot = self.store.snapshot()
            chosen = list(revisions or [])
            if any(r not in snapshot['bundles'] for r in chosen):
                raise ValueError('decision must name a captured immutable revision')
            if action in ('withdraw', 'reject', 'supersede'):
                if not chosen or not reason.strip():
                    raise ValueError('terminal decisions need exact revisions and a reason')
                if action == 'supersede' and (replacement not in snapshot['bundles'] or replacement in chosen):
                    raise ValueError('supersession must name a different captured revision')
                for revision in chosen:
                    state['decisions'][revision] = {'state': {'withdraw': 'withdrawn', 'reject': 'rejected',
                                                            'supersede': 'superseded'}[action],
                                                     'reason': reason, 'replacement': replacement}
                    state['states'][revision] = state['decisions'][revision]['state']
            elif action == 'pause':
                state['paused'] = True
            elif action == 'resume':
                if chosen:
                    authoring = G.P._peer('reasoning.authoring')
                    paths = [str(self.project.record())]
                    document = authoring.load(G.P, paths) if self.project.record().exists() else {}
                    authoring.pending_compatible(G.P, paths, document, snapshot=snapshot,
                                                 decisions=state['decisions'], resume=chosen)
                    for revision in chosen:
                        state['decisions'].pop(revision, None)
                        state['states'][revision] = 'captured'
                state['paused'] = False
            else:
                state['failures'], state['retry_at'] = 0, 0
            self._save(state, 'decision', action=action, revisions=chosen, reason=reason, replacement=replacement)
            return self._status(state)

    def _verify_obligations(self):
        state = self._load()
        snapshot = self.store.snapshot()
        config = self.project.config()
        configured = config.get('publication')
        key = scope_identity(configured) if configured else None
        if state['scope'] not in (None, key):
            raise Attention('publication scope differs from its receipts')
        # A deliberate local disposal is content-bound and can be proved offline.
        # Provider-derived closed/rejected and any accepted receipt need live reads.
        terminal = {r: decision['state'] for r, decision in state['decisions'].items()
                    if r in snapshot['bundles'] and decision.get('state') in TERMINAL and not decision.get('pr')}
        unresolved = set(snapshot['bundles']) - set(terminal)
        target = None
        if unresolved or state.get('intent'):
            scope = self._scope()
            self._reconcile(state, snapshot, scope)
            terminal.update({r: value for r, value in state['states'].items()
                             if value == 'accepted' or value in TERMINAL or value == 'closed'})
            target = state['verified']['target']
        return dict(verified=True, ledger_ref=snapshot['ref'], scope=key,
                    generation=config['generation'], target=target,
                    terminal=terminal, unresolved=sorted(set(snapshot['bundles']) - set(terminal)),
                    checked_at=self.clock())

    def verify_obligations(self):
        """Fresh proof of content acceptance or explicit local terminal decisions.

        For mode transitions use transition_guard, which retains decision ownership
        until the caller checks ledger/configuration under the project policy lock.
        """
        with self.lock():
            return self._verify_obligations()

    @contextlib.contextmanager
    def transition_guard(self):
        """Network first; retain publisher ownership while caller locks policy.

        Required order: publisher -> policy -> record. Check proof ledger_ref,
        generation and scope under policy lock before changing mode. Never acquire
        this guard from inside a policy lock: capture must not wait on the network.
        """
        with self.lock():
            yield self._verify_obligations()


def trigger_after_capture(project):
    """Fire one detached bounded attempt; capture has already been acknowledged.

    This creates no schedule. A stopped host only retries when an authorized host
    invokes this function again, for example during a session opening or scan.
    """
    project = project if isinstance(project, M.Project) else M.Project(project)
    if not project.git or project.config()['mode'] != 'advanced' or not (project.config().get('publication') or {}).get('standing_permission'):
        return {'started': False, 'reason': 'no standing publication permission'}
    state = Publisher(project)._load()
    if state['paused'] or state['failures'] >= MAX_FAILURES or time.time() < state['retry_at']:
        return {'started': False, 'reason': 'paused or bounded backoff'}
    # An absolute script also works when the caller imports the source tree from
    # elsewhere and the target project's cwd cannot import that source package.
    args = [sys.executable, str(Path(__file__).resolve()), str(project.root)]
    try:
        process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL, start_new_session=True, cwd=str(project.root))
        return {'started': True, 'pid': process.pid}
    except OSError as error:
        return {'started': False, 'reason': str(error)}


def retry_session(project):
    return trigger_after_capture(project)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('usage: pending_publication PROJECT')
    Publisher(sys.argv[1]).run()
