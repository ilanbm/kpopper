"""A measured page feeds followups only while its exact inputs remain current."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

from scripts import followups as F, render_page as R


class PageMeasurements(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.work = self.root / 'project'
        self.work.mkdir()
        self.record = self.work / 'GROUNDING.yaml'
        self.brief = self.work / '.kpopper/view.yaml'
        self.brief.parent.mkdir()
        self.env = patch.dict(os.environ, {'XDG_STATE_HOME': str(self.root / 'state')})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.doc = {'known': {'facts.count': {'v': 0}}, 'judgments': {
            'c.hidden': {'rests_on': ['facts.count'], 'seen': {'facts.count': 0}, 'verdict': 'Fits', 'wrong_if': 'facts.count > 1'},
            'c.page': {'rests_on': ['page.spill'], 'seen': {'page.spill': 0}, 'verdict': 'Covered', 'wrong_if': 'page.spill > 0'}}}
        self.save()
        self.brief.write_text('title: Coverage\nsections:\n  - title: Current\n    pick: [c.page, facts.count]\n')
        self.store = F.Store(self.work)
        self.store.setup(timezone='UTC', private=True)
        self.store.add({'id': 'pagecheck', 'title': 'Review coverage', 'why': 'Look at uncovered warnings',
                        'how': 'Read them', 'scope': 'Read and report only', 'related': ['page.spill'],
                        'when': {'condition': {'id': 'page.spill', 'op': '!=', 'value': 0}}})

    def save(self):
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))

    def state(self):
        return self.store.scan()['items'][0]['state']

    def measure(self):
        result = subprocess.run([sys.executable, str(Path(R.__file__).parent / 'cli.py'), 'experimental', 'hub', '--verify'],
                                cwd=self.work, env=os.environ, text=True, capture_output=True, timeout=30)
        self.assertNotIn('Traceback', result.stderr)
        return result

    def test_measure_rescan_and_staleness_are_three_distinct_states(self):
        self.assertEqual(self.state(), 'unknown')
        self.measure()
        with patch.object(R, 'build', side_effect=AssertionError('scan must not build')):
            self.assertEqual(self.state(), 'waiting')
        values, _, error = F.graph(str(self.record))
        self.assertIsNone(error)
        self.assertEqual(values['page.spill'], 0)
        self.doc['known']['facts.count']['v'] = 2
        self.save()
        self.assertEqual(self.state(), 'unknown')
        measured = self.measure()
        self.assertNotEqual(measured.returncode, 0)  # A firing condition is still a real measurement.
        self.assertEqual(self.state(), 'ready')
        self.assertEqual(F.graph(str(self.record))[0]['page.spill'], 1)
        self.brief.write_text('title: Coverage\nsections:\n  - title: All\n    pick: judgments\n')
        self.assertEqual(self.state(), 'unknown')
        self.measure()
        self.assertEqual(self.state(), 'waiting')

    def test_pure_build_is_not_a_published_measurement(self):
        R.build([str(self.record)], str(self.brief))
        self.assertEqual(self.state(), 'unknown')

    def test_alternate_view_never_replaces_the_default_measurement(self):
        alternate = self.work / 'alternate.yaml'
        alternate.write_text('title: Other\nsections:\n  - title: All\n    pick: judgments\n')
        with contextlib.redirect_stdout(io.StringIO()):
            R.verify([str(self.record)], str(alternate))
        self.assertEqual(self.state(), 'unknown')
        self.measure()
        self.assertEqual(self.state(), 'waiting')

    def test_hypothesis_addition_invalidates_the_measurement(self):
        self.measure()
        self.assertEqual(self.state(), 'waiting')
        folder = self.work / '.kpopper/hypotheses'
        folder.mkdir()
        (folder / 'alternative.yaml').write_text('known:\n  facts.count: {v: 3}\n')
        self.assertEqual(self.state(), 'unknown')

    def test_shard_changes_invalidate_the_measurement(self):
        shard = self.work / 'facts.yaml'
        shard.write_text(yaml.safe_dump({'known': self.doc.pop('known')}))
        self.doc['also'] = ['facts.yaml']
        self.save()
        self.measure()
        self.assertEqual(self.state(), 'waiting')
        shard.write_text('known:\n  facts.count: {v: 2}\n')
        self.assertEqual(self.state(), 'unknown')

    def test_legacy_entry_and_brief_work_together(self):
        self.record.rename(self.work / 'PROVENANCE.yaml')
        self.record = self.work / 'PROVENANCE.yaml'
        self.brief.rename(self.work / 'PROVENANCE.view.yaml')
        self.measure()
        self.assertEqual(F.graph(str(self.record))[0]['page.spill'], 0)

    def test_corrupt_measurement_is_unknown_and_scan_does_not_repair_it(self):
        self.measure()
        expected = F.M.snapshot([str(self.record)])
        path = F.M.cache_path(expected['scope'])
        payload = json.loads(path.read_text())
        payload['counts']['page.spill'] = 9
        path.write_text(json.dumps(payload))
        before = path.read_bytes()
        self.assertEqual(self.state(), 'unknown')
        self.assertEqual(path.read_bytes(), before)

    def test_changes_during_render_do_not_publish_mixed_measurements(self):
        build = R.build
        def changing(*args, **kwargs):
            result = build(*args, **kwargs)
            self.doc['known']['facts.count']['v'] = 2
            self.save()
            return result
        with patch.object(R, 'build', side_effect=changing), self.assertRaisesRegex(ValueError, 'changed'):
            R.measured_build([str(self.record)], str(self.brief))
        self.assertEqual(self.state(), 'unknown')

    def test_graph_and_measurement_are_from_the_same_read(self):
        self.measure()
        load = F.P.load
        loads = 0
        def racing(*args, **kwargs):
            nonlocal loads
            loads += 1
            doc = load(*args, **kwargs)
            if loads == 2:  # Discovery is followed by the actual bound read.
                self.doc['known']['facts.count']['v'] = 2
                self.save()
                self.measure()
            return doc
        with patch.object(F.P, 'load', side_effect=racing):
            values, _, error = F.graph(str(self.record))
        self.assertEqual(values, {})
        self.assertIn('changed', error)

    def test_loaded_code_change_prevents_publication(self):
        build = R.build
        def changing(*args, **kwargs):
            result = build(*args, **kwargs)
            guard = patch.object(R.MEASUREMENTS, '_code_identity', return_value={'new': 'version'})
            guard.start()
            self.addCleanup(guard.stop)
            return result
        with patch.object(R, 'build', side_effect=changing), self.assertRaisesRegex(ValueError, 'code changed'):
            R.measured_build([str(self.record)], str(self.brief))

    def test_unwritable_cache_does_not_break_rendering_or_claim_a_measurement(self):
        with patch.object(R.MEASUREMENTS, 'publish', side_effect=PermissionError('read only')), contextlib.redirect_stderr(io.StringIO()) as error:
            page = R.measured_build([str(self.record)], str(self.brief))[0]
        self.assertIn('<html', page)
        self.assertIn('could not be saved', error.getvalue())
        self.assertEqual(self.state(), 'unknown')

    def test_relative_state_home_uses_the_absolute_home_fallback(self):
        scope = F.M.snapshot([str(self.record)])['scope']
        with patch.dict(os.environ, {'XDG_STATE_HOME': 'relative-state'}), patch.object(Path, 'home', return_value=self.root):
            self.assertEqual(F.M.cache_path(scope).parent, self.root / '.local/state/kpopper/page-measurements')

    def test_core_identity_change_invalidates_the_measurement(self):
        self.measure()
        changed = dict(F.M._core_identity())
        changed['ready'] = not changed['ready']
        with patch.object(F.M, '_core_identity', return_value=changed):
            self.assertEqual(self.state(), 'unknown')

    def test_a_derived_rule_reads_the_published_count(self):
        if not F.M._core_identity().get('ready'):
            self.skipTest('build the core to exercise a calculation over page counts')
        self.doc['known']['page.double'] = {'rule': {'op': 'mul', 'args': [{'ref': 'page.spill'}, {'num': '2'}]}}
        self.save()
        self.assertIn('unavailable', F.graph(str(self.record))[0]['page.double'])
        self.measure()
        values, _, error = F.graph(str(self.record))
        self.assertIsNone(error)
        self.assertEqual(values['page.double']['computed']['value'], values['page.spill'] * 2)
