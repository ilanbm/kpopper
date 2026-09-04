"""Component and output-lint regressions. No browser, network, or live records."""
import contextlib
import copy
import datetime
import io
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import render_page as R
from page_lint import Surface, lint_output
from page_words import WORDS

FIXTURE = ROOT / 'tests/fixtures/page'


class Components(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.record = self.root / 'PROVENANCE.yaml'
        self.brief = self.root / 'PROVENANCE.view.yaml'
        shutil.copy(FIXTURE / 'components.yaml', self.record)
        shutil.copy(FIXTURE / 'components.view.yaml', self.brief)

    def change(self, path, fn):
        doc = yaml.safe_load(path.read_text())
        fn(doc)
        path.write_text(yaml.safe_dump(doc, sort_keys=False, allow_unicode=True))

    def build(self):
        return R.build([str(self.record)], str(self.brief))

    def lint(self, transform=lambda x: x):
        page, e, j, _, info = self.build()
        return lint_output(transform(page), e, j, info)

    def elements(self, cls):
        return [n for n in Surface(self.build()[0]).root.walk() if cls in n.classes()]

    def test_component_fixture_verifies(self):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            result = R.verify([str(self.record)], str(self.brief))
        self.assertEqual(result, 0, out.getvalue())

    def test_measurement_recipe_is_preserved_in_the_payload(self):
        for recipe in ('python3 measure.py', {'command': 'python3 measure.py', 'inputs': ['grant.csv']}):
            with self.subTest(recipe=recipe):
                self.change(self.record, lambda d: d['known']['repair.grant'].update(measure=recipe))
                _, entries, _, _, _ = self.build()
                self.assertEqual(entries['repair.grant'].get('measure'), recipe)

    def test_count_tile_keeps_the_picked_date_as_its_source(self):
        big = self.elements('big')[0]
        self.assertEqual(big.attrs['data-countdown'], '2027-02-01')
        self.assertIn('date.cold', [n.attrs.get('data-id') for n in big.walk()])
        self.assertIn('days', big.text())

    def test_alert_has_icon_title_note_tag_and_group_dot(self):
        alert = self.elements('al')[0]
        classes = set().union(*(n.classes() for n in alert.walk()))
        self.assertTrue({'ico', 'at', 'aw', 'tag', 'group-dot'} <= classes)
        self.assertIn('awaiting a quote', alert.text())
        self.assertIn('unverified', alert.text())
        self.assertIn('judgment', alert.text())
        self.assertIn('warn', next(n for n in alert.walk() if 'ico' in n.classes()).classes())

    def test_card_keeps_reasoning_and_unverified_in_place(self):
        card = self.elements('card')[0]
        self.assertEqual(card.attrs['data-judgment'], 'c.ready')
        self.assertIn('The available grant is', card.text())
        self.assertIn('unverified', card.text())
        self.assertIn('--group:var(--g0)', card.attrs['style'])

    def test_timeline_groups_days_and_marks_today_without_script(self):
        class Day(datetime.date):
            @classmethod
            def today(cls):
                return cls(2026, 9, 4)
        with patch.object(R.datetime, 'date', Day):
            days = self.elements('day')
        self.assertEqual([n.attrs['data-day'] for n in days], ['2026-09-01', '2026-09-04', '2027-02-01'])
        self.assertIn('past', days[0].classes())
        self.assertIn('hot', days[1].classes())
        self.assertIn('today', days[1].text())

    def test_axis_is_written_prose_with_signed_live_references(self):
        axis = self.elements('axis')[0]
        self.assertIn('-1,200', axis.text())
        self.assertIn('+2,400', axis.text())
        self.assertEqual(len([n for n in axis.walk() if 'axis-step' in n.classes()]), 2)
        self.change(self.record, lambda d: d['known']['repair.grant'].update(v=3500))
        self.assertIn('+3,500', self.elements('axis')[0].text())
        self.assertIn('Moved since', self.build()[0])

    def test_multiline_reference_stays_inside_one_axis_step(self):
        self.change(self.record, lambda d: d['known']['doc.guide'].update(v='first\nsecond'))
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='The guide says {{doc.guide}}.'))
        steps = self.elements('axis-step')
        self.assertEqual(len(steps), 1)
        reference = next(n for n in steps[0].walk() if n.attrs.get('data-id') == 'doc.guide')
        self.assertEqual(reference.text(), 'first\nsecond')
        self.change(self.record, lambda d: d['judgments']['c.ready'].update(because='First reason.\nSecond reason.'))
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='The judgment is {{c.ready}}.'))
        self.assertEqual(len(self.elements('axis-step')), 1)

    def test_axis_requires_text(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='', pick='repair.grant'))
        self.assertTrue(any('axis needs' in x for x in self.build()[4]['contract']['bad']))

    def test_links_use_record_destinations(self):
        link = self.elements('lk')[0]
        self.assertEqual(link.attrs['href'], 'https://example.org/greenhouse')
        self.assertIn('doc.guide', [n.attrs.get('data-id') for n in link.walk()])
        self.change(self.record, lambda d: d['known']['doc.guide'].update(url='javascript:alert(1)'))
        self.assertTrue(any('safe url' in reason for _, reason in self.build()[4]['misfit']))

    def test_local_links_escape_spaces_and_fragments(self):
        self.assertEqual(R.link_target({'file': 'quotes/first #one.pdf'}), 'quotes/first%20%23one.pdf')
        self.assertEqual(R.link_target({'url': 'guide.html?lang=en#intro'}), 'guide.html?lang=en#intro')
        for url in ['javascript:alert(1)', 'data:text/html,<script>', '//bad.example', 'java\nscript:bad', 'https://[', 'https:missing-host']:
            self.assertEqual(R.link_target({'url': url}), '')

    def test_alert_keeps_both_blocked_reason_and_unverified_note(self):
        self.change(self.record, lambda d: d['judgments']['c.ready'].update(blocked_on='Waiting for the contractor schedule.'))
        alert = self.elements('al')[0]
        self.assertIn('contractor schedule', alert.text())
        self.assertIn('awaiting a quote', alert.text())

    def test_filenames_are_not_mistaken_for_internal_keys(self):
        self.change(self.record, lambda d: d['known']['repair.grant'].update(note='See annual_report.docx, annual_report.pptx, annual_report.svg and annual_report.sh.'))
        self.assertFalse(self.lint()[0])

    def test_footer_names_truth_and_elsewhere_from_entries(self):
        footer = next(n for n in Surface(self.build()[0]).root.walk() if n.tag == 'footer')
        self.assertIn('Truth: The repair guide', footer.text())
        self.assertIn('Elsewhere: The grant available', footer.text())
        self.assertIn('snapshot', footer.text())
        self.assertIn('doc.guide', [n.attrs.get('data-id') for n in footer.walk()])

    def test_footer_rejects_unanchored_claims(self):
        self.change(self.brief, lambda d: d.update(truth='Everything is guaranteed'))
        self.assertTrue(any('unanchored footer' in x for x in self.build()[4]['contract']['bad']))

    def test_group_overlap_draws_under_both_groups(self):
        groups = self.elements('group')
        under = [n.text() for n in groups if any(c.attrs.get('data-id') == 'date.visit' for c in n.walk())]
        self.assertEqual(len(under), 2)
        self.assertTrue(any('The glazier' in x for x in under))
        self.assertTrue(any('The owner' in x for x in under))

    def test_judgments_overlap_in_grouped_cards_too(self):
        def group(d):
            d['groups']['responsibilities']['The glazier'].append('c.ready')
            d['groups']['responsibilities']['The owner'].append('c.ready')
            d['tabs'][0]['sections'][-1]['pick'] = ['c.ready']
        self.change(self.brief, group)
        groups = self.elements('group')
        self.assertEqual(len(groups), 2)
        self.assertTrue(all(any('card' in c.classes() for c in n.walk()) for n in groups))
        self.assertFalse(self.build()[4]['misfit'])

    def test_group_identity_survives_value_changes(self):
        color = self.elements('card')[0].attrs['style']
        self.change(self.record, lambda d: d['known']['repair.grant'].update(v=8000))
        self.assertEqual(color, self.elements('card')[0].attrs['style'])

    def test_flat_groups_and_older_aliases_match(self):
        self.change(self.brief, lambda d: (d.update(groups=d['groups']['threads']), d['tabs'][0]['sections'][-1].pop('by')))
        modern = self.build()[0]
        self.change(self.brief, lambda d: (d.update(fronts=d.pop('groups')), d['tabs'][0]['sections'][-1].update({'as': 'fronts'})))
        self.assertEqual(modern.replace('data-component="grouped"', 'data-component="fronts"'), self.build()[0])

    def test_prefix_and_field_groupings_remain_available(self):
        self.change(self.record, lambda d: d['known']['repair.deposit'].update({'from': 'doc.guide'}))
        for field in ('prefix', 'unit', 'from'):
            self.change(self.brief, lambda d: d['tabs'][0]['sections'][-1].update(by=field))
            page, _, _, _, info = self.build()
            self.assertFalse(info['contract']['bad'])
            self.assertTrue(self.elements('group'))

    def test_bare_key_in_connective_prose_fails_verify(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='Read repair.grant before deciding.'))
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = R.verify([str(self.record)], str(self.brief))
        self.assertEqual(code, 1)
        self.assertIn('bare key on the reading surface: repair.grant', out.getvalue())

    def test_keys_in_hover_payload_do_not_fail_lint(self):
        self.assertFalse(self.lint()[0])
        self.assertIn('repair.grant', self.build()[0])

    def test_unknown_key_cannot_hide_in_connective_prose(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='Read repair.unrecorded first.'))
        self.assertTrue(any('bare key' in x for x in self.lint()[0]))

    def test_removed_value_reference_fails_output_lint(self):
        fail, _ = self.lint(lambda p: p.replace('data-id="repair.grant"', 'data-broken="repair.grant"'))
        self.assertTrue(any('no reference' in x for x in fail), fail)

    def test_nested_value_references_fail(self):
        fail, _ = self.lint(lambda p: p.replace('>+2,400</span>', '><span data-id="repair.deposit">+2,400</span></span>'))
        self.assertTrue(any('more than one reference' in x for x in fail), fail)

    def test_numbers_dates_and_quotes_in_connective_prose_need_references(self):
        for text in ['The budget is 2400 EUR.', 'The visit is 2026-09-04.', 'The quote says “ready”.']:
            self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text=text))
            self.assertTrue(any('value has no reference' in x for x in self.lint()[0]))

    def test_table_only_warns_of_a_dump(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'].append({'title': 'Amounts', 'why': 'Read the figures', 'pick': 'repair.', 'as': 'table'}))
        self.assertTrue(any('a dump with a heading: Amounts' in x for x in self.lint()[1]))

    def test_removed_judgment_marker_fails(self):
        fail, _ = self.lint(lambda p: p.replace('class="judgment-label"', 'class="ordinary-label"').replace('class="tag ok"', 'class="ordinary-tag"'))
        self.assertTrue(any('not visibly marked' in x for x in fail))

    def test_removed_unverified_warning_fails(self):
        fail, _ = self.lint(lambda p: p.replace('data-warning="true"', ''))
        self.assertTrue(any('unverified is not said' in x for x in fail))

    def test_unreviewed_connective_prose_is_flagged_in_place(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].pop('seen'))
        self.assertIn('not reviewed against every reference', self.build()[0])
        self.assertTrue(any('not been reviewed' in x for x in self.lint()[1]))
        fail, _ = self.lint(lambda p: p.replace('data-warning="true"', ''))
        self.assertTrue(any('unreviewed text is not flagged' in x for x in fail))

    def test_moved_text_and_judgments_must_keep_the_tint(self):
        self.change(self.record, lambda d: d['known']['repair.deposit'].update(v=-1500))
        fail, _ = self.lint(lambda p: p.replace('class="txt moved"', 'class="txt"'))
        self.assertTrue(any('connective text moved' in x for x in fail))
        fail, _ = self.lint(lambda p: p.replace('class="card moved"', 'class="card"').replace('fx in mv', 'fx in'))
        self.assertTrue(any('c.ready: moved since reviewed' in x for x in fail))

    def test_missing_intent_and_why_are_named(self):
        self.change(self.brief, lambda d: (d['tabs'][0].pop('serves'), d['tabs'][0]['sections'][0].pop('why')))
        warn = self.lint()[1]
        self.assertIn('tab without serves: Before winter', warn)
        self.assertTrue(any('section without why: Time left' in x for x in warn))

    def test_unanchored_connective_prose_warns(self):
        self.change(self.brief, lambda d: d['tabs'][0]['sections'][4].update(text='Everything is ready for the winter.'))
        self.assertTrue(any('connective, or an unrecorded claim' in x for x in self.lint()[1]))

    def test_arrangement_without_decision_warns(self):
        _, _, _, _, info = self.build()
        _, notes = R.arrangement_lines(info)
        self.assertTrue(any('no arrangement decision is recorded' in x for x in notes))

    def test_record_language_controls_chrome_and_direction(self):
        for lang in ('he', 'ar'):
            self.change(self.record, lambda d: d['meta'].update(language=lang))
            page = self.build()[0]
            self.assertIn(f'<html lang="{lang}" dir="rtl">', page)
            self.assertIn('>'+WORDS[lang]['tab_record']+' <span', page)
            self.assertIn(WORDS[lang]['judgment'], page)
            self.assertTrue(any('record language' in x for x in self.lint()[0]))

    def test_a_missing_chrome_catalog_fails(self):
        self.change(self.record, lambda d: d['meta'].update(language='fr'))
        self.assertIn('no chrome catalog for record language fr', self.lint()[0])

    def test_changing_only_values_cannot_flip_language(self):
        doc = {'known': {'x.a': {'name': 'The amount', 'v': 'כסף'*1000}}}
        self.assertEqual(R.language(doc), 'en')
        for name, lang in [('הסכום שנותר לתשלום', 'he'), ('المبلغ المتبقي للدفع', 'ar')]:
            doc['known']['x.a']['name'] = name
            self.assertEqual(R.language(doc), lang)

    def test_both_theme_catalogs_have_every_word(self):
        self.assertEqual(set(WORDS['en']), set(WORDS['he']))
        self.assertEqual(set(WORDS['en']), set(WORDS['ar']))
        self.assertIn(':root[data-theme=dark]', R.CSS)

    def test_long_reasoning_does_not_cut_a_reference_in_half(self):
        self.change(self.record, lambda d: d['judgments']['c.ready'].update(
            because='The reason remains in the record. ' * 20 + 'The grant is {{repair.grant}}.'))
        card = self.elements('card')[0]
        self.assertIn('The grant is 2,400.', card.text())
        self.assertFalse(self.lint()[0])

    def test_record_cannot_close_the_payload_script(self):
        self.change(self.record, lambda d: d['known']['repair.grant'].update(note='</script><script>alert(1)</script>'))
        page = self.build()[0]
        self.assertNotIn('</script><script>alert(1)', page)
        self.assertIn('\\u003c/script>', page)


if __name__ == '__main__':
    unittest.main()
