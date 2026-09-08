"""Exact record model and navigation; names carry no truth implication."""
from __future__ import annotations
from collections import defaultdict
import copy
import hashlib
import json
from urllib.parse import quote

ALERTS = {'moved', 'falsified', 'broken', 'unchecked', 'no_predicate', 'contested'}
def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))

def digest(value):
    return hashlib.sha256(encode(value).encode()).hexdigest()


def validate_assessment_body(node):
    """A normalized role view may rename fields; it cannot mask changed source fields."""
    if 'assessment_body' not in node:
        return
    fields = node.get('assessment_fields')
    if not isinstance(fields, dict) or set(fields) - {'deps', 'snapshot', 'predicate'}:
        raise ValueError('invalid assessment field mapping')
    expected = dict(node['body'])
    for role, canonical in [('deps', 'rests_on'), ('snapshot', 'seen'), ('predicate', 'wrong_if')]:
        source = fields.get(role)
        if source is not None and not isinstance(source, str):
            raise ValueError('assessment source field must be a string')
        if source:
            expected[canonical] = node['body'].get(source)
    if encode(node['assessment_body']) != encode(expected):
        raise ValueError('assessment body is stale or disagrees with the original fields')

class BudgetTooSmall(ValueError):
    pass

class RecordMap:
    def __init__(self, data):
        self.data = copy.deepcopy(data)
        data = self.data
        self.nodes = data['nodes']
        if not all(isinstance(nid, str) and nid and nid.isprintable()
                   and not any(char in nid for char in '#:/') for nid in self.nodes):
            raise ValueError('node IDs must be printable names without #, : or / reference delimiters')
        self.snapshot = digest(data)
        self.edges = sorted(data.get('edges', []), key=encode)
        self.groups = defaultdict(set)
        self.children = defaultdict(set)
        self.leaves = {}
        self.direct = defaultdict(list)
        order = data.get('node_order', sorted(self.nodes))
        if len(order) != len(self.nodes) or set(order) != set(self.nodes):
            raise ValueError('node_order must be a permutation of node IDs')
        self.rank = {nid: index for index, nid in enumerate(order)}
        if set(data['topics']) != set(self.nodes):
            raise ValueError('topics must cover every node exactly once')
        if len({encode(e) for e in self.edges}) != len(self.edges):
            raise ValueError('duplicate edge')
        for edge in self.edges:
            if set(edge) != {'from', 'rel', 'to'} or edge['from'] not in self.nodes:
                raise ValueError('invalid edge')
            if not all(isinstance(v, str) and v for v in edge.values()):
                raise ValueError('invalid edge text')
        for nid, node in sorted(self.nodes.items()):
            if not isinstance(node['states'], list) or not isinstance(node['body'], dict):
                raise ValueError('invalid node')
            validate_assessment_body(node)
            parts = data['topics'][nid]
            if not isinstance(parts, list) or not all(isinstance(x, str) and x for x in parts):
                raise ValueError('invalid topic path')
            parent = '/'
            self.groups[parent].add(nid)
            for part in parts:
                path = parent.rstrip('/') + '/' + quote(part, safe='')
                self.children[parent].add(path)
                self.groups[path].add(nid)
                parent = path
            self.leaves[nid] = parent
            self.direct[parent].append(nid)
        self.groups['/']  # empty record remains addressable
        self._balance('/')

    def _balance(self, path, fanout=6):
        """Bound width with explicit lexical buckets, without inventing subject meaning."""
        entries = self.successors(path)
        if len(entries) > fanout:
            width = (len(entries) + fanout - 1) // fanout
            self.children[path].clear()
            self.direct[path].clear()
            for i in range(0, len(entries), width):
                bucket = path.rstrip('/') + '/@lex' + str(i // width + 1)
                self.children[path].add(bucket)
                for kind, key in entries[i:i + width]:
                    self.groups[bucket].update(self.members((kind, key)))
                    if kind == 'group': self.children[bucket].add(key)
                    else: self.direct[bucket].append(key)
        for child in sorted(self.children[path]):
            self._balance(child, fanout)

    def expect(self, snapshot):
        if digest(self.data) != self.snapshot:
            raise ValueError('snapshot data mutated')
        if snapshot is not None and snapshot != self.snapshot:
            raise ValueError('snapshot changed; reopen before expanding')

    def members(self, entry):
        kind, key = entry
        return self.groups[key] if kind == 'group' else {key}

    def successors(self, path):
        return ([('group', p) for p in sorted(self.children[path])]
                + [('node', n) for n in sorted(self.direct[path], key=self.rank.get)])

    def read(self, operation, identifier, snapshot):
        self.expect(snapshot)
        if operation == 'alerts' and identifier is None:
            return {n: sorted(ALERTS & set(node['states'])) for n, node in self.nodes.items()
                    if ALERTS & set(node['states'])}
        if operation == 'node' and identifier in self.nodes:
            return copy.deepcopy(self.nodes[identifier])
        if operation == 'node' and identifier in self.groups and len(self.groups[identifier]) == 1:
            nid = next(iter(self.groups[identifier]))
            return {'id': nid, **copy.deepcopy(self.nodes[nid])}
        if operation == 'edges' and (identifier in self.groups or identifier in self.nodes):
            members = self.groups[identifier] if identifier in self.groups else {identifier}
            return copy.deepcopy([e for e in self.edges if e['from'] in members or e['to'] in members])
        if operation == 'source' and identifier in self.data.get('sources', {}):
            source = self.data['sources'][identifier]
            if hashlib.sha256(source['text'].encode()).hexdigest() != source['sha256']:
                raise ValueError('source checksum mismatch')
            return copy.deepcopy(source)
        raise ValueError('unlisted operation or identifier')


class MapTree(RecordMap):
    """Retain semantic children; use lexical buckets only for exceptionally wide nodes."""
    def _balance(self,path,fanout=64):
        entries=self.successors(path)
        if len(entries)>fanout:
            width=max(16,(len(entries)+15)//16)
            self.children[path].clear(); self.direct[path].clear()
            for i in range(0,len(entries),width):
                bucket=path.rstrip('/')+'/@ids'+str(i//width+1)
                self.children[path].add(bucket)
                for kind,key in entries[i:i+width]:
                    self.groups[bucket].update(self.members((kind,key)))
                    if kind=='group': self.children[bucket].add(key)
                    else: self.direct[bucket].append(key)
        for child in sorted(self.children[path]): self._balance(child,fanout)

    def layers(self,path='/'):
        if path not in self.groups: raise ValueError('unknown branch')
        frontier=[('group',path)]
        yield frontier
        while True:
            following=[]; changed=False
            for kind,key in frontier:
                children=self.successors(key) if kind=='group' else []
                if children:
                    following.extend(children); changed=True
                else: following.append((kind,key))
            if not changed: return
            frontier=following
            yield frontier
