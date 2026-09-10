"""Search continuation is a bound read position and never skips budget-omitted hits."""
import base64
import json
from pathlib import Path
import tempfile
import unittest
from tests.test_session_context import CORE_READY

if CORE_READY:
    from scripts.session.view import GroundingService
    from scripts.session.search import SearchIndex

@unittest.skipUnless(CORE_READY,'install session extras and set up core')
class SearchPagination(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.folder=Path(self.temp.name);self.path=self.folder/'record.json'
        self.data={'nodes':{f'item.{i:03d}':{'kind':'known','states':[],'body':{'v':'shared observation'}} for i in range(70)},
                   'topics':{f'item.{i:03d}':['items'] for i in range(70)},'edges':[]}
        self.path.write_text(json.dumps(self.data))
        self.service=GroundingService('pages',self.path,self.folder/'state');_,self.rev=self.service.graph()
    def search(self,**kwargs):
        return json.loads(self.service.searching(kwargs.pop('query','shared observation'),self.rev,mode=kwargs.pop('mode','lexical'),**kwargs))
    def test_budget_reduced_pages_reach_all_matches_once_in_order(self):
        got=[];cursor=None
        for _ in range(80):
            text=self.service.searching('shared observation',self.rev,tokens=650,limit=32,mode='lexical',cursor=cursor)
            self.assertLessEqual(len(self.service.encoder.encode(text)),650)
            page=json.loads(text);self.assertEqual(page['offset'],len(got))
            self.assertEqual(page['matched_candidates'],70)
            got.extend(h['id'] for h in page['hits']);cursor=page['next_cursor']
            if cursor is None:break
            self.assertGreater(page['returned'],0)
        self.assertEqual(got,sorted(self.data['nodes']))
    def test_fresh_service_can_continue_with_different_page_size(self):
        first=self.search(limit=3);self.service=GroundingService('pages',self.path,self.folder/'state')
        second=self.search(limit=5,cursor=first['next_cursor'],tokens=2000)
        self.assertEqual(second['offset'],3)
        self.assertEqual([h['id'] for h in second['hits']],sorted(self.data['nodes'])[3:8])
    def test_different_query_ids_branch_or_requested_mode_reject(self):
        cursor=self.search(limit=2)['next_cursor']
        for kwargs in [{'query':'observation'},{'ids':['item.003']},{'branch':'/items'},{'mode':'hybrid'}]:
            with self.assertRaisesRegex(ValueError,'cursor|search'):self.search(cursor=cursor,**kwargs)
    def test_backend_change_rejects_even_with_same_revision(self):
        cursor=self.search(limit=2,mode='hybrid')['next_cursor']
        class Semantic:
            def rank(inner,documents,query):return {'scores':{k:1.0 for k in documents},'model':{'name':'test'}}
        self.service._searcher=SearchIndex(Semantic())
        with self.assertRaisesRegex(ValueError,'cursor|search'):self.search(cursor=cursor,mode='hybrid')
    def test_cache_flags_do_not_invalidate_but_model_identity_does(self):
        class Semantic:
            calls=0;revision='one'
            def rank(inner,documents,query):
                inner.calls+=1
                return {'scores':{k:1.0 for k in documents},'index_reused':inner.calls>1,
                        'model':{'name':'test','revision':inner.revision}}
        semantic=Semantic();self.service._searcher=SearchIndex(semantic)
        first=self.search(limit=2,mode='hybrid')
        self.assertEqual(self.search(cursor=first['next_cursor'],mode='hybrid')['offset'],2)
        semantic.revision='two'
        with self.assertRaisesRegex(ValueError,'cursor|search'):self.search(cursor=first['next_cursor'],mode='hybrid')
    def test_same_backend_changed_scores_reject(self):
        class Semantic:
            score=1.0
            def rank(inner,documents,query):return {'scores':{k:inner.score for k in documents},'model':{'name':'test'}}
        semantic=Semantic();self.service._searcher=SearchIndex(semantic)
        first=self.search(limit=2,mode='semantic');semantic.score=2.0
        with self.assertRaisesRegex(ValueError,'cursor|search'):self.search(cursor=first['next_cursor'],mode='semantic')
    def test_stale_record_rejects_before_continuation(self):
        cursor=self.search(limit=2)['next_cursor'];self.data['nodes']['item.069']['body']['v']='changed'
        self.path.write_text(json.dumps(self.data))
        with self.assertRaisesRegex(ValueError,'reopen'):self.search(cursor=cursor)
    def test_cursor_is_not_an_authenticated_permission_token(self):
        cursor=self.search(limit=2)['next_cursor']
        value=json.loads(base64.urlsafe_b64decode(cursor+'='*((-len(cursor))%4)))
        value['offset']=70
        changed=base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip('=')
        last=self.search(cursor=changed);self.assertEqual(last['hits'],[]);self.assertIsNone(last['next_cursor'])
        for invalid in [-1,71,True,1.5]:
            value['offset']=invalid
            changed=base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip('=')
            with self.assertRaises(ValueError):self.search(cursor=changed)
    def test_malformed_cursors_reject_without_implicit_restart(self):
        for cursor in ['',True,'!not-base64','e30','x'*1025]:
            with self.assertRaises(ValueError):self.search(cursor=cursor)

if __name__=='__main__':unittest.main()
