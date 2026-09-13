"""Source-aware live views and portable, explicitly selected contribution snapshots.

The checkout remains the code-world. Pending bodies are named proposals; reading
never adopts them, chooses a newest revision, or updates a review snapshot.
"""
import copy
import os
from pathlib import Path
import tempfile

try:
    from . import pending_grounding as G, project_modes as M
except ImportError:
    import pending_grounding as G
    import project_modes as M


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


def overlay(paths, doc, *, read_mode='live'):
    doc.read_mode = read_mode
    doc.contributions, doc.knowledge_conflicts = [], {}
    if read_mode == 'frozen':
        return doc
    if read_mode != 'live':
        raise ValueError('read mode must be live or frozen')
    project = project_for(paths)
    if not project.git or project.config()['mode'] != 'advanced':
        return doc
    # Explicit unrelated files are frozen artifacts, never an implicit overlay target.
    if Path(paths[0]).resolve() != project.record():
        return doc
    snap = G.Store(project).snapshot()
    doc.pending_ref = snap['ref']
    if not snap['bundles']:
        return doc
    holders = {}
    meanings = {}
    def meaning(document, name):
        try:
            _, judgments, fields = G.P.infer(document)
            roles = {'judgment': name in judgments, 'fields': fields if name in judgments else {}}
        except SystemExit:
            roles = {'unreadable': True}
        return G.identity({'schema': {k: document[k] for k in ('schema',) if k in document}, 'roles': roles})
    for nid, pair in G.entries(doc).items():
        holders.setdefault(nid, []).append(('checkout', pair))
        meanings.setdefault(nid, set()).add(meaning(doc, nid))
    for name, hyp in doc.hypotheses.items():
        if not hyp['error']:
            for nid, pair in G.entries(hyp['doc']).items():
                holders.setdefault(nid, []).append(('hypothesis:' + name, pair))
                meanings.setdefault(nid, set()).add(meaning(G.P.layered(doc, hyp), nid))
    # Compare a configured target when locally available, without overlaying its code facts.
    publication = project.config().get('publication')
    if publication:
        ref = 'refs/remotes/' + publication['remote'] + '/' + publication['target']
        result = M.git(project.root, 'show', ref + ':' + project.config()['record'], check=False)
        if result.returncode == 0:
            target = G.P.yaml.safe_load(result.stdout) or {}
            for nid, pair in G.entries(target).items():
                holders.setdefault(nid, []).append(('target:' + ref, pair))
                meanings.setdefault(nid, set()).add(meaning(target, nid))
    for revision, bundle in snap['bundles'].items():
        manifest = bundle['manifest']
        name = 'pending-' + revision
        body = copy.deepcopy(manifest['document'])
        entry_map = G.entries(body)
        events = [dict(e) for e in snap['events'] if e['revision'] == revision]
        status = {'revision': revision, 'state': 'captured locally', 'scope': manifest['scope'],
                  'roots': manifest['roots'], 'events': events, 'ledger_ref': snap['ref']}
        doc.contributions.append(status)
        doc.hypotheses[name] = {
            'kind': 'contribution',
            'name': name, 'path': 'git:' + snap['ref'] + ':' + revision,
            'head': {'claim': 'Captured project contribution; acceptance has not been verified',
                     'folds': 'never', 'scope': manifest['scope'], 'publication': status},
            'doc': body, 'ids': set(entry_map),
            'raw': {nid: pair[1] for nid, pair in entry_map.items()}, 'error': None}
        for nid, pair in entry_map.items():
            holders.setdefault(nid, []).append((name, pair))
            meanings.setdefault(nid, set()).add(meaning(body, nid))
    pending_ids = {nid for b in snap['bundles'].values() for nid in G.entries(b['manifest']['document'])}
    for nid in pending_ids:
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
    return result


def materialize(project, revision, destination, *, ref=None):
    """Export complete immutable closure/evidence to a new directory, ready for review/CI.

    A conflict never overwrites existing artifacts. This export is explicit adoption
    preparation; it makes no claim about acceptance and never refreshes seen.
    """
    project = project if isinstance(project, M.Project) else M.Project(project)
    store = G.Store(project)
    pinned = ref or store.head()
    bundle = store.read_bundle(revision, pinned)
    destination = Path(destination).expanduser().resolve()
    if destination.exists():
        raise ValueError('snapshot destination already exists; select a new directory')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.knowledge-', dir=destination.parent) as temporary:
        root = Path(temporary) / 'snapshot'
        root.mkdir()
        document = copy.deepcopy(bundle['manifest']['document'])
        for path, data in bundle['files'].items():
            M.relative_path(path)
            if path in ('GROUNDING.yaml', 'snapshot.json'):
                raise ValueError('evidence conflicts with snapshot metadata')
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        (root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(document, allow_unicode=True, sort_keys=False), encoding='utf-8')
        (root / 'snapshot.json').write_bytes(G.json_bytes({'version': 1, 'revision': revision,
                                                        'ledger_ref': pinned, 'read_mode': 'frozen'}))
        # rename cannot replace a populated directory; no checkout content is changed.
        root.rename(destination)
    return {'state': 'materialized', 'revision': revision, 'record': str(destination / 'GROUNDING.yaml'),
            'read_mode': 'frozen', 'ledger_ref': pinned}
