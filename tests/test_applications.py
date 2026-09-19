"""HTML applications are explicit, optional consumers of the record."""
import json
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
RECORD = '''known:
  s.source: {name: Source, read: "2026-09-17"}
  m.value: {name: Value, v: 1, from: s.source}
judgments:
  d.safe:
    rests_on: [m.value]
    seen: {m.value: 1}
    verdict: Below the limit
    wrong_if: m.value > 10
'''


class Applications(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'GROUNDING.yaml').write_text(RECORD)
        (self.root / '.kpopper').mkdir()
        # An unusable presentation must not prevent reading or updating the record.
        (self.root / '.kpopper/view.yaml').write_text('tabs: [invalid presentation]\n')
        self.env = {**os.environ, 'XDG_STATE_HOME': str(self.root / 'state'),
                    'XDG_CONFIG_HOME': str(self.root / 'config'),
                    'KPOPPER_SESSION_DISABLE': '1', 'KPOPPER_READ_MODE': 'frozen'}

    def cli(self, *args, blocked=()):
        blockdir = self.root / 'blocker'
        blockdir.mkdir(exist_ok=True)
        (blockdir / 'sitecustomize.py').write_text("""
import importlib.abc, sys
class Block(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if any(part in %r for part in fullname.split('.')):
            with open(%r, 'a') as trace:
                trace.write(fullname + '\\n')
            raise ImportError('blocked optional module: ' + fullname)
sys.meta_path.insert(0, Block())
""" % (blocked, str(self.root / 'optional-imports')))
        env = {**self.env, 'PYTHONPATH': str(blockdir)}
        return subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), *args],
                              cwd=self.root, env=env, text=True, capture_output=True)

    def test_help_separates_experimental_applications(self):
        result = self.cli('--help')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('kpop experimental', result.stdout)
        self.assertNotIn('kpop page ', result.stdout)
        result = self.cli('experimental', '--json')
        self.assertEqual(result.returncode, 0, result.stderr)
        applications = json.loads(result.stdout)['applications']
        self.assertEqual({a['name'] for a in applications}, {'hub', 'annotated-doc'})
        self.assertTrue(all(a['layer'] == 'application' and a['status'] == 'experimental'
                            and a['extra'] == 'html' for a in applications))

    def test_application_help_and_missing_dependency_remedy(self):
        for app in ('hub', 'annotated-doc'):
            result = self.cli('experimental', app, '--help', blocked=('html5lib', 'tinycss2'))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('experimental', result.stdout)
        result = self.cli('experimental', 'hub', blocked=('html5lib',))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn('kpopper[html]', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertFalse((self.root / '.kpopper/build').exists())

    def test_ordinary_commands_do_not_load_html_even_with_a_view(self):
        blocked = ('render_page', 'html5lib', 'tinycss2', 'page_words', 'page_lint')
        commands = [('open',), ('check',), ('pull', 'd.safe'), ('affects', 'm.value'),
                    ('set', 'm.value', '2'), ('review', 'd.safe'),
                    ('add', 'm.other', 'v=3'), ('consolidate', '--dry-run')]
        for args in commands:
            with self.subTest(args=args):
                result = self.cli(*args, blocked=blocked)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertNotIn('blocked optional module', result.stdout + result.stderr)
                self.assertNotIn('brief beside the record could not be built', result.stdout)
                self.assertFalse((self.root / 'optional-imports').exists())

    def test_page_conditions_remain_unchecked_without_rendering(self):
        with (self.root / 'GROUNDING.yaml').open('a') as record:
            record.write('''  d.coverage:
    rests_on: [page.unserved]
    seen: {page.unserved: 0}
    verdict: All intents covered
    wrong_if: page.unserved > 0
''')
        result = self.cli('check', blocked=('render_page', 'html5lib', 'tinycss2'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('page.unserved', result.stdout)
        self.assertIn('counted when the page is built', result.stdout)
        self.assertFalse((self.root / 'optional-imports').exists())

    def test_new_history_record_and_hub_help_work_without_html(self):
        (self.root / 'GROUNDING.yaml').unlink()
        (self.root / '.kpopper/view.yaml').unlink()
        blocked = ('render_page', 'html5lib', 'tinycss2')
        for args in [('add', 'm.value', 'v=2', 'name=Value'), ('open',), ('check',),
                     ('pull', 'm.value'), ('experimental', 'hub', '--help')]:
            result = self.cli(*args, blocked=blocked)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'optional-imports').exists())
        self.assertIn('core/v1', (self.root / 'GROUNDING.yaml').read_text())

    def test_hub_selects_core_when_a_custom_brief_is_supplied(self):
        (self.root / 'GROUNDING.yaml').unlink()
        (self.root / '.kpopper/view.yaml').unlink()
        created = self.cli('add', 'm.value', 'v=2', 'name=Value')
        self.assertEqual(created.returncode, 0, created.stdout + created.stderr)
        (self.root / 'custom.yaml').write_text('title: Hub\nsections:\n  - title: Values\n    pick: all\n')
        result = self.cli('experimental', 'hub', '--brief', 'custom.yaml', '--out', 'hub.html')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('data-profile="core/v1"', (self.root / 'hub.html').read_text())

    def test_layout_proposal_with_its_own_new_source_requires_the_application(self):
        from scripts import consolidate as C
        base = C.P.Record({'known': {'m.value': {'v': 1}}})
        proposed = {
            'sources': {'s.new': {'asked': 'Show the new subject', 'read': '2026-09-18'}},
            'judgments': {'v.new': {'rests_on': ['s.new', 'page.unserved'],
                                    'verdict': 'A new layout', 'wrong_if': 'page.unserved > 0',
                                    'seen': {'s.new': 'read 2026-09-18', 'page.unserved': 0}}},
        }
        hypothesis = {'doc': proposed, 'raw': C.P.bodies(proposed)}
        facts = {'v.new': {'fired': False}}
        with mock.patch.object(C.P, 'load', return_value=base), \
                mock.patch.object(C.P, 'brief_for', return_value='view.yaml'), \
                mock.patch.object(C.P, '_page_side', return_value=({}, None, facts)) as page:
            self.assertEqual(C.page_of(['unused.yaml'], [hypothesis]), facts)
            page.assert_called_once()

    def test_page_alias_retains_output_and_labels_experimental_status(self):
        (self.root / '.kpopper/view.yaml').unlink()
        first = self.cli('experimental', 'hub', '--out', 'new.html')
        self.assertEqual(first.returncode, 0, first.stderr)
        old = self.cli('page', '--out', 'old.html')
        self.assertEqual(old.returncode, 0, old.stderr)
        self.assertIn('kpop experimental hub', old.stderr)
        for name in ('new.html', 'old.html'):
            self.assertIn('Below the limit', (self.root / name).read_text())
        wrapped = self.cli('--json', 'experimental', 'hub', '--verify')
        self.assertEqual(wrapped.returncode, 0, wrapped.stdout + wrapped.stderr)
        self.assertEqual(json.loads(wrapped.stdout)['exit_code'], 0)

    def test_old_names_resolve_to_the_canonical_applications(self):
        for old, current in (('page', 'hub'), ('document', 'annotated-doc')):
            for prefix in ((), ('experimental',)):
                with self.subTest(old=old, prefix=prefix):
                    result = self.cli(*prefix, old, '--help')
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn('kpop experimental ' + current, result.stdout)
                    self.assertIn('kpop experimental ' + current, result.stderr)

    def test_experimental_alias_keeps_workspace_and_json_output(self):
        (self.root / '.kpopper/view.yaml').unlink()
        result = self.cli('--workspace', str(self.root), '--json', 'experimental', 'page', '--verify')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['command'], 'page')
        self.assertIn('kpop experimental hub', result.stderr)

    def test_graph_only_hypothesis_without_brief_never_loads_the_hub(self):
        record = self.root / 'GROUNDING.yaml'
        record.write_text(RECORD.replace('name: Source,', 'name: Source, asked: Check the graph,'))
        (self.root / '.kpopper/view.yaml').unlink()
        hypotheses = self.root / '.kpopper/hypotheses'
        hypotheses.mkdir()
        (hypotheses / 'graph-only.yaml').write_text('''judgments:
  v.graph_guard:
    rests_on: [s.source, graph.flagged]
    verdict: Few flagged judgments
    wrong_if: graph.flagged > 3
    seen: {s.source: "read 2026-09-17", graph.flagged: 0}
''')
        result = self.cli('consolidate', '--dry-run', blocked=('render_page', 'html5lib', 'tinycss2'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'optional-imports').exists())

    def test_legacy_presentation_review_does_not_require_document_libraries(self):
        record = self.root / 'GROUNDING.yaml'
        record.write_text(RECORD.replace('name: Source,', 'name: Source, asked: Check the graph,') + '''  v.graph_guard:
    rests_on: [s.source, graph.flagged]
    verdict: Few flagged judgments
    wrong_if: graph.flagged > 3
    seen: {s.source: "read 2026-09-17", graph.flagged: 0}
''')
        (self.root / '.kpopper/view.yaml').write_text('title: View\nsections:\n  - title: Values\n    pick: all\n')
        result = self.cli('review', 'v.graph_guard', blocked=('html5lib', 'tinycss2'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'optional-imports').exists())

    def test_graph_only_hypothesis_does_not_validate_an_unrelated_brief(self):
        self.test_graph_only_hypothesis_without_brief_never_loads_the_hub()
        (self.root / '.kpopper/view.yaml').write_text('tabs: [invalid presentation]\n')
        result = self.cli('consolidate', '--dry-run', blocked=('render_page', 'html5lib', 'tinycss2'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / 'optional-imports').exists())

    def test_unreadable_guide_returns_a_clean_error(self):
        from scripts.applications import annotated_doc
        for error in (OSError(5, 'Input/output error'), UnicodeError('invalid guide encoding')):
            output = io.StringIO()
            with mock.patch.object(annotated_doc.Path, 'read_text', side_effect=error), \
                    contextlib.redirect_stderr(output):
                self.assertEqual(annotated_doc.main(['guide']), 2)
            self.assertIn('document:', output.getvalue())

    def test_new_application_names_require_the_explicit_namespace(self):
        for name in ('hub', 'annotated-doc'):
            result = self.cli(name, '--help')
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn('kpop experimental ' + name, result.stderr)

    def test_group_json_flag_works_before_the_application_name(self):
        result = self.cli('experimental', '--json', 'hub', '--help')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['command'], 'hub')


if __name__ == '__main__':
    unittest.main()
