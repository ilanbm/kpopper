"""The live overlay infers per document, not per entry, and means exactly what it meant.

F1 hoists the per-document half of `knowledge_views.overlay`'s nested `meaning()` out of the
per-entry loops. The win is a call count; the obligation is that every meaning, and every
document a meaning is computed for, is what the per-entry code produced.

The oracle below (`old_meaning`) is a verbatim transcription of that nested function as it
stood at e888570, before this lane touched it. It is deliberately NOT expressed in terms of
the new helpers: a control computed from its subject cannot disagree with it.
"""
import copy
import json
from pathlib import Path
import subprocess
import types
import unittest
from unittest.mock import patch

from scripts import provenance as P, project_modes as M, pending_grounding as G
from scripts import knowledge_views as V
from tests.test_pending_grounding import Repository, fixture_bundle

W = G.P._peer('watch')

# A judgment in the fixture keeps `name in judgments` true for at least one entry, so the
# `roles['fields']` arm of _meaning_of - the only place the entry name is read, and the only
# place the now-shared `fields` object enters the identity - is exercised rather than dead.
JUDGMENT = {'c.fits': {'rests_on': ['api.limit'], 'seen': {'api.limit': 10},
                       'verdict': 'fits', 'wrong_if': 'api.limit < 0'}}


def old_meaning(document, name, history=None):
    """`knowledge_views.overlay.meaning` verbatim at e888570, the commit this lane branched from.

    Recover it with: git show e888570:scripts/knowledge_views.py | sed -n '118,137p'
    """
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


BASE_COMMIT = 'e888570'

# Every output EPIC decision 1 requires to be unchanged in content for every input.
OVERLAY_OUTPUTS = ('read_mode', 'knowledge_conflicts', 'contributions', 'pending_ref',
                   'pending_snapshot', 'publication', 'knowledge_target', 'target_unavailable',
                   'private_drafts', 'history_contributions', 'hypotheses')


def pre_hoist_overlay():
    """`overlay` as it stood at BASE_COMMIT, loaded from git rather than transcribed.

    Reading it out of history rather than copying it into this file means the oracle cannot
    drift toward its subject as the subject is edited: there is nothing here to edit.
    """
    root = Path(__file__).resolve().parents[1]
    shown = subprocess.run(['git', 'show', BASE_COMMIT + ':scripts/knowledge_views.py'],
                           cwd=str(root), capture_output=True)
    if shown.returncode != 0:
        raise unittest.SkipTest('history for ' + BASE_COMMIT + ' is unavailable here')
    module = types.ModuleType('scripts.knowledge_views_pre_hoist')
    module.__package__ = 'scripts'
    module.__file__ = str(root / 'scripts' / 'knowledge_views_pre_hoist.py')
    exec(compile(shown.stdout, BASE_COMMIT + ':scripts/knowledge_views.py', 'exec'),
         module.__dict__)
    return module.overlay


def normalize(value):
    """A deterministic, total shape for comparing two overlay outputs.

    Total on purpose: anything this does not understand becomes its repr rather than being
    skipped, so a difference cannot slip through a type the comparison forgot to handle.
    """
    if isinstance(value, dict):
        return {str(k): normalize(v) for k, v in sorted(value.items(), key=lambda kv: str(kv[0]))}
    if isinstance(value, (list, tuple)):
        return [normalize(item) for item in value]
    if isinstance(value, (set, frozenset)):
        return sorted(repr(normalize(item)) for item in value)
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return repr(value)


class Counter:
    """Counts calls through a module attribute without changing what it returns."""

    def __init__(self, module, name):
        self.module, self.name = module, name
        self.real = getattr(module, name)
        self.calls = []

    def __enter__(self):
        def counted(*args, **kw):
            # Retaining the arguments keeps every counted object alive for the whole window,
            # which is what makes `id()` distinctness meaningful to the callers below.
            self.calls.append(args)
            return self.real(*args, **kw)
        setattr(self.module, self.name, counted)
        return self

    def __exit__(self, *exc):
        setattr(self.module, self.name, self.real)

    @property
    def count(self):
        return len(self.calls)


class OverlayFixture(Repository):
    """A record that exercises every document kind overlay compares: the checkout, a named
    hypothesis, a pending contribution whose ids overlap the checkout, and a committed target."""

    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.document = fixture_bundle()['manifest']['document']
        self.document['judgments'] = copy.deepcopy(JUDGMENT)
        self.record.write_text(P.yaml.safe_dump(self.document))
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')
        self.capture()
        project = M.Project(self.root)
        config = {**project.config(), 'mode': 'advanced', 'publication': {
            'remote': 'origin', 'repository': 'https://example.test/repo.git',
            'target': 'trunk', 'branch': 'pending', 'standing_permission': False}}
        project.state.mkdir(parents=True, exist_ok=True)
        project.config_path.write_text(json.dumps(config))
        M.git(self.root, 'update-ref', 'refs/remotes/origin/trunk', 'HEAD')

    def hypothesis(self, name, doc):
        path = Path(P.hypothesis_path([str(self.record)], name))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(P.yaml.safe_dump(doc))
        return path

    def target(self, document=None, *, hypotheses=(), history=None):
        target = {'doc': copy.deepcopy(self.document if document is None else document),
                  'hypotheses': list(hypotheses), 'hash': 'fixture'}
        if history is not None:
            target['history'] = history
        return target

    def load(self, target=None):
        token = P._CORE_READS.set(True)
        try:
            if target is None:
                return P.load([str(self.record)])
            with patch.object(W, '_records', return_value=target):
                return P.load([str(self.record)])
        finally:
            P._CORE_READS.reset(token)


class InfersPerDocument(OverlayFixture):
    def test_checkout_entries_share_one_inference(self):
        """Before F1 this ran G.semantic_roles once per entry of every document compared."""
        with Counter(G, 'semantic_roles') as roles:
            doc = self.load()
        self.assertTrue(len(G.entries(doc)) > 1, 'fixture must hold several entries to be a test')
        self.assertTrue(roles.calls, 'nothing was counted; the patch missed the reader')
        self.assertEqual(roles.count, len({id(args[0]) for args in roles.calls}),
                         'semantic_roles ran more than once for some document')

    def test_hypothesis_layers_the_record_once_not_once_per_entry(self):
        """`layered` built a fresh Record per entry, which also defeated any per-document reuse."""
        self.hypothesis('alternate', {'known': {'api.limit': {'v': 9, 'from': 's.vendor'},
                                                'api.other': {'v': 3, 'from': 's.vendor'}}})
        with Counter(P, 'layered') as layered:
            self.load()
        self.assertEqual(layered.count, 1)

    def test_no_document_is_inferred_more_than_once_with_every_holder_present(self):
        self.hypothesis('alternate', {'known': {'api.limit': {'v': 9, 'from': 's.vendor'}}})
        target = self.target(hypotheses=[{'name': 'core', 'doc': copy.deepcopy(self.document),
                                          'head': {}}])
        with Counter(G, 'semantic_roles') as roles:
            self.load(target)
        self.assertTrue(roles.calls, 'nothing was counted; the patch missed the reader')
        self.assertEqual(roles.count, len({id(args[0]) for args in roles.calls}))


class MeansWhatItMeant(OverlayFixture):
    """The differential: every meaning the new overlay computes equals the pre-hoist one."""

    def recorded(self, target=None):
        seen, contexts = [], {}
        real_context, real_of = V._meaning_context, V._meaning_of

        def context(document, history=None):
            result = real_context(document, history)
            contexts[id(result)] = (document, history)
            return result

        def of(ctx, name):
            document, history = contexts[id(ctx)]
            result = real_of(ctx, name)
            seen.append((document, history, name, result))
            return result

        with patch.object(V, '_meaning_context', context), patch.object(V, '_meaning_of', of):
            doc = self.load(target)
        return doc, seen

    def test_every_meaning_equals_the_pre_hoist_computation(self):
        self.hypothesis('alternate', {'known': {'api.limit': {'v': 9, 'from': 's.vendor'}}})
        target = self.target(hypotheses=[{'name': 'core', 'doc': copy.deepcopy(self.document),
                                          'head': {}}])
        doc, seen = self.recorded(target)
        self.assertTrue(seen, 'the fixture computed no meanings at all')
        for document, history, name, result in seen:
            self.assertEqual(result, old_meaning(document, name, history),
                             'meaning changed for ' + name)

    def test_a_contested_id_is_still_contested(self):
        """Hand-written expectation, not derived: the hypothesis disagrees about api.limit."""
        self.hypothesis('alternate', {'known': {'api.limit': {'v': 9, 'from': 's.vendor'}}})
        self.assertIn('api.limit', self.load().knowledge_conflicts)

    def test_an_uncontested_id_is_still_uncontested(self):
        self.assertNotIn('s.vendor', self.load().knowledge_conflicts)

    def test_an_unreadable_document_still_means_unreadable(self):
        """G.semantic_roles returning None maps to {'unreadable': True} for every entry."""
        with patch.object(G, 'semantic_roles', return_value=None):
            _, seen = self.recorded()
        self.assertTrue(seen)
        for document, history, name, result in seen:
            with patch.object(G, 'semantic_roles', return_value=None):
                self.assertEqual(result, old_meaning(document, name, history))


class EntrylessDocumentsAreLeftAlone(OverlayFixture):
    """The per-document work stays gated behind a non-empty entry map.

    Before F1 `meaning()` was reachable only from inside `for nid ... in entries.items()`, so a
    document with no entries never ran the history validation or the capability read, and never
    raised. Hoisting that work unconditionally turns those silent successes into refusals.
    """

    def test_a_history_bearing_target_without_entries_is_not_refused(self):
        target = self.target({'meta': {'history': {'authority': 'fixture'}}})
        doc = self.load(target)
        self.assertFalse(getattr(doc, 'target_unavailable', None),
                         'an entry-less history document was refused where it used to be ignored')

    def test_an_entry_less_hypothesis_is_never_layered(self):
        self.hypothesis('empty', {'known': {}})
        with Counter(P, 'layered') as layered:
            self.load()
        self.assertEqual(layered.count, 0)

    def test_an_entry_less_document_is_never_inferred(self):
        self.hypothesis('empty', {'known': {}})
        with Counter(G, 'meaning_capabilities') as capabilities:
            self.load()
        self.assertTrue(capabilities.calls, 'nothing was counted; the patch missed the reader')
        for args in capabilities.calls:
            self.assertTrue(G.entries(args[0]), 'capability read for a document with no entries')


class EveryOverlayOutputIsUnchanged(OverlayFixture):
    """The proof EPIC decision 1 asks for: the old overlay and the new one, same input, every
    named output equal.

    `MeansWhatItMeant` beside this compares the meanings that get computed, and is by
    construction blind to a change in *which* documents reach the computation at all - which is
    how the refused-target defect got past it. This one reads the outputs, so it sees both.
    """

    def outputs(self, target=None):
        """Load the same fixture twice: once through the pre-hoist overlay, once through this one."""
        old_overlay = pre_hoist_overlay()
        with patch.object(V, 'overlay', old_overlay):
            before = self.load(target)
        after = self.load(target)
        return ({field: normalize(getattr(before, field, None)) for field in OVERLAY_OUTPUTS},
                {field: normalize(getattr(after, field, None)) for field in OVERLAY_OUTPUTS})

    def assertSameOutputs(self, target=None):
        before, after = self.outputs(target)
        for field in OVERLAY_OUTPUTS:
            self.assertEqual(before[field], after[field], field + ' changed')
        return before

    def test_the_oracle_is_really_the_old_code(self):
        """Guard the control itself: a vendored overlay that had been hoisted would prove nothing."""
        source = subprocess.run(['git', 'show', BASE_COMMIT + ':scripts/knowledge_views.py'],
                                cwd=str(Path(__file__).resolve().parents[1]), capture_output=True)
        self.assertEqual(source.returncode, 0)
        text = source.stdout.decode()
        self.assertIn('def meaning(document, name, history=None):', text)
        self.assertNotIn('_meaning_context', text)

    def test_with_a_hypothesis_a_pending_bundle_and_a_target(self):
        self.hypothesis('alternate', {'known': {'api.limit': {'v': 9, 'from': 's.vendor'}}})
        target = self.target(hypotheses=[{'name': 'core', 'doc': copy.deepcopy(self.document),
                                          'head': {}}])
        outputs = self.assertSameOutputs(target)
        self.assertTrue(outputs['contributions'], 'fixture lost its pending contribution')
        self.assertIn('api.limit', outputs['knowledge_conflicts'],
                      'fixture stopped producing the conflict it exists to compare')

    def test_with_nothing_configured_beyond_the_checkout(self):
        self.assertSameOutputs()

    def test_when_a_target_document_refuses(self):
        """The case that caught the eager build: a refused target leaves partial holders behind."""
        document = copy.deepcopy(self.document)
        document['known']['api.limit']['v'] = 99
        document['meta'] = {'history': {'authority': 'fixture'}}
        outputs = self.assertSameOutputs(self.target(document))
        self.assertIn('api.limit', outputs['knowledge_conflicts'])

    def test_when_documents_carry_no_entries(self):
        self.hypothesis('empty', {'known': {}})
        self.assertSameOutputs(self.target({'meta': {'history': {'authority': 'fixture'}}}))


class TheHistoryAuthoritySurvivesTheSplit(unittest.TestCase):
    """A history-bearing document's authority reaches the meaning it belongs to.

    This is the one value the split actually relocated: `authority` was a local of the nested
    `meaning()`, computed and consumed in the same call, and is now written into the context by
    `_meaning_context` and read back out by `_meaning_of`. Nothing in the overlay fixtures
    reaches it - building a document with `meta.history` AND a projection valid enough for
    `CapturedHistory` means constructing eleven cross-validated projection keys - so these
    exercise the helpers directly.

    `CapturedHistory` is stubbed to a no-op. It is a validator, called from the same place with
    the same arguments on both sides of the comparison, so stubbing it changes neither side;
    what is being compared is where the authority travels, which is what moved.
    """

    document = {'meta': {'history': {'record_id': 'fixture'}},
                'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}},
                'known': {'api.limit': {'v': 10, 'from': 's.vendor'}}}

    def meaning(self, authority, name='api.limit'):
        history = {'authority': authority}
        contract = P._peer('history_contract')
        with patch.object(contract, 'CapturedHistory', lambda *a, **kw: None):
            document = copy.deepcopy(self.document)
            new = V._meaning_of(V._meaning_context(document, history), name)
            old = old_meaning(copy.deepcopy(self.document), name, history)
        return new, old

    def test_it_equals_the_pre_hoist_meaning(self):
        new, old = self.meaning({'record_id': 'authority-one'})
        self.assertEqual(new, old)

    def test_a_different_authority_is_a_different_meaning(self):
        """Without this, dropping the authority from the identity is invisible."""
        one, _ = self.meaning({'record_id': 'authority-one'})
        two, _ = self.meaning({'record_id': 'authority-two'})
        self.assertNotEqual(one, two)

    def test_a_document_without_history_needs_no_projection(self):
        plain = {'known': {'api.limit': {'v': 10, 'from': 's.vendor'}},
                 'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}}}
        self.assertEqual(V._meaning_of(V._meaning_context(plain, None), 'api.limit'),
                         old_meaning(copy.deepcopy(plain), 'api.limit', None))

    def test_history_without_a_projection_is_refused_as_before(self):
        with self.assertRaises(P.Refused):
            V._meaning_context(copy.deepcopy(self.document), None)
        with self.assertRaises(P.Refused):
            old_meaning(copy.deepcopy(self.document), 'api.limit', None)


class ARefusedTargetStillHoldsItsFirstEntry(OverlayFixture):
    """A target document whose meaning raises keeps the holder recorded before the raise.

    The target loop is the one call site whose exception is caught (`P.Refused` is a
    `SystemExit`, `contract.CapabilityError` a `ValueError`), so partial state from a refused
    document survives into `knowledge_conflicts`. Recording the holder before computing the
    meaning is therefore observable, not an implementation detail: build the document's meaning
    eagerly, before the loop, and this conflict silently loses a variant.
    """

    def refusing_target(self):
        """A target that declares captured history but carries no projection to validate it."""
        document = copy.deepcopy(self.document)
        document['known']['api.limit']['v'] = 99
        document['meta'] = {'history': {'authority': 'fixture'}}
        return self.target(document)

    def test_the_refused_target_is_reported_unavailable(self):
        self.assertIn('missing_history_context',
                      str(getattr(self.load(self.refusing_target()), 'target_unavailable', '')))

    def test_its_first_entry_is_still_a_holder_of_the_conflict(self):
        conflicts = self.load(self.refusing_target()).knowledge_conflicts
        self.assertIn('api.limit', conflicts)
        self.assertIn('target:refs/remotes/origin/trunk',
                      [name for name, _ in conflicts['api.limit']])
