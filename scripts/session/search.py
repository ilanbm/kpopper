"""Rank candidate references. Matching never establishes support or changes a record."""
from __future__ import annotations
from collections import Counter
import hashlib
import json
import math
import re
import threading


def encode(value):
    return json.dumps(value,ensure_ascii=False,sort_keys=True,separators=(',',':'))


def terms(text):
    # Identifier components remain discoverable through ordinary project vocabulary.
    return re.findall(r'[^\W_]+',text.casefold(),re.UNICODE)


def mentioned_ids(text,ids):
    # Hyphens and other identifier punctuation extend a name. Common prose/ref
    # delimiters can surround one; a longer known opaque ID wins any overlap.
    atom=r'''[^\s"'`()\[\]{},;<>:#/]'''
    found=[]
    for identifier in ids:
        for match in re.finditer(r'(?<!'+atom+')'+re.escape(identifier)+r'(?!'+atom+')',text):
            found.append((match.start(),match.end(),identifier))
    accepted=[]
    for start,end,identifier in sorted(found,key=lambda row:(-(row[1]-row[0]),row[0],row[2])):
        if not any(start<other_end and end>other_start for other_start,other_end,_ in accepted):
            accepted.append((start,end,identifier))
    return list(dict.fromkeys(identifier for _,_,identifier in sorted(accepted)))


class SearchIndex:
    """One complete in-memory record index; exact IDs precede approximate matches."""
    def __init__(self,semantic=None):
        self.semantic=semantic;self.fingerprint=None;self._lock=threading.Lock()

    def prepare(self,nodes):
        documents={key:key+'\n'+encode(node['body']) for key,node in sorted(nodes.items())}
        fingerprint=hashlib.sha256(encode(documents).encode('utf-8')).hexdigest()
        if fingerprint!=self.fingerprint:
            frequencies={key:Counter(terms(text)) for key,text in documents.items()}
            self.documents=documents;self.frequencies=frequencies
            self.lengths={key:sum(value.values()) for key,value in frequencies.items()}
            self.df=Counter(term for frequency in frequencies.values() for term in frequency)
            self.average=sum(self.lengths.values())/max(1,len(self.lengths))
            self.fingerprint=fingerprint

    def lexical(self,query):
        query_terms=set(terms(query));n=len(self.documents);scores={}
        for key,frequency in self.frequencies.items():
            score=0.0
            for term in query_terms:
                count=frequency[term]
                if not count:continue
                inverse=math.log(1+(n-self.df[term]+.5)/(self.df[term]+.5))
                score+=inverse*count*2.2/(count+1.2*(.25+.75*self.lengths[key]/max(1,self.average)))
            scores[key]=score
        return scores

    def search(self,nodes,query,ids=None,limit=8,branch_members=(),mode='hybrid'):
        with self._lock:
            return self._search(nodes,query,ids,limit,branch_members,mode)

    def _search(self,nodes,query,ids,limit,branch_members,mode):
        if not isinstance(query,str) or len(query)>8000:raise ValueError('query must be at most8000 characters')
        if ids is None:ids=[]
        if not isinstance(ids,list) or len(ids)>64 or any(not isinstance(k,str) or not k or len(k)>500 for k in ids):
            raise ValueError('ids must contain at most64 nonempty identifier strings of at most500 characters')
        if type(limit) is not int or not 1<=limit<=32:raise ValueError('limit must be1..32')
        if mode not in {'lexical','semantic','hybrid'}:raise ValueError('unknown search mode')
        if not query.strip() and not ids:raise ValueError('supply a query or exact IDs')
        self.prepare(nodes);known=set(nodes);requested=list(dict.fromkeys(ids))
        # Returned node references and bare IDs denote the same exact record.
        # Field selectors and source handles retain their distinct read semantics.
        def bare(value):
            return value[5:] if value.startswith('node:') and '#' not in value else value
        explicit=list(dict.fromkeys(bare(value) for value in requested))
        unknown=[value for value in requested if bare(value) not in known]
        mentions=mentioned_ids(query,known);exact=list(dict.fromkeys([key for key in explicit if key in known]+mentions))
        branch=set(branch_members);lexical=self.lexical(query);dense=None;metadata=None;fallback=None
        if mode!='lexical' and query.strip():
            if self.semantic is None:fallback='Local embeddings are not configured; lexical ranking used.'
            else:
                try:
                    result=self.semantic.rank(self.documents,query);dense=result['scores']
                    if set(dense)!=known or any(type(v) not in (int,float) or not math.isfinite(v) for v in dense.values()):
                        raise ValueError('semantic scorer did not cover the complete record with finite scores')
                    metadata={key:value for key,value in result.items() if key!='scores'}
                except (ValueError,ImportError,OSError,RuntimeError) as error:
                    fallback='Local embeddings unavailable: '+str(error);dense=None
        # Branch hints only break exact score ties. They cannot remove better global matches.
        def order(scores):return sorted(scores,key=lambda key:(-scores[key],key not in branch,key))
        lexical_order=order({k:v for k,v in lexical.items() if v>0})
        if dense is None:backend='lexical';scores={key:lexical[key] for key in lexical_order}
        elif mode=='semantic':backend='semantic';scores=dense
        else:
            backend='hybrid';scores={}
            for ranking in [order(dense),lexical_order]:
                for position,key in enumerate(ranking,1):scores[key]=scores.get(key,0)+1/(60+position)
        ranking=list(dict.fromkeys(exact+order(scores)))
        hits=[]
        for key in ranking[:limit]:
            match='explicit_id' if key in explicit else 'literal_id' if key in mentions else backend
            hits.append({'id':key,'ref':'node:'+key,'links_ref':'edges:'+key,'match':match,'score':scores.get(key,0.0)})
        return {'hits':hits,'matched_candidates':len(ranking),'record_nodes':len(nodes),'unresolved_ids':unknown,
                'requested_mode':mode,'backend':backend,'fallback':fallback,'semantic_index':metadata,
                'index_fingerprint':self.fingerprint}
