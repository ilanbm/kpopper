#!/usr/bin/env python3
"""Model-free adoption oracle from an explicitly selected immutable Python tree."""
import argparse, base64, copy, datetime, json, subprocess, sys, tempfile
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--baseline', required=True, type=Path)
p.add_argument('--output', required=True, type=Path)
a = p.parse_args()
sys.path.insert(0, str(a.baseline))
from scripts import history_bundle as B, history_contract as C, history_store as H
from scripts import history_transaction as T, pending_grounding as G, project_modes as M
from unittest import mock

SCOPE = {'kind': 'project', 'environment': 'fixture'}
DAY = '2026-09-20T03:04:05.123456+00:00'
enc = lambda files: {p: base64.b64encode(raw).decode() for p, raw in sorted(files.items())}
def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout

def setup(root, profile, record):
    entry = root / 'GROUNDING.yaml'
    store = H.Store(entry)
    marker = C.authority(record_id=record, authority='history', generation=1)
    authority = Path(store.layout['history_authority']); authority.parent.mkdir(parents=True)
    authority.write_bytes(b'# exact authority bytes\n' + C.encode_document(marker))
    template = {'schema': {'deps':'rests_on', 'snapshot':'seen', 'predicate':'wrong_if'},
                'meta': {'custom': {'owner': 'fixture', 'nested': [1, True, 'unchanged']}},
                'record': {'custom-uninterpreted': 'preserve me'},
                'known': {}}
    if profile == 'core/v1': template['meta']['reasoning'] = {'version':2, 'profile':profile, 'requires':['arithmetic/v1']}
    template['meta']['history'] = H.baseline(marker, {}, H.reduce({}))
    entry.write_bytes(C.encode_document(template))
    return entry, store, marker

def claim(profile, subject, value, op, pins=None, saw=(), body=None):
    return C.make_object(subject=subject, kind='reading', by='source author', on='2026-09-17', operation=op,
        saw=saw, pins=pins, body={'v': value, 'scope': copy.deepcopy(SCOPE), **(body or {})},
        authored={'collection':'known','profile':profile,'fields':{'deps':'rests_on','snapshot':'seen','predicate':'wrong_if'}})

def publish(store, marker, objects, operation):
    captured = store.capture()
    pairs = [(obj, b'# exact imported bytes\n' + C.encode_document(obj)) for obj in objects]
    receipt = T.semantic_receipt(profile='ordinary-reader/v1', capabilities={}, before={}, after={})
    args = dict(marker=marker,operation=operation,parents=C.commit_frontier(captured.commits),baseline=captured.baseline,
        objects=pairs,receipt=receipt,view_template=C.document_template(captured.document))
    draft = C.make_commit(**args,view=b'')
    commits={**captured.commits,operation:C.encode_document(draft)}
    after=store.render(captured,objects={**captured.objects,**{o['id']:o for o in objects}},commits=commits)
    commit=C.make_commit(**args,view=after)
    files=[{'path':store.entry.name,'role':'record','before':captured.entry_bytes,'after':after}]
    for obj,raw in pairs:
        path=Path(store.layout['history']) / obj['subject'] / (obj['id']+'.yaml')
        files.append({'path':path.relative_to(store.root).as_posix(),'role':'history_object','before':None,'after':raw})
    files.append({'path':(Path(store.layout['history_commits'])/(operation+'.yaml')).relative_to(store.root).as_posix(),'role':'history_commit','before':None,'after':C.encode_document(commit)})
    store.commit(T.PreparedMutation(operation=operation,authority=marker,baseline=captured.baseline,files=files,receipt=receipt),verify=lambda data:None)

cases=[]
for profile in ('ordinary-reader/v1','core/v1'):
  with tempfile.TemporaryDirectory() as tmp:
    root=Path(tmp).resolve(); target=root/'target'; source=root/'source'; target.mkdir(); source.mkdir()
    entry,store,marker=setup(target,profile,'target-fixture')
    src,ss,sm=setup(source,profile,'source-fixture')
    td=claim(profile,'p.dep',1,'target-dependency'); ti=claim(profile,'p.input',2,'target-input',pins={'p.dep':td['id']},body={'rests_on':['p.dep']})
    untouched=claim(profile,'p.untouched',datetime.date(2026,9,17),'target-only',body={'custom-body':{'bytes':'retain'}})
    publish(store,marker,[td,ti,untouched],'target-initial')
    sd=claim(profile,'p.dep',3,'source-dependency'); si=claim(profile,'p.input',7,'source-input',pins={'p.dep':sd['id']},body={'rests_on':['p.dep'],'source-custom': {'typed-date':datetime.date(2026,9,18)}})
    extra=claim(profile,'p.extra',True,'source-extra')
    review=C.make_object(subject='p.input',kind='act',by='reviewer',on='2026-09-18',operation='source-review',saw=[si['id']],body={'act':'review','of':si['id'],'over':[],'because':'retain review','read':{}})
    publish(ss,sm,[sd,si,extra,review],'source-initial')
    artifact=B.prepare_subset(ss.capture(),['p.input','p.extra'],scope=SCOPE,shareability='project',operation='subset-fixture',recorded_at=DAY,source_entry='GROUNDING.yaml')
    bundle=G.prepare(B.adapt(artifact).document,artifact['manifest']['roots'],scope=SCOPE,shareability='project',history=artifact)
    target_files={p.relative_to(target).as_posix():p.read_bytes() for p in target.rglob('*') if p.is_file()}
    git(target,'init','-q','-b','main');git(target,'config','user.email','fixture@example.test');git(target,'config','user.name','fixture')
    project=M.Project(target)
    with mock.patch('scripts.pending_publication.trigger_after_capture',return_value={'started':False,'reason':'fixture'}):
        G.Store(project).capture(bundle,event_id='adoption-fixture',contribution_id='adoption-fixture',shareability='project')
    head=G.Store(project).head()
    ledger={path:git(target,'show',head+':'+path) for path in git(target,'ls-tree','-r','--name-only',head).decode().splitlines()}
    base={'profile':profile,'target':enc(B.captured_files(store.capture())), 'target_files':enc(target_files),
        'artifact':G._encode({k:artifact[k] for k in ('revision','manifest')}),'artifact_files':enc(artifact['files']),
        'bundle':G._encode({k:bundle[k] for k in ('revision','manifest')}),'bundle_files':enc(bundle['files']),'ledger':enc(ledger),
        'preview':G._encode(B.preview_adoption(store.capture(),artifact))}
    for name,choices in [('incoming',{'p.input':si['id'],'p.dep':sd['id']}),('target',{'p.input':ti['id'],'p.dep':td['id'],'p.extra':extra['id']}),('missing',{'p.input':si['id']}),('invalid',{'p.input':'0'*64,'p.dep':sd['id']}),('extra-choice',{'p.input':si['id'],'p.dep':sd['id'],'not.in.artifact':si['id']})]:
        case={**base,'name':profile+'/'+name,'choices':G._encode(choices),'operation':'adopt-fixture','recorded_at':DAY,'by':'explicit adopter'}
        try:
            mutation=B.prepare_adoption(entry,artifact,choices=choices,operation='adopt-fixture',recorded_at=DAY,by='explicit adopter')
            case['mutation']=base64.b64encode(mutation.to_bytes()).decode()
        except ValueError as e: case['error']=getattr(e,'code',str(e))
        cases.append(case)
    for name,other_profile in [('no-overlap',profile),('profile-mismatch','ordinary-reader/v1' if profile=='core/v1' else 'core/v1')]:
        alternate=root/name;alternate.mkdir();ae,ast,am=setup(alternate,other_profile,'alternate-target')
        only=claim(other_profile,'private.untouched',9,'alternate-initial',body={'private':True,'custom-unknown':{'keep':True}})
        publish(ast,am,[only],'alternate-initial')
        case={**base,'name':profile+'/'+name,'target':enc(B.captured_files(ast.capture())),
            'target_files':enc({p.relative_to(alternate).as_posix():p.read_bytes() for p in alternate.rglob('*') if p.is_file()}),
            'choices':G._encode({}),'operation':'adopt-fixture','recorded_at':DAY,'by':'explicit adopter',
            'preview':G._encode(B.preview_adoption(ast.capture(),artifact))}
        try: case['mutation']=base64.b64encode(B.prepare_adoption(ae,artifact,choices={},operation='adopt-fixture',recorded_at=DAY,by='explicit adopter').to_bytes()).decode()
        except ValueError as e: case['error']=getattr(e,'code',str(e))
        cases.append(case)
    full=B.export(ss.capture(),roots=['p.dep','p.input','p.extra'],scope=SCOPE,shareability='project')
    full_bundle=G.prepare(B.adapt(full).document,full['manifest']['roots'],scope=SCOPE,shareability='project',history=full)
    with mock.patch('scripts.pending_publication.trigger_after_capture',return_value={'started':False,'reason':'fixture'}):
        G.Store(project).capture(full_bundle,event_id='full-artifact',contribution_id='full-artifact',shareability='project')
    head=G.Store(project).head()
    ledger={path:git(target,'show',head+':'+path) for path in git(target,'ls-tree','-r','--name-only',head).decode().splitlines()}
    case={**base,'name':profile+'/full-artifact','choices':G._encode({}),'operation':'adopt-fixture','recorded_at':DAY,'by':'explicit adopter',
        'artifact':G._encode({k:full[k] for k in ('revision','manifest')}),'artifact_files':enc(full['files']),
        'bundle':G._encode({k:full_bundle[k] for k in ('revision','manifest')}),'bundle_files':enc(full_bundle['files']),'ledger':enc(ledger),
        'error':'subset_adoption_required','preview_error':'subset_adoption_required'}
    case.pop('preview');cases.append(case)
a.output.write_text(json.dumps({'oracle_revision':'f480ea6','cases':cases},ensure_ascii=False,separators=(',',':'))+'\n')
print(json.dumps({'cases':len(cases),'valid':sum('mutation' in c for c in cases),'output_bytes':a.output.stat().st_size}))
