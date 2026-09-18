"""Reviewed first-party modules consume granted snapshot views and typed IR.

Module discovery is static product code. Record text can request a capability but
cannot load a module, name a program, or grant itself a larger snapshot view.
"""
import copy

from .contract import MODULES, CapabilityError
from .language import children, required_modules


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
            pending.extend(children(item))
        # The core compiler supplies normalized IR. The module can see only the
        # immutable nodes explicitly granted for this request, including the
        # transitive formula closure discovered before execution.
        for nid in normalized_nodes:
            if view.read_node(nid) is None:
                raise ValueError('module input was not captured')
        return {'nodes': copy.deepcopy(normalized_nodes), 'expression': copy.deepcopy(expression),
                'declared': list(declared), 'limits': dict(limits)}


class CompositionModule:
    identity = 'composition/v1'
    specification = MODULES[identity]

    @classmethod
    def prepare(cls, view, expression, normalized_nodes, declared, limits):
        for nid in normalized_nodes:
            if view.read_node(nid) is None:
                raise ValueError('module input was not captured')
        return {'protocol': 'KP3', 'nodes': copy.deepcopy(normalized_nodes),
                'expression': copy.deepcopy(expression), 'declared': list(declared), 'limits': dict(limits)}


class QueryModule:
    """The reviewed finite-scope adapter; native code owns row semantics."""
    identity = 'query/v1'
    specification = MODULES[identity]

    @classmethod
    def prepare(cls, capture, authored_operation, *, request_id, root_witness,
                declared_capabilities, limits):
        from . import query
        return query.prepare(
            capture, authored_operation, request_id=request_id,
            root_witness=root_witness,
            declared_capabilities=declared_capabilities, limits=limits)

    @staticmethod
    def validate(prepared):
        from . import query
        return query.validate_prepared(prepared)

    @staticmethod
    def _validate_prepared(prepared):
        from . import query
        return query._validate_prepared(prepared)

    @staticmethod
    def decode_response(response, prepared):
        from . import query
        return query.validate_response(response, prepared)

    @staticmethod
    def finalize_basis(prepared, response):
        from . import query
        return query.finalize_basis(prepared, response)

    @staticmethod
    def _validated_response_and_basis(response, prepared):
        from . import query
        return query._validated_response_and_basis(response, prepared)


REGISTRY = {
    ArithmeticModule.identity: ArithmeticModule,
    CompositionModule.identity: CompositionModule,
    QueryModule.identity: QueryModule,
}


def module(identity):
    try:
        return REGISTRY[identity]
    except KeyError:
        raise CapabilityError('unsupported_capability', 'module is not installed: ' + identity) from None
