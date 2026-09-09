"""Project-bound epistemic context with durable proposals; no canonical record writes."""
from __future__ import annotations
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.parse import urlsplit

import tiktoken
from .model import RecordMap, BudgetTooSmall, encode, digest

ROOT=Path(__file__).resolve().parent
RULES=(ROOT/'rules.txt').read_text(encoding='utf-8').strip()
BOOTSTRAP=(ROOT/'bootstrap.txt').read_text(encoding='utf-8').strip()


class TextEncoder:
    """Tokenizer marker spellings inside evidence are ordinary text."""
    def __init__(self,name):
        self.inner=tiktoken.get_encoding(name); self.name=name
    def encode(self,text): return self.inner.encode(text,disallowed_special=())


def atomic_create(path, value):
    """Publish a complete file without overwriting another process's result."""
    payload=encode(value)+'\n'
    fd,temporary=tempfile.mkstemp(prefix='.pending-',dir=path.parent)
    try:
        with os.fdopen(fd,'w',encoding='utf-8') as out:
            out.write(payload); out.flush(); os.fsync(out.fileno())
        try: os.link(temporary,path)
        except FileExistsError: pass
    finally:
        os.unlink(temporary)
    return json.loads(path.read_text(encoding='utf-8'))


def normalize(value):
    if isinstance(value,dict): return {k:normalize(v) for k,v in value.items()}
    if isinstance(value,(list,tuple)): return [normalize(v) for v in value]
    if isinstance(value,set): return sorted(normalize(v) for v in value)
    if value is None or isinstance(value,(str,int,float,bool)): return value
    return str(value)


def native_record(path, reader_path):
    """Use the configured product reader; do not infer semantic relationships from names."""
    if not path.exists():
        return {'nodes':{},'edges':[],'topics':{},'scope':'No record yet. Material learning can be proposed.',
                'sources':{},'native_hypotheses':{}}
    spec=importlib.util.spec_from_file_location('_configured_kpopper_reader',reader_path)
    p=importlib.util.module_from_spec(spec); spec.loader.exec_module(p)
    files=[Path(f).resolve() for f in p._files_of([str(path)])]
    captured={f:f.read_bytes() for f in files}
    doc=p.load([str(path)])
    if files!=[Path(f).resolve() for f in p._files_of([str(path)])] or any(f.read_bytes()!=b for f,b in captured.items()):
        raise ValueError('record sources changed during read; retry')
    sources={}; origins={}
    for f,raw_bytes in captured.items():
        handle='record' if f==path.resolve() else 'record.'+hashlib.sha256(str(f).encode()).hexdigest()[:16]
        sources[handle]={'text':raw_bytes.decode('utf-8'),'sha256':hashlib.sha256(raw_bytes).hexdigest(),
                         'location':str(f)}
        loaded=p.yaml.safe_load(raw_bytes) or {}
        for section,members in loaded.items():
            if isinstance(members,dict):
                origins.setdefault(section,{}).update({nid:handle for nid in members})
            else: origins.pop(section,None)
    collections={k:v for k,v in p.collections_of(doc).items() if k!='meta'}
    try:
        all_ids,judgments,fields=p.infer(doc)
        raw=p.with_builtins(doc,all_ids,judgments,fields)
        flags=p.flags(all_ids,judgments,fields,raw)
    except SystemExit as failure:
        judgment_fields={'rests_on','wrong_if','seen','verdict','reopened_by','blocked_on'}
        source_only=set(collections)<={'known','sources','open','questions'} and not any(
            judgment_fields.intersection(body) for group in collections.values() for body in group.values()
            if isinstance(body,dict))
        if 'no dependency field found' not in str(failure) or not source_only:
            raise ValueError(str(failure))
        # A source-only record has no judgments to evaluate; do not invent a sentinel.
        all_ids={nid for group in collections.values() for nid in group}
        judgments={}; raw=p.bodies(doc); flags={}
    ids={nid for group in collections.values() for nid in group}
    ids.update(nid for nid in all_ids if p.is_builtin(nid))
    sections={nid:section for section,group in collections.items() for nid in group}
    questions={nid for section in p.OPEN for nid in (doc.get(section) or {})}
    disputed=p.contested(doc)
    nodes={}; edges=[]
    for nid in sorted(ids):
        source_body=raw.get(nid)
        body=source_body if isinstance(source_body,dict) else {'v':source_body}
        states=set(flags.get(nid,[]))
        if nid in questions: states.add('question')
        if nid in disputed: states.add('contested')
        if nid.startswith('prior.'): states.add('prior')
        if p._blocked_text(body): states.add('declared_gap')
        if p._reopened_text(body): states.add('human_reopener')
        kind='judgment' if nid in judgments else ('computed' if p.is_builtin(nid) else sections.get(nid,'entry'))
        nodes[nid]={'kind':kind,'states':sorted(states),'body':body}
        if not isinstance(source_body,dict): nodes[nid]['source_body']=source_body
        if nid in judgments:
            # Preserve source spelling in body; the kernel receives the reader's
            # declared/inferred roles separately, never a guessed field vocabulary.
            assessed=dict(body)
            for role,canonical in [('deps','rests_on'),('snapshot','seen'),('predicate','wrong_if')]:
                source_field=fields.get(role)
                if source_field: assessed[canonical]=body.get(source_field)
            nodes[nid]['assessment_body']=assessed
            nodes[nid]['assessment_fields']=dict(fields)
        if nid in origins.get(sections.get(nid),{}):
            nodes[nid]['record_source']=origins[sections[nid]][nid]
        else: nodes[nid]['record_sources']=sorted(sources)
        if nid in judgments:
            deps=nodes[nid]['assessment_body'].get('rests_on')
            if isinstance(deps,list) and all(isinstance(dep,str) for dep in deps):
                edges.extend({'from':nid,'rel':'rests_on','to':dep} for dep in deps)
        if isinstance(body.get('from'),str) and body['from'] in ids:
            edges.append({'from':nid,'rel':'from','to':body['from']})
        if nid not in judgments:
            for field in ['rule','v']:
                value=body.get(field)
                if isinstance(value,str) and p.EXPR.search(value):
                    edges.extend({'from':nid,'rel':'rule_reads','to':ref}
                                 for ref in sorted(set(p.ID.findall(value))) if ref in ids and ref!=nid)
    # Relations are a set; process-specific hash order must not change a revision.
    unique_edges={encode(edge):edge for edge in edges}
    data={'nodes':nodes,'edges':[unique_edges[key] for key in sorted(unique_edges)],
          'topics':{nid:[sections.get(nid,'computed')]+nid.split('.')[:-1] for nid in nodes},
          'scope':(doc.get('meta') or {}).get('scope','Epistemic project record.'),
          'sources':sources,
          'native_hypotheses':getattr(doc,'hypotheses',{}),
          'origin':{'reader_sha256':hashlib.sha256(reader_path.read_bytes()).hexdigest(),
                    'flags':'record reader only; page validation not run; native hypotheses readable separately'}}
    return normalize(data)


class SessionService:
    def __init__(self,project,input_path,state_dir,reader=None,encoding='o200k_base'):
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}',project): raise ValueError('invalid project name')
        self.project=project; self.input_path=Path(input_path).resolve()
        self.reader=Path(reader).resolve() if reader else None
        self.state_dir=Path(state_dir).resolve()
        self.encoder=TextEncoder(encoding)
        self.state_dir.mkdir(parents=True,exist_ok=True,mode=0o700)
        identity={'project':project,'input_path':str(self.input_path)}
        held=atomic_create(self.state_dir/'project.json',identity)
        if held!=identity: raise ValueError('state directory belongs to another project/input')

    def graph(self):
        data=native_record(self.input_path,self.reader) if self.reader else json.loads(self.input_path.read_text(encoding='utf-8'))
        graph=RecordMap(data)
        revision=digest({'project':self.project,'graph':graph.snapshot})
        return graph,revision

    def proposals(self):
        result={}
        for path in sorted(self.state_dir.glob('proposal-*.json')):
            proposal=json.loads(path.read_text(encoding='utf-8'))
            core={k:proposal[k] for k in ['project','base_revision','kind','text','basis','revisit']}
            key=digest(core)
            if (proposal.get('project')!=self.project or path.name!='proposal-'+key+'.json' or proposal.get('id')!=key
                    or proposal.get('status')!='proposed'):
                raise ValueError('proposal identity mismatch')
            result[key]=proposal
        return result

    @staticmethod
    def budget(tokens):
        if type(tokens) is not int or not 64<=tokens<=65536: raise ValueError('tokens must be 64..65536')

    def expect(self,revision):
        graph,current=self.graph()
        if revision!=current: raise ValueError('project record changed or revision belongs elsewhere; reopen')
        return graph,current

    def opening(self,tokens=700):
        raise NotImplementedError("a session view must supply its projection")

    def read_value(self,graph,ref):
        base,separator,pointer=ref.partition('#')
        current=digest({'project':self.project,'graph':graph.snapshot})
        pending={k:{**v,'stale_base':v['base_revision']!=current} for k,v in self.proposals().items()}
        if base=='pending': value=pending
        elif base=='native': value=graph.data.get('native_hypotheses',{})
        elif base=='alerts': value=graph.read('alerts',None,graph.snapshot)
        elif base.startswith('proposal:'):
            key=base[9:]
            if key not in pending: raise ValueError('unknown proposal')
            value=pending[key]
        elif base.startswith('node:'): value=graph.read('node',base[5:],graph.snapshot)
        elif base.startswith('source:'): value=graph.read('source',base[7:],graph.snapshot)
        elif base.startswith('edges:'): value=graph.read('edges',base[6:],graph.snapshot)
        else: raise ValueError('unknown reference')
        if separator and pointer:
            if not pointer.startswith('/'): raise ValueError('field selector must be a JSON pointer')
            for part in pointer[1:].split('/'):
                key=part.replace('~1','/').replace('~0','~')
                if isinstance(value,dict) and key in value: value=value[key]
                elif isinstance(value,list) and re.fullmatch(r'0|[1-9][0-9]*',key) and int(key)<len(value): value=value[int(key)]
                else: raise ValueError('unknown field')
        return value

    def reading(self,ref,revision,tokens=1600,offset=None):
        self.budget(tokens)
        graph,current=self.expect(revision)
        value=self.read_value(graph,ref)
        if offset is not None:
            if type(offset) is not int or not isinstance(value,str) or not 0<=offset<=len(value):
                raise ValueError('offset must address this exact text field')
            def fragment(end):
                return encode({'ref':ref,'revision':current,'complete':offset==0 and end==len(value),
                    'text_fragment':value[offset:end],'offset':offset,'next_offset':end if end<len(value) else None,
                    'total_characters':len(value),'sha256':hashlib.sha256(value.encode()).hexdigest()})
            low=offset; high=len(value)
            if len(self.encoder.encode(fragment(low)))>tokens: raise BudgetTooSmall('budget cannot carry fragment metadata')
            while low<high:
                middle=(low+high+1)//2
                if len(self.encoder.encode(fragment(middle)))<=tokens: low=middle
                else: high=middle-1
            if low==offset and offset<len(value): raise BudgetTooSmall('budget cannot carry any text')
            return fragment(low)
        response=encode({'project':self.project,'revision':current,'ref':ref,'complete':True,'value':value})
        needed=len(self.encoder.encode(response))
        if needed>tokens:
            # Directory entries retain exact field handles instead of clipping claims.
            keys=list(value) if isinstance(value,dict) else list(range(len(value))) if isinstance(value,list) else []
            children=[ref+('/' if '#' in ref else '#/')+str(key).replace('~','~0').replace('/','~1') for key in keys]
            next_step='read this text with offset=0 for labeled fragments' if isinstance(value,str) else 'read a field or raise tokens'
            diagnostic=encode({'complete':False,'required_tokens':needed,'ref':ref,'children':children,'next':next_step})
            if len(self.encoder.encode(diagnostic))>tokens:
                diagnostic=encode({'complete':False,'required_tokens':needed,'ref':ref,'next':next_step})
            if len(self.encoder.encode(diagnostic))>tokens: raise BudgetTooSmall('budget cannot carry a complete read response')
            return diagnostic
        return response

    def proposing(self,revision,kind,text,basis,revisit=''):
        graph,current=self.expect(revision)
        if kind not in {'observed','inferred','assumed','question'}: raise ValueError('unknown epistemic kind')
        if not isinstance(text,str) or not text.strip() or len(text)>8000: raise ValueError('text must contain 1..8000 characters')
        if not isinstance(basis,list) or len(basis)>30 or not all(isinstance(ref,str) and len(ref)<=500 for ref in basis):
            raise ValueError('basis must contain at most 30 reference strings')
        if not isinstance(revisit,str) or len(revisit)>2000: raise ValueError('invalid re-opener')
        if kind in {'observed','inferred'} and not basis: raise ValueError('observed/inferred claims need source or premise references')
        if kind!='question' and not revisit.strip(): raise ValueError('a consequential claim needs a falsifier or human re-opener')
        for ref in basis:
            # A proposed alternative cannot silently become an established premise.
            if ref.startswith('external:'):
                location=urlsplit(ref[9:])
                if location.scheme not in {'https','http','file'} or not (location.netloc or location.path):
                    raise ValueError('external basis needs an explicit http(s) or file locator')
            elif ref.startswith(('node:','source:')):
                self.read_value(graph,ref)
            else: raise ValueError('basis must name recorded refs or an external: locator')
        core={'project':self.project,'base_revision':current,'kind':kind,'text':text,
              'basis':sorted(set(basis)),'revisit':revisit}
        key=digest(core)
        proposal={**core,'id':key,'status':'proposed','claimed_by':'session; not authenticated as a person',
                  'unverified_external_basis':[ref for ref in core['basis'] if ref.startswith('external:')],
                  'created_at':datetime.now(timezone.utc).isoformat()}
        saved=atomic_create(self.state_dir/('proposal-'+key+'.json'),proposal)
        if any(saved.get(k)!=v for k,v in core.items()): raise ValueError('proposal collision')
        return encode({'id':key,'status':'proposed','read':'proposal:'+key,'canonical_record_changed':False})
