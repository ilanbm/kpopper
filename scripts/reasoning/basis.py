"""One immutable graph index for computational input identities.

The versioned Merkle recipe hashes each local input and dependency edge once.
Strongly connected components have a canonical joint identity: cycles are
witnessed as unavailable, never evaluated by breaking an arbitrary edge. Full
sorted closure envelopes are materialized only when explicitly requested.
"""
import copy
from collections import deque


RECIPE = 'merkle-inputs/v1'


class InputBasis:
    def __init__(self, snapshot_data):
        # Local imports keep contract.node_basis free to delegate to this index.
        from .contract import PROFILE, MAX_NODES, MAX_EDGES, capabilities, digest
        from .language import node_expression, references, required_modules

        self._digest = digest
        cap = capabilities(snapshot_data['document'], profile=PROFILE)
        self._identity = {'version': 1, 'recipe': RECIPE, 'profile': PROFILE,
                          'modules': ['arithmetic/v1'],
                          'as_of': copy.deepcopy(snapshot_data.get('as_of'))}
        nodes = snapshot_data['nodes']
        if len(nodes) > MAX_NODES:
            raise ValueError('node_limit')
        if any(not isinstance(nid, str) for nid in nodes):
            raise ValueError('node IDs must be strings')
        conflicts = snapshot_data.get('context', {}).get('conflicts', {})
        if not isinstance(conflicts, dict) or any(not isinstance(nid, str) for nid in conflicts):
            raise ValueError('invalid conflicts')
        self._modules = {}
        self._atoms = {}
        self._edges = {}
        self._fingerprints = {}
        self._problems = {}
        edge_count = 0

        def normalize(node):
            try:
                expression = node_expression(node)
                unavailable = expression.get('unavailable')
                problems = {unavailable} if unavailable else set()
                return {'status': 'unavailable' if unavailable else 'present',
                        'expression': copy.deepcopy(expression)}, set(references(expression)), problems
            except (ValueError, TypeError, SyntaxError, RecursionError):
                body = node.get('body')
                if isinstance(body, dict):
                    # Preserve malformed computational input without capturing
                    # review history, assessment output, or incidental fields.
                    raw = {key: copy.deepcopy(body[key]) for key in ('rule', 'v', 'quoted') if key in body}
                else:
                    raw = copy.deepcopy(body)
                return {'status': 'invalid', 'input': raw}, set(), {'invalid_expression'}

        for nid in sorted(set(nodes) | set(conflicts)):
            local_modules = {'arithmetic/v1'}
            if nid in nodes:
                atom, refs, problems = normalize(nodes[nid])
                local_modules.update(required_modules(atom.get('expression', {})))
            else:
                atom, refs, problems = {'status': 'missing'}, set(), {'missing_reference'}
            if nid in conflicts:
                variants = []
                raw_variants = conflicts[nid]
                if not isinstance(raw_variants, (list, tuple)):
                    raise ValueError('invalid conflict variants')
                for variant in raw_variants:
                    if not isinstance(variant, (list, tuple)) or len(variant) != 2 or not isinstance(variant[0], str):
                        raise ValueError('invalid conflict variant')
                    holder, body = variant
                    other, other_refs, other_problems = normalize({
                        'body': body, 'fields': nodes.get(nid, {}).get('fields', {})})
                    local_modules.update(required_modules(other.get('expression', {})))
                    refs.update(other_refs)
                    problems.update(other_problems)
                    variants.append({'holder': holder, 'input': other})
                atom = {'status': 'contested', 'base': atom,
                        'variants': sorted(variants, key=digest)}
                problems.add('contested')
            self._modules[nid] = local_modules
            self._atoms[nid] = atom
            self._edges[nid] = tuple(sorted(refs))
            self._problems[nid] = frozenset(problems)
            edge_count += len(refs)
            if edge_count > MAX_EDGES:
                raise ValueError('edge_limit')

        # Missing targets remain explicit nodes in the identity graph.
        missing = {target for targets in self._edges.values() for target in targets} - set(self._edges)
        for nid in sorted(missing):
            self._modules[nid] = {'arithmetic/v1'}
            self._atoms[nid] = {'status': 'missing'}
            self._modules[nid] = frozenset({'arithmetic/v1'})
            self._edges[nid] = ()
            self._problems[nid] = frozenset({'missing_reference'})
        self._index_components()

    def _index_components(self):
        """Iterative Kosaraju, followed by dependency-first condensation traversal."""
        order, visited = [], set()
        for root in sorted(self._edges):
            if root in visited:
                continue
            visited.add(root)
            pending = [(root, iter(self._edges[root]))]
            while pending:
                nid, children = pending[-1]
                child = next(children, None)
                if child is None:
                    pending.pop()
                    order.append(nid)
                elif child not in visited:
                    visited.add(child)
                    pending.append((child, iter(self._edges[child])))

        reverse = {nid: [] for nid in self._edges}
        for nid, targets in self._edges.items():
            for target in targets:
                reverse[target].append(nid)
        components, owner = [], {}
        for root in reversed(order):
            if root in owner:
                continue
            number = len(components)
            owner[root] = number
            pending, members = [root], []
            while pending:
                nid = pending.pop()
                members.append(nid)
                for parent in reverse[nid]:
                    if parent not in owner:
                        owner[parent] = number
                        pending.append(parent)
            components.append(sorted(members))

        dependencies = [set() for _ in components]
        dependants = [set() for _ in components]
        for nid, targets in self._edges.items():
            source = owner[nid]
            for target in targets:
                destination = owner[target]
                if source != destination:
                    dependencies[source].add(destination)
                    dependants[destination].add(source)
        remaining = [len(items) for items in dependencies]
        ready = deque(i for i, count in enumerate(remaining) if count == 0)
        while ready:
            number = ready.popleft()
            members = components[number]
            cyclic = len(members) > 1 or members[0] in self._edges[members[0]]
            problems, modules = set(), {'arithmetic/v1'}
            for nid in members:
                modules.update(self._modules[nid])
                problems.update(self._problems[nid])
                for target in self._edges[nid]:
                    if owner[target] != number:
                        modules.update(self._modules[target])
                        problems.update(self._problems[target])
            identity = {**self._identity, 'modules': sorted(modules)}
            if cyclic:
                problems.add('cyclic_reference')
                component_hash = self._digest({**identity, 'kind': 'cycle',
                    'members': [{'id': nid, 'input': self._atoms[nid],
                                 'references': list(self._edges[nid])} for nid in members],
                    'external_dependencies': [
                        {'from': nid, 'id': target, 'fingerprint': self._fingerprints[target]}
                        for nid in members for target in self._edges[nid] if owner[target] != number]})
                for nid in members:
                    self._fingerprints[nid] = self._digest({**identity,
                        'kind': 'cycle-member', 'id': nid, 'component': component_hash})
            else:
                nid = members[0]
                self._fingerprints[nid] = self._digest({**identity, 'kind': 'node',
                    'id': nid, 'input': self._atoms[nid],
                    'dependencies': [self._witness(target) for target in self._edges[nid]]})
            for nid in members:
                self._modules[nid] = frozenset(modules)
                self._problems[nid] = frozenset(problems)
            for dependant in dependants[number]:
                remaining[dependant] -= 1
                if remaining[dependant] == 0:
                    ready.append(dependant)
        if len(self._fingerprints) != len(self._edges):
            raise ValueError('unresolved computational component')
        # Do not retain duplicate normalized bodies after the fixed index exists.
        self._atoms.clear()

    def _ensure_node(self, nid):
        if not isinstance(nid, str):
            raise ValueError('node ID must be a string')
        if nid not in self._fingerprints:
            self._modules[nid] = frozenset({'arithmetic/v1'})
            self._edges[nid] = ()
            self._problems[nid] = frozenset({'missing_reference'})
            self._fingerprints[nid] = self._digest({**self._identity, 'kind': 'node',
                'id': nid, 'input': {'status': 'missing'}, 'dependencies': []})

    def _witness(self, nid):
        return {'kind': 'node', 'id': nid, 'fingerprint': self._fingerprints[nid]}

    def fingerprint(self, nid):
        self._ensure_node(nid)
        return self._fingerprints[nid]

    def modules(self, root_ids):
        required = {'arithmetic/v1'}
        for nid in root_ids:
            self._ensure_node(nid)
            required.update(self._modules[nid])
        return sorted(required)

    def dependencies(self, root_ids):
        """Sorted complete closure with transitive fingerprints, including roots."""
        pending, seen = [], set()
        for nid in root_ids:
            self._ensure_node(nid)
            pending.append(nid)
        while pending:
            nid = pending.pop()
            if nid in seen:
                continue
            seen.add(nid)
            pending.extend(self._edges[nid])
        return [self._witness(nid) for nid in sorted(seen)]

    def basis(self, nid):
        self._ensure_node(nid)
        dependencies = self.dependencies([nid])
        envelope = {**copy.deepcopy(self._identity), 'modules': sorted(self._modules[nid]),
                    'dependencies': dependencies}
        envelope['digest'] = self._digest(envelope)
        return {'status': 'unavailable' if self._problems[nid] else 'available',
                'diagnostics': sorted(self._problems[nid]),
                'fingerprint': self._fingerprints[nid], 'dependencies': dependencies,
            'basis': envelope}

    def summary(self, nid):
        """A constant-time identity/availability view without copying ancestors."""
        self._ensure_node(nid)
        return {'status': 'unavailable' if self._problems[nid] else 'available',
                'diagnostics': sorted(self._problems[nid]), 'fingerprint': self._fingerprints[nid]}
