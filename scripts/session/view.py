"""A guarded epistemic view and revision-bound session service."""
from __future__ import annotations
from collections import Counter, defaultdict
import copy
import json
from pathlib import Path
from .core import Core
from .model import MapTree, BudgetTooSmall
from .reader import CheckedSessionService, pointer_value
from .store import RULES, TextEncoder, encode, digest

META_REFS = {'orientation', 'assessment', 'pending', 'native'}

def apply_profile(data,profile):
    """Declared navigation and an editorial orientation; neither implies a judgment."""
    data=copy.deepcopy(data)
    if not profile: return data
    groups=profile['groups']; assigned={}
    for path,ids in groups.items():
        if not isinstance(path,str) or not path or not all(path.split('/')): raise ValueError('invalid profile path')
        if not isinstance(ids,list): raise ValueError('profile group must list IDs')
        for nid in ids:
            if nid in assigned: raise ValueError('profile assigns an ID more than once')
            assigned[nid]=path.split('/')
    data['topics']={nid:assigned.get(nid,['catalog']+nid.split('.')[:-1]) for nid in data['nodes']}
    orientation=copy.deepcopy(profile.get('orientation',{}))
    if orientation:
        if not orientation.get('text') or not orientation.get('basis'): raise ValueError('orientation needs text and basis')
        for nid in orientation['basis']:
            if nid not in data['nodes']: raise ValueError('orientation basis missing: '+nid)
    data['orientation']=orientation
    data['navigation_profile']={'description':profile.get('description','Declared navigation.'),
        'sha256':digest(profile),'unmatched_ids':sorted(set(assigned)-set(data['nodes'])),
        'opening_depth':profile.get('opening_depth',1)}
    if type(data['navigation_profile']['opening_depth']) is not int or not 1<=data['navigation_profile']['opening_depth']<=8:
        raise ValueError('opening_depth must be 1..8')
    return data


def make_events(data,scan):
    """All epistemic event payloads are computed by Lean; Python only orders keys."""
    return {key:copy.deepcopy(scan['events'][key]) for key in sorted(scan['events'],key=lambda key:int(key[1:]))}


class View:
    def __init__(self,data,scan,revision,project,encoder,pending=0,stale=0):
        self.data=data; self.graph=MapTree(data); self.scan=scan; self.revision=revision
        self.project=project; self.encoder=encoder; self.pending=pending; self.stale=stale
        self.events=make_events(data,scan)
        self.attention={nid for event in self.events.values() for nid in event['affected']}
        self.conflicts={nid for nid,node in data['nodes'].items() if 'contested' in node['states']}
        self._cells={}; self._edges_by_source=defaultdict(list)
        for index,edge in enumerate(data['edges']): self._edges_by_source[edge['from']].append(index)

    def cell(self,entry):
        entry=tuple(entry)
        if entry in self._cells: return self._cells[entry]
        kind,key=entry; members=sorted(self.graph.members(entry))
        edge_indices=sorted(i for nid in members for i in self._edges_by_source[nid])
        cell={'kind':kind,'key':key,'topic':self.graph.leaves[key] if kind=='node' else key,
            'members':members,'conflicts':sorted(set(members)&self.conflicts),
            'questions':sorted(n for n in members if 'question' in self.data['nodes'][n]['states']),
            'attention':sorted(set(members)&self.attention),'edge_indices':edge_indices,
            'relation_counts':dict(Counter(self.data['edges'][i]['rel'] for i in edge_indices)),
            'links_ref':'links:'+key}
        self._cells[entry]=cell
        return cell

    def packet(self,frontier):
        return {'schema':'kpopper.epistemic-view.v3','project':self.project,'revision':self.revision,
            'counts':self.scan['counts'],'cells':[self.cell(entry) for entry in frontier],
            'conditions_ref':'conditions:/','events':self.events,
            'orientation':self.data.get('orientation',{}),'rules':RULES,
            'links':self.data['edges'],'pending':self.pending,'stale_pending':self.stale,
            'native_hypotheses':sum(h.get('kind') != 'contribution' for h in self.data.get('native_hypotheses',{}).values()),
            'contributions':self.data.get('contributions',[]),
            'read_mode':self.data.get('read_mode','frozen')}

    def relation_counts(self,counts):
        return ' '.join(({'rests_on':'dependency_count','from':'source_count'}.get(rel,rel+'_count'))+'='+str(n)
                        for rel,n in sorted(counts.items()))

    def cell_line(self,cell):
        key=cell['key']
        if cell['kind']=='group': line='+ '+key+' ['+str(len(cell['members']))+']'
        else:
            ref='node:'+key
            line='- '+(encode(ref) if any(char.isspace() for char in ref) else ref)
        if cell['conflicts']: line+=' CONTESTED='+str(len(cell['conflicts']))
        if cell['questions']: line+=' questions='+str(len(cell['questions']))
        if cell['attention']: line+=' review='+str(len(cell['attention']))
        if cell['relation_counts']: line+=' '+self.relation_counts(cell['relation_counts'])
        return line

    def event_line(self,key,row):
        line='@event:'+key+' '
        if row['kind']=='premise_change':
            line+=row['id']+' '
            values='seen='+encode(row['value_at_review'])+' current='+encode(row['current_recorded_value'])
            if len(self.encoder.encode(values))>32:
                values='text changed' if row['role']=='recorded_judgment_claim' else 'recorded value changed'
            line+=values
        elif row['kind'] in {'contested','unreadable'}: line+=row['id']+' '+row['kind'].upper()
        else:
            predicate=row['falsifier']; state={True:'TRIGGERED',False:'NOT_TRIGGERED',None:'UNKNOWN'}[predicate['holds_on_current_values']]
            detail=row['id']+' '+predicate['expression']+' '+state
            line+=detail if len(self.encoder.encode(detail))<=48 else row['id']+' '+row['kind'].upper()
        return line

    def map_lines(self,cells,path='/'):
        lines=[]; context=None
        for cell in cells:
            if cell['kind']=='node':
                if cell['topic']!=context and cell['topic']!=path: lines.append('@ '+cell['topic'])
                context=cell['topic']
            else: context=None
            lines.append(self.cell_line(cell))
        return lines

    def render_open(self,packet,events_expanded):
        c=packet['counts']; orientation=packet['orientation']
        purpose=orientation.get('text',self.data.get('scope',''))
        lines=[RULES,'',f'project={self.project} revision={self.revision}',
            'purpose='+encode(purpose)+' @orientation',
            f"record: {c['nodes']} ids; {c['judgments']} judgments; contested={c['contested']}; changed={c['premise_changed']}; unreadable={c['errors']}",
            f"executable falsifiers: triggered={c['falsifier_triggered']} not_triggered={c['falsifier_not_triggered']} unknown={c['falsifier_unknown']} absent={c['falsifier_not_declared']}",
            f"prose declarations (not evaluated): blocked_on={c['gap_declared']} reopened_by={c['human_reopener_declared']} @conditions:/",
            'current=recorded; seen=review snapshot. Not triggered does not mean verified.',
            f"pending={self.pending} stale={self.stale}; native hypotheses={packet['native_hypotheses']}",
            'events='+str(len(self.events))+' @events:/']
        if not (self.pending or self.stale or packet['native_hypotheses']):
            lines=[line for line in lines if not line.startswith('pending=')]
        for contribution in packet.get('contributions', []):
            lines.append('PENDING '+contribution['revision'][:12]+' '+contribution['state']+' '+encode(contribution['scope'])+' @native')
        if events_expanded: lines.extend(self.event_line(key,row) for key,row in self.events.items())
        lines.append('MAP / — declared navigation; names do not establish claims:')
        lines.extend(self.map_lines(packet['cells']))
        if packet.get('link_map'):
            lines.append('LINK MAP — folded endpoints; all links counted; exact links at links:/')
            lines.extend(row['from']+' '+row['rel']+' '+row['to']+' ['+str(len(row['edge_indices']))+']'
                         for row in packet['link_map'])
        lines.append('Counts describe links: dependency_count=rests_on, source_count=from; not proof. Read node:ID, /topic, links:ID or @ref without @; pass revision.')
        return '\n'.join(lines)+'\n'

    def refine(self,frontier,render,tokens):
        """Spend remaining room on complete sibling expansions at mixed depths.

        Prefer additional leaf names per token, then additional branches per
        token. Ties retain the declared tree order. This is a structural greedy
        heuristic, not semantic relevance or a globally optimal packing claim.
        Every replacement retains all other cells and all children of its group.
        """
        frontier=list(frontier); text=render(frontier)
        current_tokens=len(self.encoder.encode(text))
        while True:
            best=None
            for index,(kind,key) in enumerate(frontier):
                if kind!='group': continue
                children=self.graph.successors(key)
                if not children or children==[(kind,key)]: continue
                candidate=frontier[:index]+children+frontier[index+1:]
                rendered=render(candidate); size=len(self.encoder.encode(rendered))
                if size>tokens: continue
                added=max(1,size-current_tokens)
                score=(sum(k=='node' for k,_ in children)/added,
                       (len(children)-1)/added,-size,-index)
                if best is None or score>best[0]: best=(score,candidate,rendered,size)
            if best is None: return frontier,text
            _,frontier,text,current_tokens=best

    def link_map(self,frontier):
        owner={nid:key for entry in frontier for nid in self.graph.members(entry) for key in [entry[1]]}
        rows={}
        for i,edge in enumerate(self.data['edges']):
            source=owner[edge['from']]; target=owner.get(edge['to'],'unresolved:'+edge['to'])
            key=(source,edge['rel'],target)
            rows.setdefault(key,[]).append(i)
        return [{'from':source,'rel':rel,'to':target,'edge_indices':indices}
                for (source,rel,target),indices in sorted(rows.items())]

    def opening(self,tokens):
        frontier=[('group','/')]; expanded=False
        packet=self.packet(frontier); text=self.render_open(packet,expanded)
        if len(self.encoder.encode(text))>tokens:
            raise BudgetTooSmall('minimum complete opening needs '+str(len(self.encoder.encode(text)))+' reference tokens')
        # The profile can declare a useful opening depth. All selected layers
        # remain complete; event details use room after this navigation minimum.
        levels=list(self.graph.layers())
        preferred=self.data.get('navigation_profile',{}).get('opening_depth',1)
        for level in levels[1:preferred+1]:
            candidate=self.packet(level); rendered=self.render_open(candidate,False)
            if len(self.encoder.encode(rendered))>tokens: break
            frontier=level; packet=candidate; text=rendered
        rendered=self.render_open(packet,True)
        if len(self.encoder.encode(rendered))<=tokens: expanded=True; text=rendered
        for level in levels:
            if len(level)<len(frontier): continue
            candidate=self.packet(level); rendered=self.render_open(candidate,expanded)
            if len(self.encoder.encode(rendered))>tokens: break
            frontier=level; packet=candidate; text=rendered
        frontier,text=self.refine(frontier,lambda cells:self.render_open(self.packet(cells),expanded),tokens)
        packet=self.packet(frontier)
        # Use remaining space for complete relation maps at increasing depths.
        # Nothing is selected by edge weight or alphabetic first-fit.
        for level in levels[1:]:
            candidate=copy.deepcopy(packet); candidate['link_map']=self.link_map(level)
            candidate['link_cells']=[self.cell(entry) for entry in level]
            rendered=self.render_open(candidate,expanded)
            if len(self.encoder.encode(rendered))>tokens: break
            packet=candidate; text=rendered
        packet['events_expanded']=expanded
        return {'text':text,'tokens':len(self.encoder.encode(text)),'packet':packet,'frontier':frontier,
            'visible_ids':sum(cell['kind']=='node' for cell in packet['cells'])}

    def branch(self,path,tokens):
        if path not in self.graph.groups:
            raise ValueError('unknown branch; read / with this revision to list valid branches')
        def render(level):
            lines=['revision='+self.revision,'MAP '+path]
            lines.extend(self.map_lines([self.cell(entry) for entry in level],path))
            lines.append('Counts describe links, not IDs. Read node:ID, /topic or links:ID. Names are locators.')
            return '\n'.join(lines)+'\n'
        chosen=None; frontier=None
        for level in self.graph.layers(path):
            text=render(level)
            if len(self.encoder.encode(text))>tokens: break
            chosen=text; frontier=level
        if chosen is None: raise BudgetTooSmall('budget cannot carry the complete branch root')
        return self.refine(frontier,render,tokens)[1]

    def members(self,key):
        if key in self.graph.nodes: return {key}
        if key in self.graph.groups: return self.graph.groups[key]
        raise ValueError('unknown node or topic; read / with this revision to list valid handles')

    def links(self,key):
        members=self.members(key)
        return [e for e in self.data['edges'] if e['from'] in members]

    def link_view(self,key,tokens):
        edges=self.links(key)
        lines=['revision='+self.revision,'LINKS '+key+' — source relation target; rests_on does not mean implies.']
        lines.extend(e['from']+' '+e['rel']+' '+e['to'] for e in edges)
        text='\n'.join(lines)+'\n'
        if len(self.encoder.encode(text))<=tokens: return text
        if key not in self.graph.groups:
            diagnostic='revision='+self.revision+'\n'+str(len(edges))+' links folded; read links:'+key+'#/INDEX (0..'+str(len(edges)-1)+') or raise tokens.\n'
            if len(self.encoder.encode(diagnostic))>tokens: raise BudgetTooSmall('budget cannot carry link metadata')
            return diagnostic
        chosen=None
        for level in self.graph.layers(key):
            lines=['revision='+self.revision,'LINKS '+key+' — grouped by source; expand a links: reference.']
            for entry in level:
                cell=self.cell(entry)
                lines.append('+ '+cell['links_ref']+' ['+str(len(cell['edge_indices']))+'] '+self.relation_counts(cell['relation_counts']))
            text='\n'.join(lines)+'\n'
            if len(self.encoder.encode(text))>tokens: break
            chosen=text
        if chosen is None: raise BudgetTooSmall('budget cannot carry link directory')
        return chosen


class GroundingService(CheckedSessionService):
    def __init__(self,*args,profile=None,**kwargs):
        super().__init__(*args,**kwargs)
        self.profile=Path(profile) if profile else None

    def graph(self):
        self.core.ensure_program(); graph,_=super().graph()
        profile=json.loads(self.profile.read_text(encoding='utf-8')) if self.profile else None
        data=apply_profile(graph.data,profile)
        data['project_context']=self.project
        data['epistemic_core_source']=self.core.build['source_sha256']
        routes=MapTree(data)
        data['navigation_routes']={path:sorted(members) for path,members in routes.groups.items()}
        data['navigation_leaf_routes']=routes.leaves
        graph=MapTree(data)
        return graph,digest({'project':self.project,'graph':graph.snapshot})

    def view(self,graph,revision):
        pending=self.proposals()
        return View(graph.data,self.core.scan(graph.data),revision,self.project,self.encoder,
            len(pending),sum(p['base_revision']!=revision for p in pending.values()))

    def opening_packet(self,tokens=700):
        self.budget(tokens); graph,revision=self.graph(); view=self.view(graph,revision)
        result=view.opening(tokens)
        result['guard']=self.core.guard(graph.data,result['packet'])
        return result

    def opening(self,tokens=700): return self.opening_packet(tokens)['text']

    def reading(self,ref,revision,tokens=1600,offset=None):
        self.budget(tokens)
        if ref.startswith('/') or (ref.startswith('links:') and '#' not in ref):
            if offset is not None: raise ValueError('offset applies only to exact text fields')
            graph,_=self.expect(revision); view=self.view(graph,revision)
            return view.branch(ref,tokens) if ref.startswith('/') else view.link_view(ref[6:],tokens)
        return super().reading(ref,revision,tokens,offset)

    def read_value(self,graph,ref):
        base,separator,pointer=ref.partition('#')
        if (base.startswith('source:') and base[7:] in graph.nodes
                and base[7:] not in graph.data.get('sources',{})):
            raise ValueError('this is a record entry; read node:'+base[7:]+' with this revision')
        if base in graph.nodes and base not in META_REFS: return self.read_value(graph,'node:'+ref)
        if base.startswith('node:') and base[5:] in graph.nodes and graph.nodes[base[5:]]['kind']=='judgment':
            selected=pointer[1:].split('/',1)[0] if pointer.startswith('/') else ''
            if not separator or selected in {'epistemic_card','checked_bundle_ref','raw_ref','scope','state_tags_ref','state_tags_scope'}:
                value=super().read_value(graph,base)
                scan=self.core.scan(graph.data)
                if any(event['kind']=='contested' and event['id']==base[5:] for event in scan['events'].values()):
                    value['epistemic_card']+='\nRECORDED CONFLICT: YES; this flag does not establish or refute the claim.'
                value['state_tags_ref']=base+'#/states'
                value['state_tags_scope']='Derived annotations, not the source status; read body for the recorded status and revision history.'
                return pointer_value(value,pointer) if separator and pointer else value
        revision=digest({'project':self.project,'graph':graph.snapshot})
        if base=='orientation':
            value=graph.data.get('orientation') or {'text':graph.data.get('scope',''),'basis':['record scope']}
        elif base.startswith(('event:','events:','links:','conditions:')) or base=='assessment':
            view=self.view(graph,revision)
            if base.startswith('event:'):
                if base[6:] not in view.events: raise ValueError('unknown event')
                value=view.events[base[6:]]
            elif base.startswith('events:'):
                members=view.members(base[7:])
                value={key:row for key,row in view.events.items() if set(row['affected'])&members}
            elif base.startswith('links:'): value=view.links(base[6:])
            elif base.startswith('conditions:'):
                members=view.members(base[11:])
                value={nid:row for nid,row in view.scan['conditions'].items() if nid in members}
            else: value={'counts':view.scan['counts'],'events':list(view.events),'errors':view.scan['errors']}
        else:
            try: return super().read_value(graph,ref)
            except ValueError as error:
                if str(error) in {'unknown reference','unlisted operation or identifier'}:
                    raise ValueError(str(error)+'; read / with this revision to list valid handles') from error
                raise
        return pointer_value(value,pointer) if separator and pointer else value
