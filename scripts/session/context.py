"""Bounded exact reads along declared edges, with an explicit unread frontier."""
from collections import defaultdict,deque
from .model import BudgetTooSmall,encode

RELATIONS=frozenset({'rests_on','from','rule_reads'})


def context_packet(graph,read_value,ids,direction,project,revision,encoder,tokens,depth=1,max_nodes=16):
    if direction not in {'support','impact'}:raise ValueError('context direction must be support or impact')
    if type(depth) is not int or not 0<=depth<=4:raise ValueError('context depth must be0..4')
    if type(max_nodes) is not int or not 1<=max_nodes<=64:raise ValueError('context max_nodes must be1..64')
    if (not isinstance(ids,list) or not 1<=len(ids)<=8 or
            any(not isinstance(n,str) or not n or len(n)>500 for n in ids)):
        raise ValueError('context requires1..8 nonempty node IDs or node: references of at most500 characters')
    seeds=[]
    for ref in ids:
        nid=ref[5:] if ref.startswith('node:') and '#' not in ref else ref
        if nid not in graph.nodes:raise ValueError('unknown context node; use a known ID or exact node: reference')
        if nid not in seeds:seeds.append(nid)
    if max_nodes<len(seeds):raise ValueError('context max_nodes cannot omit a requested seed')
    links=defaultdict(list)
    for edge in graph.edges:
        if edge['rel'] not in RELATIONS:continue
        start,end=(edge['from'],edge['to']) if direction=='support' else (edge['to'],edge['from'])
        links[start].append((end,edge))
    for values in links.values():values.sort(key=lambda item:(item[0],encode(item[1])))
    paths={nid:[] for nid in seeds};queue=deque(seeds)
    while queue and len(paths)<max_nodes:
        nid=queue.popleft();path=paths[nid]
        if len(path)>=depth:continue
        for neighbor,edge in links[nid]:
            if neighbor in paths or neighbor not in graph.nodes:continue
            paths[neighbor]=path+[edge];queue.append(neighbor)
            if len(paths)>=max_nodes:break
    capped=any(len(path)<depth and any(n in graph.nodes and n not in paths for n,_ in links[nid])
               for nid,path in paths.items())
    selected=[]

    def packet(items):
        included={item['id'] for item in items};frontier=[]
        for nid in sorted(set(seeds)|included):
            unread=[(neighbor,edge) for neighbor,edge in links[nid] if neighbor not in included]
            if unread:
                frontier.append({'ref':'edges:'+nid,'unread_edges':len(unread),
                                 'missing_targets':len({n for n,_ in unread if n not in graph.nodes})})
        return {'project':project,'revision':revision,
                'scope':'Selected exact record reads; declared paths are not proof and external document contents are not supplied by their locators.',
                'direction':direction,'depth':depth,'max_nodes':max_nodes,'seed_refs':['node:'+n for n in seeds],
                'reads':items,'candidates':len(paths),'candidate_limit_reached':capped,
                'unread_candidates':len(paths)-len(included),
                'omitted_for_budget':['node:'+n for n in paths if n not in included],
                'unread_seed_refs':['node:'+n for n in seeds if n not in included],
                'frontier':frontier,
                'frontier_scope':'Remaining edges from seeds and returned reads, in the requested direction; not full graph closure.',
                'root_ref':'/',
                'next':'Read node:ID or node:ID#/body fields for omitted values and edges:ID for exact incident links. Continue global search for unlinked or lower-ranked qualifications.'}

    def render(items):return encode(packet(items))+'\n'
    if len(encoder.encode(render([])))>tokens:
        raise BudgetTooSmall('budget cannot carry context boundaries; raise tokens or reduce depth/max_nodes')
    for nid,path in paths.items():
        # One validated graph is shared by every read, including existing Lean cards.
        # Read failures propagate; they are never converted into absent evidence.
        value=read_value(nid)
        trial=selected+[{'id':nid,'ref':'node:'+nid,'complete':True,'via':path,'value':value}]
        if len(encoder.encode(render(trial)))<=tokens:selected=trial
    return render(selected)
