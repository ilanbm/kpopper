"""Source-aware live views and portable, explicitly selected contribution snapshots.

The checkout remains the code-world. Pending bodies are named proposals; reading
never adopts them, chooses a newest revision, or updates a review snapshot.
"""
import copy
import ctypes
import os
from pathlib import Path
import sys
import tempfile

try:
    from . import pending_grounding as G, project_modes as M, provenance as P
except ImportError:
    import pending_grounding as G
    import project_modes as M
    import provenance as P


def project_for(paths):
    first = Path(paths[0]).expanduser().resolve()
    project = M.Project(first.parent)
    if project.git:
        return project
    # Only an explicitly configured caller can own an external shared record.
    current = M.Project()
    if current.git and current.config_path.exists() and current.record() == first:
        return current
    return project


def write_paths(paths):
    """A configured Simple project has one record even for legacy root-file callers."""
    project = project_for(paths)
    first = Path(paths[0]).resolve()
    if project.config_path.exists() and project.config()['mode'] == 'simple' and first in {
            project.root / 'GROUNDING.yaml', project.root / 'PROVENANCE.yaml', project.record()}:
        return [str(project.record()), *paths[1:]]
    return paths


def has_pending(paths):
    project = project_for(paths)
    return project.git and project.config()['mode'] == 'advanced' and Path(paths[0]).resolve() == project.record() and G.Store(project).head() is not None


def history_evidence(captured):
    """Complete committed observation, independent of contribution sharing scope."""
    B = P._peer('history_bundle')
    C = B.C
    files = {'authority.yaml': captured.authority_bytes, 'entry.yaml': captured.entry_bytes}
    files.update({'commits/' + op + '.yaml': raw for op, raw in captured.commits.items()})
    files.update({'objects/' + obj['subject'] + '/' + version + '.yaml':
                  captured.object_bytes[(obj['subject'], version)]
                  for version, obj in captured.objects.items()})
    result = {'version': 1, 'files': files, 'sha256': {path: C.sha256(raw) for path, raw in files.items()},
              'rules': copy.deepcopy(captured.state['rules']), 'baseline': copy.deepcopy(captured.baseline),
              'projection': B.A.from_store_capture(captured).projection}
    validate_history_evidence(result)
    return result


def validate_history_evidence(evidence):
    """Pure captured target/member replay, without sharing or acceptance inference."""
    B = P._peer('history_bundle')
    C = B.C
    C._mapping(evidence, ('version', 'files', 'sha256', 'rules', 'baseline', 'projection'))
    C._require(type(evidence['version']) is int and evidence['version'] == 1,
               'unsupported_history_observation')
    files = evidence['files']
    B._files(files)
    C._require(set(files) == set(evidence['sha256']) and {'entry.yaml', 'authority.yaml'} <= set(files),
               'history_bundle_membership')
    C._require(all(C.sha256(raw) == evidence['sha256'][path] for path, raw in files.items()),
               'history_bundle_checksum')
    captured = B._capture(files, evidence['rules'])
    C._require(G.identity(captured.baseline) == G.identity(evidence['baseline']), 'baseline_mismatch')
    adapted = B.A.from_store_capture(captured)
    C._require(G.identity(adapted.projection) == G.identity(evidence['projection']), 'projection_mismatch')
    return captured


def overlay(paths, doc, *, read_mode='live'):
    doc.read_mode = read_mode
    doc.contributions, doc.knowledge_conflicts = [], {}
    doc.history_contributions = {}
    if read_mode == 'frozen':
        return doc
    if read_mode != 'live':
        raise ValueError('read mode must be live or frozen')
    project = project_for(paths)
    if G.P._CAPTURE_READS.get() is not None:
        doc.capture_config = copy.deepcopy(project.config())
    doc.private_drafts = G.P._peer('recording').private_drafts(project)
    if not project.git or project.config()['mode'] != 'advanced':
        return doc
    # Explicit unrelated files are frozen artifacts, never an implicit overlay target.
    if Path(paths[0]).resolve() != project.record():
        return doc
    snap = G.Store(project).snapshot()
    doc.pending_ref = snap['ref']
    # Preserve immutable evidence for portable strict capture.
    doc.pending_snapshot = copy.deepcopy(snap)
    holders = {}
    meanings = {}
    active_ids = set()
    contract = P._peer('reasoning.contract')

    def capability_of(document):
        try:
            return contract.capabilities(document)
        except contract.CapabilityError as error:
            raise P.Refused(error.code + ': ' + str(error)) from None

    def meaning(document, name, history=None):
        plain = document
        authority = None
        if isinstance(document.get('meta'), dict) and 'history' in document['meta']:
            if history is None:
                raise P.Refused('missing_history_context: contribution comparison needs captured history')
            P._peer('history_contract').CapturedHistory(document, history)
            plain = copy.deepcopy(dict(document))
            plain['meta'].pop('history')
            authority = history['authority']
        capability = G.meaning_capabilities(plain)
        result = G.semantic_roles(document)
        if result is not None:
            judgments, fields = result
            roles = {'judgment': name in judgments, 'fields': fields if name in judgments else {}}
        else:
            roles = {'unreadable': True}
        return G.identity({'schema': {k: document[k] for k in ('schema',) if k in document},
                           'roles': roles, 'reasoning': capability,
                           **({'history_authority': authority} if authority else {})})
    for nid, pair in G.entries(doc).items():
        holders.setdefault(nid, []).append(('checkout', pair))
        meanings.setdefault(nid, set()).add(meaning(doc, nid, getattr(doc, 'history_projection', None)))
    for name, hyp in doc.hypotheses.items():
        if not hyp['error']:
            for nid, pair in G.entries(hyp['doc']).items():
                holders.setdefault(nid, []).append(('hypothesis:' + name, pair))
                meanings.setdefault(nid, set()).add(meaning(G.P.layered(doc, hyp), nid, getattr(doc, 'history_projection', None)))
    # Compare a configured target when locally available, without overlaying its code facts.
    publication = project.config().get('publication')
    cache = {}
    if __package__:
        from .pending_publication import Publisher, scope_identity
    else:
        from pending_publication import Publisher, scope_identity
    cache = Publisher(project).status()
    doc.publication = cache
    observed = cache.get('last_verified') or {}
    if publication:
        ref = 'refs/remotes/' + publication['remote'] + '/' + publication['target']
        if observed.get('scope') == scope_identity(publication) and observed.get('target'):
            ref = observed['target']
        doc.knowledge_target = {'ref': ref, 'revision': None}
        resolved = M.git(project.root, 'rev-parse', '--verify', ref + '^{commit}', check=False)
        if resolved.returncode == 0:
            doc.knowledge_target['revision'] = resolved.stdout.decode().strip()
            try:
                # The existing bounded reader follows committed pointers and named
                # hypotheses; no private working files or target code are imported.
                W = G.P._peer('watch')
                # watch can use the independently imported ingestion reader. Pass
                # only this consumer's capability permission and restore it after
                # reading; committed target bytes are pinned by the Git revision.
                core_token = W.P._CORE_READS.set(P._CORE_READS.get())
                try:
                    target = W._records(project.root, project.config()['record'], resolved.stdout.decode().strip(), include_files=True)
                finally:
                    W.P._CORE_READS.reset(core_token)
                # Validate the complete target before interpreting any of its entries.
                # _records currently uses P.load too; keep this comparison boundary
                # explicit so a different bounded reader cannot bypass its guard.
                for document in [target['doc'], *(hyp['doc'] for hyp in target['hypotheses'])]:
                    capability = capability_of(document)
                    if capability['profile'] == contract.PROFILE and not P._CORE_READS.get():
                        raise P.Refused('unsupported_capability: use core/v1 consumer')
                doc.knowledge_target_snapshot = copy.deepcopy(target)
                target_docs = [('target:' + ref, target['doc'])]
                for hyp in target['hypotheses']:
                    layered = copy.deepcopy(target['doc'])
                    for collection, members in G.P.collections_of(hyp['doc']).items():
                        layered.setdefault(collection, {}).update(copy.deepcopy(members))
                    if 'schema' in hyp['doc']:
                        layered['schema'] = copy.deepcopy(hyp['doc']['schema'])
                    target_docs.append(('target:' + ref + ':hypothesis:' + hyp['name'], layered))
                for label, target_doc in target_docs:
                    for nid, pair in G.entries(target_doc).items():
                        holders.setdefault(nid, []).append((label, pair))
                        meanings.setdefault(nid, set()).add(meaning(target_doc, nid,
                            target.get('history', {}).get('projection')))
            except (ValueError, OSError, SystemExit) as error:
                doc.target_unavailable = str(error)
        else:
            doc.target_unavailable = 'configured target has no locally available observation'
    for revision, bundle in snap['bundles'].items():
        manifest = bundle['manifest']
        name = 'pending-' + revision
        body = copy.deepcopy(manifest['document'])
        history = None
        if manifest['version'] == 3:
            B = P._peer('history_bundle')
            G.validate_bundle(bundle)
            artifact = B.from_contribution(bundle)
            adapted = B.adapt(artifact)
            body, history = adapted.document, adapted.projection
            doc.history_contributions[revision] = {
                'artifact_revision': artifact['revision'], 'projection': history,
                'scope': copy.deepcopy(manifest['scope']), 'roots': list(manifest['roots']), 'status': 'active'}
        entry_map = G.entries(body)
        events = [dict(e) for e in snap['events'] if e['revision'] == revision]
        status = {'revision': revision, 'state': 'captured locally', 'scope': manifest['scope'],
                  'roots': manifest['roots'], 'events': events, 'ledger_ref': snap['ref']}
        state = cache.get('states', {}).get(revision, 'captured')
        status.update(publication_state=state, verified=False,
                      last_verified=observed or None, pr=cache.get('pr'))
        if state != 'captured':
            status['state'] = state + (' (last observed)' if state in ('accepted', 'proposed', 'closed') else '')
        doc.contributions.append(status)
        decision = cache.get('decisions', {}).get(revision, {})
        if decision.get('state') in ('withdrawn', 'rejected', 'superseded'):
            if history is not None:
                doc.history_contributions[revision]['status'] = 'retired'
            # Explicit decisions retire an active proposal, not its immutable
            # evidence or publication history. Sequence alone never retires it.
            continue
        try:
            G.validate_bundle(bundle)
        except ValueError as error:
            raise P.Refused(getattr(error, 'code', 'invalid_contribution') + ': ' + str(error)) from None
        active_ids.update(entry_map)
        doc.hypotheses[name] = {
            'kind': 'contribution',
            'name': name, 'path': 'git:' + snap['ref'] + ':' + revision,
            'head': {'claim': 'Project contribution: ' + status['state'] + '; current remote acceptance is unverified',
                     'folds': 'never', 'scope': manifest['scope'], 'publication': status},
            'doc': body, 'ids': set(entry_map),
            'raw': {nid: pair[1] for nid, pair in entry_map.items()}, 'error': None}
        for nid, pair in entry_map.items():
            holders.setdefault(nid, []).append((name, pair))
            meanings.setdefault(nid, set()).add(meaning(body, nid, history))
    for nid in active_ids:
        variants = holders.get(nid, [])
        if len({G.identity(list(pair)) for _, pair in variants}) > 1 or len(meanings.get(nid, ())) > 1:
            doc.knowledge_conflicts[nid] = [(name, copy.deepcopy(pair[1])) for name, pair in variants]
    return doc


def lines(doc):
    contributions = getattr(doc, 'contributions', [])
    result = ['PENDING ' + c['revision'][:12] + ' · ' + c['state'] + ' · ' +
              c['scope']['kind'] + ': ' + c['scope']['environment'] + ' · ' + ', '.join(c['roots'])
              for c in contributions]
    result.extend('CONFLICT ' + nid + ': ' + ', '.join(name for name, _ in variants)
                  for nid, variants in sorted(getattr(doc, 'knowledge_conflicts', {}).items()))
    if getattr(doc, 'target_unavailable', None):
        result.append('TARGET UNVERIFIED: ' + doc.target_unavailable)
    if getattr(doc, 'private_drafts', None):
        result.append(str(len(doc.private_drafts)) + ' private drafts retained; inspect `kpopper knowledge status`')
    return result


def _rename_absent(source, destination):
    """One filesystem operation, refusing even an empty destination created late."""
    if os.name == 'nt':
        os.rename(source, destination)  # Windows rename never replaces an existing path.
        return
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == 'darwin':
        rename = library.renamex_np
        rename.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
        result = rename(os.fsencode(source), os.fsencode(destination), 0x00000004)  # RENAME_EXCL
    elif sys.platform.startswith('linux') and hasattr(library, 'renameat2'):
        rename = library.renameat2
        rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
        result = rename(-100, os.fsencode(source), -100, os.fsencode(destination), 1)  # NOREPLACE
    else:
        raise ValueError('atomic absent-destination publication is unavailable on this platform')
    if result:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), str(destination))


def publish_tree(destination, populate, validate=None):
    """Build on the destination filesystem, validate, and publish into absence."""
    destination = Path(destination).expanduser().absolute()
    if os.path.lexists(destination):
        raise FileExistsError('snapshot destination already exists; select a new directory')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.knowledge-', dir=destination.parent) as temporary:
        staging = Path(temporary) / 'snapshot'
        staging.mkdir()
        populate(staging)
        if validate is not None:
            validate(staging)
        _rename_absent(staging, destination)
    return destination


def materialize(project, revision, destination, *, ref=None):
    """Export complete immutable closure/evidence to a new directory, ready for review/CI.

    A conflict never overwrites existing artifacts. This export is explicit adoption
    preparation; it makes no claim about acceptance and never refreshes seen.
    """
    project = project if isinstance(project, M.Project) else M.Project(project)
    store = G.Store(project)
    pinned = ref or store.head()
    bundle = store.read_bundle(revision, pinned)
    G.validate_bundle(bundle)
    destination = Path(destination).expanduser().resolve()
    def populate(root):
        document = copy.deepcopy(bundle['manifest']['document'])
        for path, data in bundle['files'].items():
            M.relative_path(path)
            if path in ('GROUNDING.yaml', 'snapshot.json'):
                raise ValueError('evidence conflicts with snapshot metadata')
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        if bundle['manifest']['version'] == 3:
            B = P._peer('history_bundle')
            captured = B.validate(B.from_contribution(bundle))
            layout = P.layout(root / 'GROUNDING.yaml')
            images = {Path(layout['history_authority']): captured.authority_bytes}
            images.update({Path(layout['history_commits']) / (op + '.yaml'): raw
                           for op, raw in captured.commits.items()})
            images.update({Path(layout['history']) / subject / (version + '.yaml'): raw
                           for (subject, version), raw in captured.object_bytes.items()})
            images[root / 'GROUNDING.yaml'] = object.__new__(B.H.Store).render(captured)
            for path, raw in images.items():
                if path.exists():
                    raise ValueError('evidence conflicts with history snapshot authority')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
        else:
            (root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(document, allow_unicode=True, sort_keys=False), encoding='utf-8')
        metadata = {'version': 1, 'revision': revision, 'ledger_ref': pinned, 'read_mode': 'frozen'}
        if bundle['manifest']['version'] == 3:
            metadata.update(version=3, contribution=G._encode(bundle['manifest']))
        (root / 'snapshot.json').write_bytes(G.json_bytes(metadata))
    def validate(root):
        if bundle['manifest']['version'] == 3:
            B = P._peer('history_bundle')
            captured = B.H.Store(root / 'GROUNDING.yaml').capture()
            expected = B.adapt(B.from_contribution(bundle))
            actual = B.A.from_store_capture(captured)
            P._peer('reasoning.snapshot').Snapshot.capture(root / 'GROUNDING.yaml', read_mode='frozen')
            if G.identity(actual.document) != G.identity(expected.document) or G.identity(actual.projection) != G.identity(expected.projection):
                raise ValueError('materialized history does not reproduce captured contribution')
    try:
        publish_tree(destination, populate, validate=validate)
    except FileExistsError as error:
        raise ValueError('snapshot destination already exists; select a new directory') from error
    return {'state': 'materialized', 'revision': revision, 'record': str(destination / 'GROUNDING.yaml'),
            'read_mode': 'frozen', 'ledger_ref': pinned}
