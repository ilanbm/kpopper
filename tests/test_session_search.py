import unittest
from scripts.session.search import SearchIndex

class FakeSemantic:
    def __init__(self,scores=None,error=None):self.scores=scores;self.error=error
    def rank(self,docs,query):
        if self.error:raise ValueError(self.error)
        return {'scores':self.scores,'document_count':len(docs)}

def nodes(**texts):return {key:{'body':{'v':text}} for key,text in texts.items()}

class SearchBoundaries(unittest.TestCase):
    def test_exact_identifier_beats_semantically_high_distractor(self):
        data=nodes(**{'p.script_copies':'Editable script copies','other':'Browser browser browser'})
        index=SearchIndex(FakeSemantic({'p.script_copies':.01,'other':.99}))
        result=index.search(data,'p.script_copies',limit=1,mode='semantic')
        self.assertEqual(result['hits'][0]['id'],'p.script_copies');self.assertEqual(result['hits'][0]['match'],'literal_id')
    def test_hint_cannot_displace_better_global_candidate(self):
        data=nodes(**{'global':'global','hinted':'hinted'})
        result=SearchIndex(FakeSemantic({'global':.9,'hinted':.8})).search(data,'query',limit=1,branch_members={'hinted'},mode='semantic')
        self.assertEqual(result['hits'][0]['id'],'global')
    def test_branch_only_breaks_score_ties(self):
        result=SearchIndex(FakeSemantic({'a':.8,'z':.8})).search(nodes(a='a',z='z'),'query',limit=1,branch_members={'z'},mode='semantic')
        self.assertEqual(result['hits'][0]['id'],'z')
    def test_unknown_exact_id_is_reported_without_invented_match(self):
        result=SearchIndex().search(nodes(a='A source'),'',['unseen'])
        self.assertEqual(result['hits'],[]);self.assertEqual(result['unresolved_ids'],['unseen'])
    def test_returned_node_reference_is_accepted_as_an_exact_identifier(self):
        result=SearchIndex().search(nodes(**{'p.script_copies':'one'}),'',['node:p.script_copies','p.script_copies'])
        self.assertEqual([h['id'] for h in result['hits']],['p.script_copies'])
        self.assertEqual(result['hits'][0]['match'],'explicit_id');self.assertEqual(result['unresolved_ids'],[])
    def test_unknown_references_preserve_spelling_and_do_not_change_read_kind(self):
        requested=['node:missing','source:known','node:known#/body','node:node:known']
        result=SearchIndex().search(nodes(known='one'),'',requested)
        self.assertEqual(result['hits'],[]);self.assertEqual(result['unresolved_ids'],requested)
    def test_identifier_components_are_lexically_searchable(self):
        result=SearchIndex().search(nodes(**{'p.script_copies':'one editable file','d.other':'unrelated'}),'script copies',mode='lexical')
        self.assertEqual(result['hits'][0]['id'],'p.script_copies')
    def test_no_partial_identifier_match(self):
        result=SearchIndex().search(nodes(**{'a.b':'neutral','a.b.c':'neutral'}),'a.b.c',mode='lexical')
        self.assertEqual(result['hits'][0]['id'],'a.b.c');self.assertNotEqual(result['hits'][1]['match'],'literal_id')
    def test_hyphenated_long_identifier_wins_over_its_prefix(self):
        data=nodes(**{'p.script':'copies','p.script-copies':'copies'})
        result=SearchIndex().search(data,'p.script-copies',limit=1,mode='lexical')
        self.assertEqual(result['hits'][0]['id'],'p.script-copies');self.assertEqual(result['hits'][0]['match'],'literal_id')
    def test_unknown_identifier_extension_is_not_a_literal_prefix_match(self):
        for suffix in ['-copies','+copies','@copies']:
            result=SearchIndex().search(nodes(**{'p.script':'script'}),'p.script'+suffix,mode='lexical')
            self.assertTrue(all(h['match']!='literal_id' for h in result['hits']))
    def test_long_opaque_id_and_a_later_separate_short_id_are_preserved(self):
        from scripts.session.search import mentioned_ids
        self.assertEqual(mentioned_ids('"p.script copies" then p.script',{'p.script','p.script copies'}),['p.script copies','p.script'])
    def test_semantic_failure_falls_back_over_complete_record(self):
        data=nodes(a='bananas',b='apples')
        result=SearchIndex(FakeSemantic(error='full-index limit')).search(data,'apples',mode='semantic')
        self.assertEqual(result['hits'][0]['id'],'b');self.assertEqual(result['record_nodes'],2);self.assertIn('full-index limit',result['fallback'])
    def test_partial_semantic_scores_do_not_silently_hide_documents(self):
        result=SearchIndex(FakeSemantic({'a':.9})).search(nodes(a='banana',b='apple'),'apple')
        self.assertEqual(result['backend'],'lexical');self.assertEqual(result['hits'][0]['id'],'b')
    def test_long_source_tail_survives_lexical_indexing(self):
        result=SearchIndex().search(nodes(a='padding '*30000+'uniquetail',b='neutral'),'uniquetail',mode='lexical')
        self.assertEqual(result['hits'][0]['id'],'a')
    def test_source_changes_invalidate_index(self):
        index=SearchIndex();first=index.search(nodes(a='apple'),'apple',mode='lexical');second=index.search(nodes(a='pear',b='apple'),'apple',mode='lexical')
        self.assertNotEqual(first['index_fingerprint'],second['index_fingerprint']);self.assertEqual(second['hits'][0]['id'],'b')
    def test_empty_queries_and_invalid_bounds_reject(self):
        for kwargs in [{},{'query':'x','limit':0},{'query':'x','limit':True},{'query':'x','ids':[None]},{'query':'x','mode':'unknown'}]:
            with self.assertRaises(ValueError):SearchIndex().search(nodes(a='a'),**({'query':''}|kwargs))
    def test_large_record_falls_back_completely_before_loading_model_assets(self):
        from scripts.session.embeddings import E5Index
        data=nodes(**{'item.'+str(n):'ordinary value' for n in range(513)})
        data['item.512']['body']['v']='unique_tail_target'
        result=SearchIndex(E5Index('/missing-assets')).search(data,'unique tail target')
        self.assertEqual(result['backend'],'lexical');self.assertIn('512',result['fallback'])
        self.assertEqual(result['record_nodes'],513);self.assertEqual(result['hits'][0]['id'],'item.512')


import asyncio
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

try:
    from scripts.session.view import GroundingService
    from scripts.session.core import Core
    CORE_READY=bool(Core())
except (ImportError,ValueError):
    CORE_READY=False

if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS')=='1' and not CORE_READY:
    raise RuntimeError('search integration requires session dependencies and compiled core')

@unittest.skipUnless(CORE_READY,'install checked-session dependencies and set up the core')
class SearchIntegration(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.folder=Path(self.temp.name);self.path=self.folder/'record.json'
        self.data={'nodes':{'m.value':{'kind':'known','states':[],'body':{'v':2,'from':'s.report'}},'s.report':{'kind':'sources','states':[],'body':{'text':'Reported observation, not fresh verification.'}},'d.choice':{'kind':'judgment','states':[],'body':{'rests_on':['m.value'],'seen':{'m.value':1},'wrong_if':'m.value > 10','verdict':'retain the hypothesis','because':'based on the recorded observation'}}},'topics':{'m.value':['known'],'s.report':['sources'],'d.choice':['judgments']},'edges':[{'from':'m.value','rel':'from','to':'s.report'},{'from':'d.choice','rel':'rests_on','to':'m.value'}]}
        self.path.write_text(json.dumps(self.data));self.service=GroundingService('fixture',self.path,self.folder/'state')
        _,self.revision=self.service.graph()
    def test_search_returns_readable_originals_and_preserves_full_opening(self):
        before=self.path.read_bytes();opened=self.service.opening_packet(700)
        response=json.loads(self.service.searching('d.choice',self.revision,mode='lexical'))
        self.assertEqual(response['hits'][0]['ref'],'node:d.choice')
        body=json.loads(self.service.reading(response['hits'][0]['ref'],self.revision))['value']['body']
        self.assertEqual(body,self.data['nodes']['d.choice']['body'])
        edges=json.loads(self.service.reading(response['hits'][0]['links_ref'],self.revision))['value']
        self.assertIn({'from':'d.choice','rel':'rests_on','to':'m.value'},edges)
        after=self.service.opening_packet(700);self.assertEqual(opened['text'],after['text']);self.assertTrue(after['guard']['accepted'])
        self.assertEqual(self.path.read_bytes(),before)
    def test_stale_revision_and_foreign_revision_are_rejected(self):
        self.data['nodes']['m.value']['body']['v']=3;self.path.write_text(json.dumps(self.data))
        with self.assertRaisesRegex(ValueError,'reopen'):self.service.searching('value',self.revision)
        other=GroundingService('different',self.path,self.folder/'other-state');_,other_rev=other.graph()
        with self.assertRaisesRegex(ValueError,'reopen'):self.service.searching('value',other_rev)
    def test_no_embedding_configuration_and_bad_assets_are_explicit_fallbacks(self):
        first=json.loads(self.service.searching('observation',self.revision))
        self.assertEqual(first['backend'],'lexical');self.assertIn('not configured',first['fallback'])
        service=GroundingService('fixture',self.path,self.folder/'state',embedding_dir=self.folder/'missing')
        result=json.loads(service.searching('observation',self.revision))
        self.assertEqual(result['backend'],'lexical');self.assertIn('unavailable',result['fallback'])
        self.assertTrue(result['hits'])
    def test_budget_drops_only_whole_candidates_and_counts_the_newline(self):
        data={'nodes':{},'topics':{},'edges':[]}
        for n in range(40):
            key='item.'+('long_name_'*8)+str(n);data['nodes'][key]={'kind':'known','states':[],'body':{'v':'shared match'}};data['topics'][key]=['items']
        self.path.write_text(json.dumps(data));_,rev=self.service.graph()
        text=self.service.searching('shared match',rev,tokens=450,limit=32,mode='lexical');result=json.loads(text)
        self.assertLessEqual(len(self.service.encoder.encode(text)),450);self.assertTrue(text.endswith('\n'))
        self.assertTrue(result['limited']);self.assertGreater(result['budget_omitted'],0);self.assertGreater(result['returned'],0)
        for hit in result['hits']:self.assertIn(hit['id'],data['nodes']);self.assertEqual(hit['ref'],'node:'+hit['id'])
    def test_unknown_branch_is_not_used_as_an_access_filter(self):
        with self.assertRaisesRegex(ValueError,'unknown branch'):self.service.searching('value',self.revision,branch='/missing')
    def test_cli_search_matches_the_service_and_stays_in_budget(self):
        expected=self.service.searching('d.choice',self.revision,tokens=1000,mode='lexical')
        command=[sys.executable,'scripts/session_cli.py','search','--normalized','--no-settings','--input',str(self.path),'--project','fixture','--state',str(self.folder/'state'),'--query','d.choice','--revision',self.revision,'--search-mode','lexical','--tokens','1000']
        result=subprocess.run(command,text=True,encoding='utf-8',capture_output=True)
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout,expected)
        self.assertLessEqual(len(self.service.encoder.encode(result.stdout)),1000)

    @unittest.skipUnless(importlib.util.find_spec('mcp'),'install the MCP session dependency')
    def test_real_mcp_search_matches_cli_and_rejects_stale_revision(self):
        from mcp import Client
        from mcp.client.stdio import StdioServerParameters
        expected=json.loads(self.service.searching('d.choice',self.revision,mode='lexical'))
        async def scenario():
            script=Path(__file__).resolve().parents[1]/'scripts'/'session_cli.py'
            params=StdioServerParameters(command=sys.executable,args=[str(script),'serve','--normalized','--no-settings','--input',str(self.path),'--project','fixture','--state',str(self.folder/'state')],cwd=self.folder)
            async with Client(params) as client:
                result=await client.call_tool('kpopper_search',{'query':'d.choice','revision':self.revision,'mode':'lexical'})
                self.assertFalse(result.is_error,str(result))
                actual=json.loads(''.join(p.text for p in result.content if p.type=='text'))
                self.assertEqual(actual,expected)
                first=actual['hits'][0]
                evidence=await client.call_tool('kpopper_read',{'ref':first['ref'],'revision':self.revision})
                self.assertFalse(evidence.is_error,str(evidence))
                self.data['nodes']['m.value']['body']['v']=4;self.path.write_text(json.dumps(self.data))
                stale=await client.call_tool('kpopper_search',{'query':'value','revision':self.revision})
                self.assertTrue(stale.is_error)
        asyncio.run(scenario())

if __name__=='__main__':unittest.main()
