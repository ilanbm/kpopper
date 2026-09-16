"""Reviewed first-party modules consume granted snapshot views and typed IR.

Module discovery is static product code. Record text can request a capability but
cannot load a module, name a program, or grant itself a larger snapshot view.
"""
import copy

from .contract import MODULES, CapabilityError


class ArithmeticModule:
    identity = 'arithmetic/v1'
    specification = MODULES[identity]

    @classmethod
    def prepare(cls, view, expression, normalized_nodes, declared, limits):
        pending = [expression, *normalized_nodes.values()]
        while pending:
            item = pending.pop()
            if 'op' in item and item['op'] not in cls.specification['operators']:
                raise CapabilityError('unsupported_capability', 'operator is not registered in arithmetic/v1')
            pending.extend(item.get('args', []))
        # The core compiler supplies normalized IR. The module can see only the
        # immutable nodes explicitly granted for this request, including the
        # transitive formula closure discovered before execution.
        for nid in normalized_nodes:
            if view.read_node(nid) is None:
                raise ValueError('module input was not captured')
        return {'nodes': copy.deepcopy(normalized_nodes), 'expression': copy.deepcopy(expression),
                'declared': list(declared), 'limits': dict(limits)}


REGISTRY = {ArithmeticModule.identity: ArithmeticModule}


def module(identity):
    try:
        return REGISTRY[identity]
    except KeyError:
        raise CapabilityError('unsupported_capability', 'module is not installed: ' + identity) from None
