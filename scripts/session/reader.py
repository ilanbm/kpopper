"""Cards derived from the Lean core and exact raw field access."""
from __future__ import annotations
import copy

from .store import CapturedSessionService, SessionService, encode
from .core import Core
from ..expressions import text as expression_text
from ..pending_grounding import _decode as typed_decode, _encode as typed_encode

def card(bundle):
    """Format only facts computed by Lean; no status/value inference in this layer."""
    lines=['RECORDED CLAIM '+bundle['id']+': '+bundle['recorded_claim'],
           'PREMISES — current recorded value / value at review:']
    if bundle.get('recorded_status') is not None:
        lines.insert(1,'RECORDED STATUS (not evaluated): '+encode(bundle['recorded_status']))
    for premise in bundle['premises']:
        state={True:'changed',False:'unchanged',None:'unknown'}[premise['changed']]
        current='UNKNOWN' if premise['current_recorded_value'] is None else encode(premise['current_recorded_value'])
        old='same' if premise['changed'] is False else ('UNKNOWN' if premise['value_at_review'] is None else encode(premise['value_at_review']))
        line=premise['id']+': '+current+' / '+old+' ['+state+']'
        if premise.get('rule_changed') is True: line+='; formula changed'
        if premise['role']=='prior_premise': line+='; confidence belongs to this premise only'
        lines.append(line)
    falsifier=bundle['falsifier']
    outcome={True:'TRIGGERED',False:'NOT TRIGGERED',None:'UNKNOWN'}[falsifier['holds_on_current_values']]
    lines.append('EXECUTABLE FALSIFIER FOR '+bundle['id']+': '+expression_text(falsifier['expression'])+' => '+outcome+' ('+falsifier['reason']+')')
    trigger={True:'YES',False:'NO',None:'UNKNOWN'}[bundle['mechanical_review_trigger']]
    lines.append('MECHANICAL REVIEW TRIGGER: '+trigger+'; human conditions are not evaluated')
    if bundle['human_reopener']['declaration'] is not None:
        lines.append('HUMAN RE-OPENER (not evaluated): '+encode(bundle['human_reopener']['declaration']))
    if bundle['declared_gap']['declaration'] is not None:
        lines.append('RECORDED blocked_on DECLARATION (not evaluated): '+encode(bundle['declared_gap']['declaration']))
    confidence=bundle['declared_judgment_confidence']
    lines.append('JUDGMENT CONFIDENCE: '+('not recorded; do not inherit premise confidence' if confidence is None else encode(confidence)))
    lines.append('Scope: this record snapshot; source truth and action authority are not certified.')
    return '\n'.join(lines)

class CheckedSessionService(SessionService):
    """Default judgment reads now pass through Lean. Exact raw fields remain addressable."""
    def __init__(self,*args,**kwargs):
        super().__init__(*args,**kwargs)
        self.core=Core()

    def read_value(self,graph,ref):
        base,separator,pointer=ref.partition('#')
        if base.startswith('node:'):
            nid=base[5:]
            checked_fields={'epistemic_card','checked_bundle_ref','raw_ref','scope'}
            selected=pointer[1:].split('/',1)[0] if pointer.startswith('/') else ''
            if nid in graph.nodes and graph.nodes[nid]['kind']=='judgment' and (not separator or selected in checked_fields):
                result=self.core.assess(graph.data,nid)
                value={'body':graph.nodes[nid]['body'], 'epistemic_card':card(result['bundle']),
                        'checked_bundle_ref':'checked:'+nid,'raw_ref':base+'#/body',
                        'scope':'Checks and source fields belong to this ID. Read a premise itself to establish its fields or their absence. World truth and action authority are not verified.'}
                return pointer_value(value,pointer) if separator and pointer else value
        if base.startswith('checked:'):
            result=self.core.assess(graph.data,base[8:])['bundle']
            return pointer_value(result,pointer) if separator and pointer else result
        return super().read_value(graph,ref)

def pointer_value(value,pointer):
    if not pointer.startswith('/'): raise ValueError('field selector must be a JSON pointer')
    for component in pointer[1:].split('/'):
        key=component.replace('~1','/').replace('~0','~')
        if isinstance(value,dict) and key in value: value=value[key]
        elif isinstance(value,list) and key.isdigit() and int(key)<len(value): value=value[int(key)]
        else: raise ValueError('unknown checked field')
    return value


class CoreSessionReader(CapturedSessionService):
    """Exact reads from a retained ``CapturedAssessment`` projection."""
    def read_value(self, graph, ref):
        base, separator, pointer = ref.partition('#')
        if base.startswith('checked:'):
            raise ValueError('checked: handles belong to checked-reader/v1; use finding:ID')
        if base.startswith('finding:'):
            identifier = base[8:]
            finding = graph.data.get('core_findings', {}).get(identifier)
            if finding is None:
                raise ValueError('unknown core finding')
            value = typed_decode(copy.deepcopy(finding))
            if separator and pointer:
                value = pointer_value(value, pointer)
            return {'encoding': 'typed-json/v1', 'value': typed_encode(value)}
        if base.startswith('history:'):
            identifier = base[8:]
            finding = graph.data.get('core_history_subjects', {}).get(identifier)
            if finding is None:
                raise ValueError('unknown core history subject')
            value = typed_decode(copy.deepcopy(finding))
            if separator and pointer:
                value = pointer_value(value, pointer)
            return {'encoding': 'typed-json/v1', 'value': typed_encode(value)}
        if base == 'history':
            value = {identifier: typed_decode(copy.deepcopy(finding))
                     for identifier, finding in graph.data.get('core_history_subjects', {}).items()}
            if separator and pointer:
                value = pointer_value(value, pointer)
            return {'encoding': 'typed-json/v1', 'value': typed_encode(value)}
        if base.startswith('node:') and base[5:] in graph.nodes:
            node = graph.nodes[base[5:]]
            value = {'body': copy.deepcopy(node['body']),
                     'body_encoding': node.get('body_encoding'),
                     'finding_ref': 'finding:' + base[5:],
                     'status': copy.deepcopy(node['core_status']),
                     'status_text': node['core_status_text'],
                     'dependencies': copy.deepcopy(node['core_dependencies']),
                     'scope': ('Captured core/v1 findings for this node; world truth and '
                               'action authority are not certified.')}
            return pointer_value(value, pointer) if separator and pointer else value
        return super().read_value(graph, ref)
