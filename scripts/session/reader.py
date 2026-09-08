"""Cards derived from the Lean core and exact raw field access."""
from __future__ import annotations
from .store import SessionService, encode
from .core import Core

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
        if premise['role']=='prior_premise': line+='; confidence belongs to this premise only'
        lines.append(line)
    falsifier=bundle['falsifier']
    outcome={True:'TRIGGERED',False:'NOT TRIGGERED',None:'UNKNOWN'}[falsifier['holds_on_current_values']]
    lines.append('EXECUTABLE FALSIFIER: '+falsifier['expression']+' => '+outcome+' ('+falsifier['reason']+')')
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
                value={'epistemic_card':card(result['bundle']),
                        'checked_bundle_ref':'checked:'+nid,'raw_ref':base+'#/body',
                        'scope':'Lean computed these fields from the supplied record; prose/world truth is not verified.'}
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
