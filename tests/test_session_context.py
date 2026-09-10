"""Explicit graph reads retain source qualifications and visible boundaries."""
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

try:
    from scripts.session.core import Core
    from scripts.session.view import GroundingService
    CORE_READY=bool(Core())
except (ImportError,ValueError):
    CORE_READY=False
if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS')=='1' and not CORE_READY:
    raise RuntimeError('context tests require session extras and compiled core')


@unittest.skipUnless(CORE_READY,'install checked-session dependencies and set up the core')
class ContextIntegration(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.folder=Path(self.temp.name);self.path=self.folder/'record.json'
        self.data={'nodes':{
            's.report':{'kind':'sources','states':[],'body':{'text':'A reported check on September1; not a fresh observation.'}},
            'm.value':{'kind':'known','states':[],'body':{'v':2,'from':'s.report'}},
            'd.old':{'kind':'judgment','states':[],'body':{'rests_on':['m.value'],'seen':{'m.value':1},
                'wrong_if':'m.value > 10','verdict':'Original recommendation','status':'Superseded; keep original wording.'}},
            'd.new':{'kind':'judgment','states':[],'body':{'rests_on':['m.value'],'seen':{'m.value':2},
                'wrong_if':'m.value > 10','verdict':'Proposed alternative; requires a person to decide.'}},
            'd.old.child':{'kind':'known','states':[],'body':{'v':'A similar name is not a declared dependency.'}},
            'open.permission':{'kind':'open','states':['question'],'body':{'v':'No approval to send the message is recorded.'}}},
            'edges':[{'from':'d.old','rel':'rests_on','to':'m.value'},
                     {'from':'d.new','rel':'rests_on','to':'m.value'},
                     {'from':'m.value','rel':'from','to':'s.report'}]}
        self.refresh()
    def refresh(self):
        self.data['topics']={nid:[node['kind']] for nid,node in self.data['nodes'].items()}
        self.path.write_text(json.dumps(self.data))
        self.service=GroundingService('context',self.path,self.folder/'state');_,self.revision=self.service.graph()
    def context(self,ids,direction='support',**kwargs):
        return json.loads(self.service.contextualizing(ids,self.revision,direction,**kwargs))
    def test_support_reads_exact_sources_and_original_review_state(self):
        before=self.path.read_bytes();packet=self.context(['node:d.old'],depth=2,tokens=4000)
        reads={row['id']:row for row in packet['reads']}
        self.assertEqual(set(reads),{'d.old','m.value','s.report'})
        for nid,row in reads.items():
            expected=self.service.read_value(self.service.graph()[0],'node:'+nid)
            self.assertEqual(row['value'],expected);self.assertTrue(row['complete'])
            for edge in row['via']:self.assertIn(edge,self.data['edges'])
        self.assertEqual(reads['d.old']['value']['body']['seen'],{'m.value':1})
        self.assertIn('Superseded',reads['d.old']['value']['body']['status'])
        self.assertIn('epistemic_card',reads['d.old']['value'])
        self.assertEqual(self.path.read_bytes(),before)
    def test_impact_reaches_dependents_without_hierarchy_or_unlinked_permission(self):
        packet=self.context(['m.value'],'impact',depth=1,tokens=4000)
        self.assertEqual({r['id'] for r in packet['reads']},{'m.value','d.old','d.new'})
        self.assertNotIn('open.permission',str([r['id'] for r in packet['reads']]))
        self.assertEqual(packet['direction'],'impact')
    def test_depth_and_candidate_limits_leave_exact_incident_edge_routes(self):
        packet=self.context(['s.report'],'impact',depth=1,tokens=4000)
        self.assertEqual({r['id'] for r in packet['reads']},{'s.report','m.value'})
        frontier=next(r for r in packet['frontier'] if r['ref']=='edges:m.value')
        self.assertEqual(frontier['unread_edges'],2)
        links=json.loads(self.service.reading(frontier['ref'],self.revision))['value']
        self.assertTrue(any(e['from']=='d.old' for e in links))
        capped=self.context(['s.report'],'impact',depth=4,max_nodes=1,tokens=4000)
        self.assertEqual(len(capped['reads']),1);self.assertTrue(capped['candidate_limit_reached'])
    def test_cycles_and_multiple_seed_routes_read_each_body_once(self):
        self.data['edges'].append({'from':'s.report','rel':'rule_reads','to':'d.old'});self.refresh()
        packet=self.context(['d.old','node:d.old','m.value'],depth=4,tokens=6000)
        ids=[r['id'] for r in packet['reads']]
        self.assertEqual(len(ids),len(set(ids)));self.assertEqual(set(ids),{'d.old','m.value','s.report'})
    def test_budget_omits_a_whole_large_body_and_retains_seed_recovery(self):
        self.data['nodes']['s.report']['body']['text']='Long source qualification. '*3000;self.refresh()
        raw=self.service.contextualizing(['s.report'],self.revision,'support',tokens=700)
        packet=json.loads(raw);self.assertLessEqual(len(self.service.encoder.encode(raw)),700)
        self.assertEqual(packet['reads'],[]);self.assertEqual(packet['unread_seed_refs'],['node:s.report'])
        self.assertGreater(packet['unread_candidates'],0);self.assertIn('#/body',packet['next'])
    def test_missing_target_is_explicit_not_a_fabricated_source(self):
        self.data['edges'].append({'from':'m.value','rel':'from','to':'missing'});self.refresh()
        packet=self.context(['m.value'],tokens=4000)
        self.assertNotIn('missing',{r['id'] for r in packet['reads']})
        row=next(r for r in packet['frontier'] if r['ref']=='edges:m.value')
        self.assertEqual(row['missing_targets'],1)
    def test_bounds_unknown_refs_and_stale_revision_reject(self):
        for ids,kwargs in [([],{}),(['missing'],{}),(['source:s.report'],{}),(['node:m.value#/body'],{}),
                           (['d.old','m.value'],{'max_nodes':1}),(['m.value'],{'depth':True}),
                           (['m.value'],{'depth':5}),(['m.value'],{'max_nodes':65})]:
            with self.assertRaises(ValueError):self.context(ids,**kwargs)
        with self.assertRaises(ValueError):self.context(['m.value'],'both')
        self.data['nodes']['m.value']['body']['v']=3;self.path.write_text(json.dumps(self.data))
        with self.assertRaisesRegex(ValueError,'reopen'):self.context(['m.value'])
    def test_wide_record_preserves_a_bounded_frontier(self):
        for i in range(5000):
            nid=f'x.{i:04d}';self.data['nodes'][nid]={'kind':'known','states':[],'body':{'v':i}}
            self.data['edges'].append({'from':'s.report','rel':'rule_reads','to':nid})
        self.refresh();raw=self.service.contextualizing(['s.report'],self.revision,'support',tokens=1000,max_nodes=8)
        packet=json.loads(raw)
        self.assertLessEqual(packet['candidates'],8);self.assertTrue(packet['candidate_limit_reached'])
        self.assertLessEqual(len(self.service.encoder.encode(raw)),1000)
        row=next(r for r in packet['frontier'] if r['ref']=='edges:s.report')
        self.assertGreater(row['unread_edges'],4900)
    def test_cli_context_matches_service(self):
        expected=self.service.contextualizing(['d.old'],self.revision,'support',tokens=2000)
        cmd=[sys.executable,'scripts/session_cli.py','context','--normalized','--no-settings','--input',str(self.path),
             '--project','context','--state',str(self.folder/'state'),'--id','d.old','--revision',self.revision,
             '--direction','support','--tokens','2000']
        result=subprocess.run(cmd,text=True,encoding='utf-8',capture_output=True)
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout,expected)
    @unittest.skipUnless(importlib.util.find_spec('mcp'),'install MCP dependency')
    def test_mcp_context_matches_service(self):
        from mcp import Client
        from mcp.client.stdio import StdioServerParameters
        expected=self.context(['m.value'],'impact',tokens=2000)
        async def scenario():
            script=Path(__file__).resolve().parents[1]/'scripts/session_cli.py'
            params=StdioServerParameters(command=sys.executable,args=[str(script),'serve','--normalized','--no-settings',
                '--input',str(self.path),'--project','context','--state',str(self.folder/'state')],cwd=self.folder)
            async with Client(params) as client:
                result=await client.call_tool('kpopper_context',{'ids':['m.value'],'revision':self.revision,'direction':'impact','tokens':2000})
                self.assertFalse(result.is_error,str(result))
                self.assertEqual(json.loads(''.join(p.text for p in result.content if p.type=='text')),expected)
                for name,value in [('depth',True),('max_nodes',True),('tokens',2000.0)]:
                    invalid=await client.call_tool('kpopper_context',{'ids':['m.value'],'revision':self.revision,
                        'direction':'impact',name:value})
                    self.assertTrue(invalid.is_error,(name,value))
        asyncio.run(scenario())

if __name__=='__main__':unittest.main()
