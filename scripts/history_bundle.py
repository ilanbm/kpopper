"""Explicit, bounded, source-free transport of complete committed history.

Commit manifests are indivisible: an export refuses when their complete ancestry
carries a subject outside the explicitly authorized roots. Staging
objects and the private capture inventory never travel. Digests establish
consistency of captured evidence, not authenticity or publication permission.
"""
import copy

from . import history_contract as C, history_store as H, history_adapter as A
from . import pending_grounding as G

CAPABILITY = 'history-closure/v1'
MAX_BYTES = H.MAX_CAPTURE_BYTES
PREFIX = 'history-closure/'


def _files(files):
    C._require(isinstance(files, dict) and len(files) <= 2 * C.MAX_OBJECTS + 2,
               'history_limit')
    total = 0
    for path, raw in files.items():
        C.relative_path(path)
        C._require(type(raw) is bytes, 'invalid_bytes')
        total += len(raw)
        C._require(len(raw) <= C.MAX_REQUEST_BYTES and total <= MAX_BYTES, 'history_limit')
    G._portable_files(files)


def _authorization(captured, roots, scope, shareability):
    C._require(shareability == 'project', 'history_not_shareable')
    C._require(isinstance(roots, list) and roots and roots == sorted(set(roots)),
               'invalid_history_roots')
    for subject in roots:
        C._text(subject, C.SUBJECT)
    C._require(isinstance(scope, dict) and scope.get('kind') in ('project', 'external', 'code')
               and isinstance(scope.get('environment'), str) and scope['environment'].strip(),
               'invalid_history_scope')
    if scope['kind'] == 'code':
        import re
        C._require(re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', str(scope.get('commit', ''))),
                   'invalid_history_scope')
    G._privacy(scope)
    subjects = {obj['subject'] for obj in captured.objects.values()}
    C._require(set(roots) <= subjects, 'invalid_history_roots')
    C._require(set(roots) == subjects, 'history_export_requires_full_authorization',
               ', '.join(sorted(subjects - set(roots))))
    document = A.from_store_capture(captured).document
    for subject, (_, body) in G.entries(document).items():
        C._require(G.identity(body.get('scope')) == G.identity(scope),
                   'history_scope_mismatch', subject)
    return document


def _capture(files, rules):
    marker = C.validate_authority(C.decode_document(files['authority.yaml']))
    document = C.decode_document(files['entry.yaml'])
    commits, objects = {}, {}
    for path, raw in files.items():
        parts = path.split('/')
        if len(parts) == 2 and parts[0] == 'commits' and parts[1].endswith('.yaml'):
            op = parts[1][:-5]
            C._text(op)
            commits[op] = raw
        elif len(parts) == 3 and parts[0] == 'objects' and parts[2].endswith('.yaml'):
            subject, version = parts[1], parts[2][:-5]
            C._text(subject, C.SUBJECT)
            C._text(version, C.OBJECT_ID)
            objects[(subject, version)] = raw
        else:
            C._require(path in ('authority.yaml', 'entry.yaml'), 'invalid_history_bundle_path')
    selected = C.committed_objects(marker, commits, objects)
    C._require(set(objects) == {(o['subject'], vid) for vid, o in selected.items()},
               'history_bundle_membership', 'uncommitted objects are not portable authority')
    state = H.reduce(selected, rules=rules)
    baseline = H.baseline(marker, commits, state)
    C.bind_authority(marker, document.get('meta', {}).get('history'))
    captured = H.Capture(files['entry.yaml'], document, marker, commits, objects,
                         selected, state, baseline, {}, authority_bytes=files['authority.yaml'])
    # These methods consume only the supplied evidence; never open a store.
    store = object.__new__(H.Store)
    rendered = store.render(captured)
    C._require(store._known_view(captured) or
               G.identity(C.decode_document(rendered)) == G.identity(document),
               'unresolved_view_edit')
    for role, kind in (('heads', 'claim'), ('open_acts', 'act')):
        for subject, versions in document['meta']['history'][role].items():
            for version in versions:
                obj = selected.get(version)
                C._require(obj is not None and obj['subject'] == subject and
                           (obj['kind'] == 'act') == (kind == 'act'),
                           'incomplete_view_baseline')
    return captured


def validate(artifact):
    """Validate complete bytes/membership and return detached committed capture."""
    C._require(isinstance(artifact, dict) and set(artifact) == {'revision', 'manifest', 'files'},
               'invalid_history_bundle')
    manifest = C.detached(artifact['manifest'], C.MAX_REQUEST_BYTES)
    C._mapping(manifest, ('version', 'requires', 'roots', 'scope', 'shareability', 'rules',
                          'baseline', 'files'))
    C._require(type(manifest['version']) is int and manifest['version'] == 1
               and manifest['requires'] == [CAPABILITY], 'unsupported_history_bundle')
    C._require(G.identity(manifest) == artifact['revision'], 'history_bundle_identity')
    files = artifact['files']
    _files(files)
    C._require(isinstance(manifest['files'], dict) and set(files) == set(manifest['files'])
               and {'authority.yaml', 'entry.yaml'} <= set(files), 'history_bundle_membership')
    for path, raw in files.items():
        C._require(C.sha256(raw) == manifest['files'][path], 'history_bundle_checksum', path)
        G._privacy(C.decode_document(raw))
    captured = _capture(files, manifest['rules'])
    C._require(G.identity(captured.state['rules']) == G.identity(manifest['rules']), 'rules_mismatch')
    C._require(G.identity(captured.baseline) == G.identity(manifest['baseline']), 'baseline_mismatch')
    _authorization(captured, manifest['roots'], manifest['scope'], manifest['shareability'])
    return captured


def export(captured, *, roots, scope, shareability, authority_bytes=None):
    """Export an already captured complete closure, with explicit sharing consent.

    The exact authority bytes must be retained by the reader or explicitly
    supplied, and must match its private captured inventory. No live reads occur.
    """
    C._require(isinstance(roots, (list, tuple)) and roots and all(isinstance(s, str) for s in roots),
               'invalid_history_roots')
    raw = authority_bytes if authority_bytes is not None else getattr(captured, 'authority_bytes', None)
    C._require(type(raw) is bytes, 'missing_captured_authority_bytes')
    C._require(G.identity(C.decode_document(raw)) == G.identity(captured.marker), 'authority_mismatch')
    digests = {digest for (kind, _), digest in captured.inventory.items() if kind == 'bytes'}
    C._require(C.sha256(raw) in digests, 'uncaptured_authority_bytes')
    files = {'authority.yaml': raw, 'entry.yaml': captured.entry_bytes}
    files.update({'commits/' + op + '.yaml': data for op, data in captured.commits.items()})
    files.update({'objects/' + obj['subject'] + '/' + vid + '.yaml':
                  captured.object_bytes[(obj['subject'], vid)] for vid, obj in captured.objects.items()})
    _files(files)
    manifest = {'version': 1, 'requires': [CAPABILITY], 'roots': sorted(set(roots)),
                'scope': copy.deepcopy(scope), 'shareability': shareability,
                'rules': copy.deepcopy(captured.state['rules']), 'baseline': copy.deepcopy(captured.baseline),
                'files': {path: C.sha256(data) for path, data in sorted(files.items())}}
    artifact = {'revision': G.identity(manifest), 'manifest': manifest, 'files': files}
    validate(artifact)
    return artifact


def adapt(artifact):
    """Replay using only captured portable bytes, preserving original profiles."""
    return A.from_store_capture(validate(artifact))


def from_contribution(bundle):
    """Extract the v3 manifest-bound artifact; validation remains explicit."""
    binding = bundle['manifest']['history']
    return {'revision': binding['revision'], 'manifest': binding['manifest'],
            'files': {path: bundle['files'][PREFIX + path]
                      for path in binding['manifest']['files'] if PREFIX + path in bundle['files']}}


def matches_document(captured, document):
    """Match an observed, regenerated or explicitly adapted view of this capture."""
    rendered = C.decode_document(object.__new__(H.Store).render(captured))
    adapted = A.from_store_capture(captured).document
    return G.identity(document) in {G.identity(captured.document), G.identity(rendered), G.identity(adapted)}
