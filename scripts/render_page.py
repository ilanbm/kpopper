#!/usr/bin/env python3
"""Render a record as one self-contained HTML page - in two tabs, sharing one provenance layer.

  python3 render_page.py [file ...] > record.html
  python3 render_page.py --brief PROVENANCE.view.yaml [file ...] > record.html
  python3 render_page.py --verify [file ...]

**Now** is the tab the session writes: an arrangement of the record aimed at what this
session is for. It is opinionated on purpose - order, sections, emphasis - and it is
written in a brief the session can rewrite in a moment, because intent is the one input
that is not in the record and dies with the conversation.

**Record** is the tab nobody writes: everything, arranged by nothing but the record's own
shape. It is the fallback when the arrangement is wrong, and it is the only tab when no
brief exists.

Two guarantees keep the opinionated tab honest, and both are mechanical rather than
remembered:

  · **it may order, it may not drop.** Anything flagged that no authored section picked
    up lands in a trailing section the brief cannot switch off.
  · **sections repopulate.** A section holds a selector, not a frozen list, so a new
    blocked judgment appears in it with no edit.

And the arrangement itself can go stale: the brief records the record's *shape* when it
was written, and a shape that moved raises a banner. Shape, not values - a date changing
does not make a layout wrong; a fourth blocked judgment might.
"""
import io, os, re, sys, json, html, hashlib, pathlib, datetime, yaml
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import provenance as P

CSS = """
*{box-sizing:border-box}
:root{color-scheme:light;--pg:#f7f7f5;--sf:#fff;--ink:#111;--ink2:#555;--mut:#8a8a85;
 --ln:#e3e3dd;--acc:#2a6fd6;--warn:#9a6410;--stop:#b4342f;--ok:#0a7d38;--wash:rgba(0,0,0,.04);
 --read:#0f6b70;--pure:#4a4fa8;--mind:#8f4068}
@media(prefers-color-scheme:dark){:root:not([data-theme=light]){color-scheme:dark;
 --pg:#0d0e0e;--sf:#181a19;--ink:#f2f2ef;--ink2:#bdbdb5;--mut:#85857e;--ln:#2a2c2b;
 --acc:#5b9bf0;--warn:#d9a445;--stop:#f0736b;--ok:#3fbf6c;--wash:rgba(255,255,255,.05);
 --read:#4fc0c4;--pure:#9096ee;--mind:#dd88b2}}
body{margin:0;background:var(--pg);color:var(--ink);font:15px/1.55 system-ui,-apple-system,sans-serif}
.wrap{max-width:980px;margin:0 auto;padding:26px 18px 80px}
h1{font-size:22px;margin:0 0 4px;letter-spacing:-.01em}
.scope{color:var(--ink2);font-size:14px;max-width:68ch;margin:0 0 8px}
.meta{color:var(--mut);font-size:12.5px;margin:0}
.ns{margin-top:10px;display:flex;flex-wrap:wrap;gap:6px}
.ns a{font-size:12px;text-decoration:none;color:var(--ink2);background:var(--sf);
 border:1px solid var(--ln);border-radius:20px;padding:3px 10px}
.ns a:hover{color:var(--acc);border-color:var(--acc)}
.tabs{display:flex;gap:4px;margin:0 0 4px;border-bottom:1px solid var(--ln)}
.tabs button{font:inherit;font-size:13px;background:none;border:none;border-bottom:2px solid transparent;
 color:var(--mut);padding:7px 13px;cursor:pointer;margin-bottom:-1px}
.tabs button:hover{color:var(--ink2)}
.tabs button[aria-selected=true]{color:var(--ink);border-bottom-color:var(--acc);font-weight:600}
.tabs .n{font-size:11px;color:var(--mut);font-weight:400}
.lede{margin:22px 0 3px;font-size:13px;color:var(--mut)}
.purpose{margin:18px 0 4px;font-size:15px;color:var(--mut);max-width:none}
.purpose b{color:var(--ink);font-weight:600;font-size:16.5px}
.sub{margin:0 0 6px;font-size:12.5px;color:var(--mut)}
.banner{margin:12px 0 0;background:var(--sf);border:1px solid var(--warn);border-left-width:3px;
 border-radius:9px;padding:9px 12px;font-size:12.5px;color:var(--warn)}
h2{font-size:11.5px;letter-spacing:.09em;text-transform:uppercase;color:var(--mut);
 font-weight:600;margin:34px 0 10px}
h2 .n{text-transform:none;letter-spacing:0;font-weight:400}
h2.spill{color:var(--warn)}
.why{color:var(--mut);font-size:12.5px;margin:-4px 0 10px;max-width:70ch}
.card{background:var(--sf);border:1px solid var(--ln);border-radius:12px;padding:13px 15px;margin-bottom:9px}
.card .id{font:12px ui-monospace,Menlo,monospace;color:var(--acc)}
.card .vd{font-weight:600;margin:3px 0 6px}
.card .bc{color:var(--ink2);font-size:13.5px}
.deps{display:flex;flex-wrap:wrap;align-items:flex-start;gap:5px;margin-top:9px}
.dep{font:11px ui-monospace,Menlo,monospace;background:var(--wash);border:1px solid var(--ln);
 border-radius:16px;padding:2px 8px;cursor:pointer;color:var(--ink2);white-space:nowrap;
 direction:ltr;unicode-bidi:isolate}
.dep:hover{border-color:var(--acc);color:var(--acc)}
.dep.dead{color:var(--stop);border-color:var(--stop);cursor:not-allowed;text-decoration:line-through}
.dep.dead:hover{color:var(--stop);border-color:var(--stop)}
.dep.wait{color:var(--warn);border-color:var(--warn);border-style:dashed;cursor:not-allowed}
.dep.wait:hover{color:var(--warn);border-color:var(--warn)}
.tag{font-size:10.5px;border:1px solid var(--ln);border-radius:20px;padding:2px 8px;color:var(--mut)}
.tag.stop{color:var(--stop);border-color:var(--stop)}
.tag.warn{color:var(--warn);border-color:var(--warn)}
.tag.ok{color:var(--ok);border-color:var(--ok)}
table{width:100%;border-collapse:collapse;font-size:13.5px;background:var(--sf);
 border:1px solid var(--ln);border-radius:12px;overflow:hidden}
td{padding:7px 12px;border-top:1px solid var(--ln);vertical-align:top}
tr:first-child td{border-top:none}
td.k{font:12px ui-monospace,Menlo,monospace;color:var(--acc);width:1%;white-space:nowrap}
td.v{font-variant-numeric:tabular-nums}
.fx{border-bottom:1px dotted var(--mut);cursor:help}
.fx:hover,.fx.on{background:var(--wash);border-bottom-color:var(--acc)}
.pop{position:absolute;z-index:90;max-width:420px;background:var(--sf);border:1px solid var(--ln);
 border-radius:11px;padding:11px 13px;box-shadow:0 8px 30px rgba(0,0,0,.22);font-size:12.5px;
 line-height:1.5;color:var(--ink2)}
.pop b{color:var(--ink)}
.pop .hd{display:flex;gap:7px;align-items:center;margin-bottom:6px}
.pop .nm{color:var(--ink);font-weight:600;margin-bottom:5px}
.pop .r{display:flex;gap:9px;padding:4px 0;border-top:1px solid var(--ln)}
.pop .r span:first-child{flex:0 0 68px;color:var(--mut)}
.pop .r span:last-child{flex:1;min-width:0;overflow-wrap:anywhere}
.pop .deps{margin-top:0}
.pop .back{background:none;border:1px solid var(--ln);border-radius:5px;color:var(--mut);
 font:inherit;font-size:11px;padding:1px 6px;cursor:pointer}
.pop .back:hover{color:var(--acc);border-color:var(--acc)}
.tl{background:var(--sf);border:1px solid var(--ln);border-radius:12px;overflow:hidden}
.tlr{display:flex;gap:14px;align-items:baseline;padding:9px 15px;border-top:1px solid var(--ln)}
.tlr:first-child{border-top:none}
.tlr.past{opacity:.45}
.tlr .when{flex:0 0 104px;font-size:12.5px;color:var(--mut);font-variant-numeric:tabular-nums;
 direction:ltr;unicode-bidi:isolate}
.tlr .what{flex:1;min-width:0;font-size:13.5px}
.tlr.mark{background:var(--wash);color:var(--acc);font-size:11px;letter-spacing:.09em;
 text-transform:uppercase;padding:5px 15px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:13px}
.front{background:var(--sf);border:1px solid var(--ln);border-radius:12px;padding:12px 14px}
.front h3{margin:0 0 7px;font:600 12px ui-monospace,Menlo,monospace;color:var(--acc)}
.kv{display:flex;gap:10px;font-size:13px;padding:5px 0;border-top:1px solid var(--ln)}
.front .kv:first-of-type{border-top:none}
.kv .kl{flex:1;min-width:0;color:var(--ink2)}
.derived{color:var(--mut);font-style:normal;font-size:12.5px}
td.kl{width:1%;white-space:nowrap;color:var(--ink2)}
.kv .kvv{flex:0 0 auto;font-variant-numeric:tabular-nums;direction:ltr;unicode-bidi:isolate}
.kv.stack{display:block}
.kv.stack .kl{display:block;margin-bottom:2px}
.kv .rule{display:block;font:11px ui-monospace,Menlo,monospace;color:var(--mut);
 direction:ltr;unicode-bidi:isolate;overflow-wrap:anywhere;text-align:left}
[dir=rtl] .tlr .when,[dir=rtl] .kv .kvv{text-align:right}
.heads{display:flex;flex-wrap:wrap;gap:13px}
.head{flex:1 1 190px;background:var(--sf);border:1px solid var(--ln);border-radius:12px;padding:14px 16px}
.head .big{font-size:26px;font-weight:600;font-variant-numeric:tabular-nums;letter-spacing:-.02em;
 direction:ltr;unicode-bidi:isolate;display:inline-block}
.head .cap{font-size:12.5px;color:var(--mut);margin-top:3px}
.alerts{background:var(--sf);border:1px solid var(--ln);border-radius:12px;overflow:hidden}
.al{display:flex;gap:11px;align-items:flex-start;padding:10px 15px;border-top:1px solid var(--ln);font-size:13.5px}
.alerts .al:first-child{border-top:none}
.al .dot{flex:0 0 auto;width:8px;height:8px;border-radius:50%;margin-top:6px}
.al .at{flex:1;min-width:0}
.al .aw{font-size:12px;color:var(--mut);margin-top:2px}
.grp{display:inline-block;font-size:11px;color:var(--mut);background:var(--wash);
 border-radius:4px;padding:1px 6px;margin-inline-end:7px;vertical-align:1px}
.nt{font-size:12px;color:var(--mut);margin-top:2px;max-width:62ch}
.rest{font-size:11.5px;color:var(--mut);margin-top:9px}
.bad{color:var(--stop);font-size:12.5px;margin:-4px 0 9px}
.fx.in{border-bottom:1px dotted var(--acc)}
.rel-down{outline:1.5px solid var(--acc);outline-offset:3px;border-radius:3px}
.rel-up{outline:1.5px dashed var(--mut);outline-offset:3px;border-radius:3px}
.kref{font:11.5px ui-monospace,Menlo,monospace;color:var(--acc);border-bottom:1px dashed var(--acc);cursor:pointer}
.treewrap{background:var(--sf);border:1px solid var(--ln);border-radius:12px;padding:10px 6px 2px;overflow-x:auto}
.treewrap svg{display:block;margin:0 auto;min-width:640px}
.tn{cursor:pointer;touch-action:none;-webkit-user-select:none;user-select:none}
.tn.pulling{cursor:grabbing}
.tn text{font:10.5px system-ui,-apple-system,sans-serif;fill:var(--ink2);pointer-events:none}
.tn.root text{fill:var(--mut);font-size:9.5px}
.tn.root circle{fill:var(--read);stroke:none}
.tn.bough circle{fill:var(--pure);stroke:none}
.tn.crown circle{fill:var(--mind);stroke:none}
.tn.crown.stopf circle,.tn.crown.warnf circle{stroke-width:2.5}
.tn.warnf circle{stroke:var(--warn);stroke-dasharray:3 2}
.tn.stopf circle{stroke:var(--stop)}
#panel-tree .hid{display:none}
.tfoc{background:none;border:1px solid var(--ln);border-radius:5px;color:var(--mut);
 font:inherit;font-size:11px;padding:1px 6px;cursor:pointer;margin-inline-start:auto}
.tfoc:hover{color:var(--acc);border-color:var(--acc)}
.focchip{display:block;margin:10px auto 2px;font:12px system-ui,-apple-system,sans-serif;
 background:var(--sf);border:1px solid var(--acc);color:var(--acc);border-radius:20px;
 padding:4px 14px;cursor:pointer}
.focchip:hover{background:var(--wash)}
.tn.crown .halo{fill:var(--mind);opacity:.16;stroke:none}
.ttrunk{fill:var(--mut);opacity:.3}
.tlimb{fill:none;stroke:var(--mut);opacity:.5;stroke-linecap:round}
.troot{fill:none;stroke:var(--read);opacity:.65;stroke-linecap:round}
.tground{stroke:var(--ln);stroke-width:1.5;fill:none}
.tsoil{fill:var(--wash)}
svg .rel-down circle{stroke:var(--acc)!important;stroke-width:3px!important}
svg .rel-up circle{stroke:var(--mut)!important;stroke-dasharray:4 3;stroke-width:2.5px!important}
svg g.rel-chain circle{stroke:var(--read);stroke-width:2.5}
.tlimb.sapu{stroke:var(--read);opacity:.95}
.tlimb.sapd{stroke:var(--acc);opacity:.9}
.troot.sapu{opacity:1;stroke-width:3.2px!important}
@keyframes sway{0%{transform:translate(0,0)}30%{transform:translate(1px,-1.6px) rotate(.4deg)}
 65%{transform:translate(-.8px,1px)}100%{transform:translate(0,0)}}
svg g.sway{animation:sway .7s ease-out 1;transform-box:fill-box;transform-origin:center}
@media(prefers-reduced-motion:reduce){svg g.sway{animation:none}}
footer{margin-top:40px;padding-top:14px;border-top:1px solid var(--ln);color:var(--mut);font-size:12.5px}
"""

JS = r"""
(function(){
 var E=window.__E||{},J=window.__J||{},pop=null,cur=null,hist=[],now=null,tmr=null,pin=false,
     drag=null,grown=false,
     SLOW=!!(window.matchMedia&&matchMedia('(prefers-reduced-motion:reduce)').matches);
 function esc(s){var d=document.createElement('div');d.textContent=s==null?'':String(s);return d.innerHTML}
 function close(){clearTimeout(tmr);unrelate();if(pop){pop.remove();pop=null}
  if(cur){cur.classList.remove('on');cur=null}hist=[];now=null;pin=false}
 var IDRE=/[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+/g;
 function link(s){return esc(s).replace(IDRE,function(m){
  return (E[m]||J[m])?'<span class="kref" data-go="'+m+'">'+m+'</span>':m})}
 function row(l,v){return '<div class="r"><span>'+esc(l)+'</span><span>'+v+'</span></div>'}
 function body(id){
  var h='<div class="hd">'+(hist.length?'<button class="back" type="button">&larr;</button>':'')+
        '<span class="kref">'+esc(id)+'</span>'+
        (document.getElementById('panel-tree')
          ?'<button class="tfoc" type="button" title="prune the tree to what this touches">tree</button>':'')+
        '</div>';
  if(J[id]){var j=J[id];
   return h+(j.verdict?row('concludes','<b>'+esc(j.verdict)+'</b>'):'')+
    row('rests on','<span class="deps">'+(j.deps||[]).map(function(d){
      return '<span class="dep" data-go="'+esc(d)+'">'+esc(d)+'</span>'}).join('')+'</span>')+
    (j.pred?row('wrong if',link(j.pred)):'')+
    (j.blocked?row('blocked',esc(j.blocked)):'')+
    (j.because?row('because',esc(j.because)):'')}
  var e=E[id]||{};
  return h+(e.name?'<div class="nm">'+esc(e.name)+'</div>':'')+
   (e.v!=null?row('value','<b>'+esc(e.v)+'</b>'):'')+
   (e.rule?row('rule',link(e.rule)):'')+
   (e.from?row('from',esc(e.from)):'')+(e.at?row('at',esc(e.at)):'')+
   (e.file?row('file',esc(e.file)):'')+(e.url?row('url',esc(e.url)):'')+
   (e.of||e.read?row('as of',esc(e.of||e.read)):'')+
   (e.used&&e.used.length?row('used by','<span class="deps">'+e.used.map(function(d){
     return '<span class="dep" data-go="'+esc(d)+'">'+esc(d)+'</span>'}).join('')+'</span>'):'')}
 function place(el){if(!el||!pop)return;var r=el.getBoundingClientRect();
  pop.style.left=Math.min(Math.max(8,r.left+scrollX),scrollX+innerWidth-pop.offsetWidth-8)+'px';
  pop.style.top=(r.bottom+pop.offsetHeight+12>innerHeight?r.top+scrollY-pop.offsetHeight-6:r.bottom+scrollY+6)+'px'}
 function walk(id,next){var out={},q=[id];
  while(q.length){var k=q.pop(),ns=next(k)||[];
   for(var i=0;i<ns.length;i++)if(!out[ns[i]]){out[ns[i]]=1;q.push(ns[i])}}
  return out}
 function relate(id){
  // direct neighbours get the outline and a breath of wind; the full chains light
  // the sap - up toward the roots that feed this, down through what it feeds.
  var d={},u={},x=E[id]||J[id]||{};
  ((x.used)||[]).forEach(function(k){d[k]=1});
  (((J[id]||{}).deps)||((E[id]||{}).par)||[]).forEach(function(k){u[k]=1});
  var TA=walk(id,function(k){return (J[k]||{}).deps||(E[k]||{}).par}),
      TD=walk(id,function(k){return (E[k]||J[k]||{}).used});
  var all=document.querySelectorAll('[data-id]');
  for(var i=0;i<all.length;i++){var k=all[i].getAttribute('data-id');
   all[i].classList.toggle('rel-down',!!d[k]);all[i].classList.toggle('rel-up',!!u[k]);
   if(all[i].classList.contains('tn')){
    all[i].classList.toggle('rel-chain',(!!TA[k]||!!TD[k])&&!d[k]&&!u[k]);
    all[i].classList.toggle('sway',!!d[k]||!!u[k])}}
  var ls=document.querySelectorAll('path[data-lt]');
  for(var i=0;i<ls.length;i++){var f=ls[i].getAttribute('data-lf'),t=ls[i].getAttribute('data-lt');
   ls[i].classList.toggle('sapu',t===id||!!TA[t]);
   ls[i].classList.toggle('sapd',f===id||!!TD[f])}}
 function unrelate(){var q=document.querySelectorAll('.rel-down,.rel-up,.rel-chain,.sway,.sapu,.sapd');
  for(var i=0;i<q.length;i++)q[i].classList.remove('rel-down','rel-up','rel-chain','sway','sapu','sapd')}

 // Focus: the tree pruned to one node's world - everything that feeds it and
 // everything it feeds. The rest of the record is one chip away, not gone.
 var F=null,showTab=null;
 function focus(id){
  F=id;
  var TA=walk(id,function(k){return (J[k]||{}).deps||(E[k]||{}).par}),
      TD=walk(id,function(k){return (E[k]||J[k]||{}).used});
  var keep={},k;keep[id]=1;
  for(k in TA)keep[k]=1;
  for(k in TD)keep[k]=1;
  var tn=document.querySelectorAll('#panel-tree .tn');
  for(var i=0;i<tn.length;i++)
   tn[i].classList.toggle('hid',!keep[tn[i].getAttribute('data-id')]);
  var ls=document.querySelectorAll('#panel-tree path[data-lt]');
  for(var i=0;i<ls.length;i++){var f=ls[i].getAttribute('data-lf'),t=ls[i].getAttribute('data-lt');
   ls[i].classList.toggle('hid',!(keep[t]&&(!f||keep[f])))}
  var chip=document.getElementById('focchip');
  if(!chip){chip=document.createElement('button');chip.id='focchip';chip.className='focchip';
   chip.type='button';chip.addEventListener('click',unfocus);
   var tw=document.querySelector('#panel-tree .treewrap');
   if(tw)tw.parentNode.insertBefore(chip,tw)}
  chip.textContent='⟵ the whole tree';
  relate(id)}
 function unfocus(){F=null;
  var q=document.querySelectorAll('#panel-tree .hid');
  for(var i=0;i<q.length;i++)q[i].classList.remove('hid');
  var chip=document.getElementById('focchip');
  if(chip)chip.remove();
  unrelate()}
 // Pulling a node. It answers the way a branch does: it follows part of the way,
 // and past about an eighth of the tree's width it plainly refuses - the applied
 // displacement is R*tanh(d/R), so the hand keeps going and the node does not. Its
 // limbs stretch with it and the neighbours it crowds are elbowed aside. None of it
 // is kept: on release everything springs back to where the record put it, and the
 // record never learns that anyone touched it.
 function at(el){var c=el.querySelector('circle');
  return c?[+c.getAttribute('cx'),+c.getAttribute('cy')]:null}
 function arc(d){var n=(d.match(/-?[\d.]+/g)||[]).map(Number);return n.length===8?n:null}
 function curve(n){return 'M'+n[0]+' '+n[1]+' C'+n[2]+' '+n[3]+' '+n[4]+' '+n[5]+
  ' '+n[6]+' '+n[7]}
 function limbs(tp,el,id){
  // what ends on this node, and what leaves it - a limb fed by a root leaves the
  // trunk rather than the root itself, so it is told apart by where it starts.
  var out=[],c=at(el),ps=tp.querySelectorAll('path[data-lt]');
  for(var i=0;i<ps.length;i++){var n=arc(ps[i].getAttribute('d'));if(!n)continue;
   var tail=ps[i].getAttribute('data-lt')===id,
       head=ps[i].getAttribute('data-lf')===id&&!!c&&
            Math.abs(n[0]-c[0])<4&&Math.abs(n[1]-c[1])<4;
   if(tail||head)out.push({p:ps[i],n:n,tail:tail,head:head})}
  return out}
 function put(m,ax,ay){
  m.ax=ax;m.ay=ay;
  m.el.style.transform=(ax||ay)?'translate('+ax.toFixed(1)+'px,'+ay.toFixed(1)+'px)':'';
  for(var i=0;i<m.ed.length;i++){var e=m.ed[i],n=e.n.slice();
   if(e.tail){n[4]+=ax*.55;n[5]+=ay*.55;n[6]+=ax;n[7]+=ay}
   if(e.head){n[0]+=ax;n[1]+=ay;n[2]+=ax*.55;n[3]+=ay*.55}
   e.p.setAttribute('d',curve(n.map(function(v){return Math.round(v*10)/10})))}}
 function elbow(q,px,py,rad){        // how far a crowded neighbour gives way
  var vx=q[0]-px,vy=q[1]-py,d=Math.sqrt(vx*vx+vy*vy);
  if(d>=rad||d<.01)return [0,0];
  var f=(rad-d)/rad*22;
  return [vx/d*f,vy/d*f]}
 var settling=[];
 function spring(m){
  // home, a hair past it, and still. One damped overshoot, and then the node is
  // exactly where it was - there is nothing here to save.
  var ax=m.ax||0,ay=m.ay||0,t0=null;
  if(SLOW||(!ax&&!ay)){put(m,0,0);return}
  m.dead=0;settling.push(m);
  requestAnimationFrame(function step(t){
   if(m.dead)return;
   if(t0===null)t0=t;
   var u=(t-t0)/420;
   if(u>=1){put(m,0,0);landed(m);return}
   put(m,ax*Math.exp(-5.2*u)*Math.cos(u*Math.PI*1.55),
       ay*Math.exp(-5.2*u)*Math.cos(u*Math.PI*1.55));
   requestAnimationFrame(step)})}
 function landed(m){m.dead=1;var i=settling.indexOf(m);if(i>=0)settling.splice(i,1)}
 function stillness(){
  // Land every spring at once. A second pull that begins mid-return would otherwise
  // take the displaced limbs for the true ones and restore the tree to a lie.
  while(settling.length){var m=settling.pop();m.dead=1;put(m,0,0)}}
 var tp=document.getElementById('panel-tree'),eat=false;
 if(tp)tp.addEventListener('pointerdown',function(e){
  var el=e.target.closest?e.target.closest('.tn[data-id]'):null;
  if(!el||e.button||drag)return;
  var svg=tp.querySelector('svg'),home=at(el);
  if(!svg||!home)return;
  clearTimeout(tmr);if(pop&&!pin)close();
  eat=false;stillness();
  if(svg.getAnimations)svg.getAnimations({subtree:true}).forEach(function(a){a.finish()});
  var r=svg.getBoundingClientRect(),vb=svg.viewBox.baseVal,
      R=vb.width/8,rad=R*.62,near=[],all=tp.querySelectorAll('.tn[data-id]');
  for(var i=0;i<all.length;i++){
   if(all[i]===el)continue;
   var q=at(all[i]);if(!q)continue;
   if(Math.sqrt((q[0]-home[0])*(q[0]-home[0])+(q[1]-home[1])*(q[1]-home[1]))<rad)
    near.push({el:all[i],ed:limbs(tp,all[i],all[i].getAttribute('data-id')),at:q})}
  drag={x:e.clientX,y:e.clientY,k:r.width?vb.width/r.width:1,R:R,rad:rad,far:0,home:home,
        me:{el:el,ed:limbs(tp,el,el.getAttribute('data-id'))},near:near};
  // capture the pointer, so a hand that lets go past the edge of the window still
  // ends the pull - a node left hanging there would be a kept position by accident.
  try{el.setPointerCapture(e.pointerId)}catch(err){}
  el.classList.remove('sway');el.classList.add('pulling')});
 document.addEventListener('pointermove',function(e){
  if(!drag)return;
  var dx=(e.clientX-drag.x)*drag.k,dy=(e.clientY-drag.y)*drag.k,
      d=Math.sqrt(dx*dx+dy*dy),s=d?drag.R*Math.tanh(d/drag.R)/d:0,ax=dx*s,ay=dy*s;
  if(d>drag.far)drag.far=d;
  put(drag.me,ax,ay);
  for(var i=0;i<drag.near.length;i++){var n=drag.near[i],
      a=elbow(n.at,drag.home[0]+ax,drag.home[1]+ay,drag.rad),
      b=elbow(n.at,drag.home[0],drag.home[1],drag.rad);
   put(n,a[0]-b[0],a[1]-b[1])}});
 function drop(released){if(!drag)return;
  var g=drag;drag=null;
  g.me.el.classList.remove('pulling');
  // A pull is not a click. Only a normal release is followed by one, though, so a
  // cancelled gesture must not leave a swallow armed for someone's next click.
  eat=released&&g.far>3;
  spring(g.me);
  for(var i=0;i<g.near.length;i++)spring(g.near[i])}
 document.addEventListener('pointerup',function(){drop(true)});
 document.addEventListener('pointercancel',function(){drop(false)});
 document.addEventListener('click',function(e){
  if(eat){eat=false;e.stopPropagation();e.preventDefault()}},true);

 // The first time the tree is shown it grows into place: the roots reach out, the
 // trunk rises, the limbs run along it and the blossoms open last. Once per page
 // load - it answers "what is the shape of the whole" and then gets out of the way.
 // A reader who has asked for less motion gets the finished tree and no wait.
 function grow(){
  var svg=document.querySelector('#panel-tree svg');
  if(!svg||grown)return;
  grown=true;
  if(SLOW||!svg.animate)return;
  function run(p,kf,opt,undo){
   var a=p.animate(kf,opt);a.onfinish=a.oncancel=undo;return a}
  function draw(sel,delay,dur){        // a limb runs out along itself
   var q=svg.querySelectorAll(sel);
   for(var i=0;i<q.length;i++)(function(p){
    var L=0;try{L=p.getTotalLength()}catch(err){}
    if(!L)return;
    p.style.strokeDasharray=L;p.style.strokeDashoffset=L;
    run(p,[{strokeDashoffset:L},{strokeDashoffset:0}],
        {duration:dur,delay:delay,easing:'ease-out',fill:'backwards'},
        function(){p.style.strokeDasharray='';p.style.strokeDashoffset=''})})(q[i])}
  function bloom(sel,delay,dur,from,org,ease){
   var q=svg.querySelectorAll(sel);
   for(var i=0;i<q.length;i++)(function(p){
    p.style.transformBox='fill-box';p.style.transformOrigin=org;
    run(p,[{transform:from,opacity:0},{transform:'none',opacity:1}],
        {duration:dur,delay:delay,easing:ease||'cubic-bezier(.2,.8,.3,1.25)',
         fill:'backwards'},
        function(){p.style.transformBox='';p.style.transformOrigin=''})})(q[i])}
  draw('.troot',0,420);
  bloom('.tn.root',120,300,'scale(.2)','center');
  bloom('.ttrunk',300,300,'scaleY(0)','50% 100%','ease-out');
  draw('.tlimb',520,400);
  bloom('.tn.bough',620,300,'scale(.2)','center');
  bloom('.tn.crown',860,340,'scale(.2)','center')}

 function paint(id){now=id;pop.innerHTML=body(id);relate(id)}
 function open(el,id,p){if(pop)close();if(!E[id]&&!J[id])return;
  cur=el;hist=[];if(p){el.classList.add('on');pin=true}
  pop=document.createElement('div');pop.className='pop';document.body.appendChild(pop);
  paint(id);
  pop.addEventListener('mouseenter',function(){clearTimeout(tmr)});
  pop.addEventListener('mouseleave',function(){if(!pin){clearTimeout(tmr);tmr=setTimeout(close,220)}});
  pop.addEventListener('click',function(ev){ev.stopPropagation();
   if(ev.target.closest('.tfoc')){var t=now;if(showTab)showTab('tree',true);focus(t);return}
   if(ev.target.closest('.back')){if(hist.length){paint(hist.pop());place(cur)}return}
   var g=ev.target.closest('[data-go]');if(!g)return;
   var t=g.getAttribute('data-go');if(t===now||(!E[t]&&!J[t]))return;
   hist.push(now);if(!pin&&cur){cur.classList.add('on');pin=true}paint(t);place(cur)});
  place(el)}
 function hit(e){return e.target.closest?e.target.closest('[data-id]'):null}
 document.addEventListener('mouseover',function(e){var el=hit(e);
  if(!el||pin||drag||el===cur)return;
  clearTimeout(tmr);tmr=setTimeout(function(){open(el,el.getAttribute('data-id'),false)},130)});
 document.addEventListener('mouseout',function(e){if(!hit(e)||pin||drag)return;
  clearTimeout(tmr);tmr=setTimeout(function(){if(!pin)close()},260)});
 document.addEventListener('click',function(e){if(e.target.closest('.pop'))return;
  var el=hit(e);if(el){e.stopPropagation();
   if(pin&&cur===el&&!hist.length)close();else{close();open(el,el.getAttribute('data-id'),true)}}
  else close()});
 document.addEventListener('keydown',function(e){if(e.key!=='Escape')return;
  if(hist.length){paint(hist.pop());place(cur);return}
  if(pop){close();return}
  if(F)unfocus()});
 addEventListener('resize',close);
 addEventListener('scroll',function(){if(!pin)close();else place(cur)},{passive:true});

 // Tabs. One provenance layer above, both panels below it - the layer binds on
 // document and keys off [data-id], so it does not know a tab exists.
 var bar=document.querySelector('.tabs');
 if(bar){
  var btns=[].slice.call(bar.querySelectorAll('button'));
  function show(name,push){
   btns.forEach(function(b){
    var on=b.getAttribute('data-tab')===name;
    b.setAttribute('aria-selected',on?'true':'false');
    document.getElementById('panel-'+b.getAttribute('data-tab')).hidden=!on});
   close();
   if(name==='tree')grow();
   if(push&&location.hash!=='#'+name){try{history.replaceState(null,'','#'+name)}catch(e){}}}
  showTab=show;                          // a card's tree button has to be able to get there
  btns.forEach(function(b){b.addEventListener('click',function(){show(b.getAttribute('data-tab'),true)})});
  function fromHash(){var w=(location.hash||'').replace('#','');
   if(w&&document.getElementById('panel-'+w))show(w,false)}
  addEventListener('hashchange',fromHash);
  fromHash();
 }
})();
"""

# ── what the record says about itself ────────────────────────────────────────
# One reading of state, used by every section selector. The same four conditions
# `provenance.py open` ranks by; named here so a brief can select on them.
STATES = ("broken", "falsified", "unchecked", "moved", "blocked", "no_predicate")


def flags_of(ids, jud, fields, raw):
    """Per judgment: the set of conditions that put it in front of a person - derived the
    way `check` and `open` derive them, so the page never disagrees with the reader."""
    out = {}
    for name, j in jud.items():
        f, blocked = set(), next((str(j["body"][k]) for k in P.BLOCKED if j["body"].get(k)), "")
        for d in j["deps"]:
            if d not in ids:
                f.add("blocked" if blocked else "broken")
            elif fields["snapshot"] and d not in j["seen"]:
                f.add("unchecked")
        if not [t for t in P.ID.findall(j["pred"]) if t in ids] and not blocked:
            f.add("no_predicate")
        elif P.evaluate(j["pred"], raw, ids) is True:
            f.add("falsified")
        if any(s == "moved" for _, _, _, s in P.moved_deps(j, raw, ids)):
            f.add("moved")
        out[name] = f
    return out


RTL = re.compile(r"[\u0590-\u05ff\u0600-\u06ff]")


def direction(doc):
    """A record written in Hebrew should not be read left to right because the tool
    was written in English. This is a decision about the record's *shape*, so it is
    stable: values changing never flips the page."""
    txt = "".join(str(v) for g in (doc or {}).values() if isinstance(g, dict)
                  for b in g.values() for v in (b.values() if isinstance(b, dict) else [b])
                  if isinstance(v, str))
    letters = [c for c in txt if c.isalpha()]
    return "rtl" if letters and len(RTL.findall(txt)) / len(letters) > 0.3 else "ltr"


def fmt(v):
    """Thousands separators on a hero number. Nothing else is touched."""
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return str(v)
    return f"{v:,}" if isinstance(v, int) else f"{v:,.10g}".rstrip()


def shape_of(ids, jud, flags):
    held = [k for k in ids if k not in jud and not P.is_builtin(k)]
    return {"entries": len(held), "judgments": len(jud),
            "flagged": sum(1 for f in flags.values() if f),
            "blocked": sum(1 for f in flags.values() if "blocked" in f)}


# ── the brief ────────────────────────────────────────────────────────────────
def find_brief(paths, explicit=None):
    if explicit:
        return explicit
    for p in list(paths) + ["PROVENANCE.yaml"]:
        c = re.sub(r"\.ya?ml$", "", p) + ".view.yaml"
        if os.path.exists(c):
            return c
    return None


def effective(brief):
    """The brief as the page reads it today. A brief written as `tabs:` draws its first tab;
    the others are declared and counted, and `--verify` says how many wait. What a tab
    carries that the page does not yet draw - `occasion`, `serves`, a section's `text` with
    its `reviewed`/`seen` - is kept and checked all the same."""
    if not isinstance(brief, dict):
        return {}
    if not brief.get("tabs") or brief.get("sections"):
        return brief
    tabs = [t for t in brief["tabs"] if isinstance(t, dict)]
    if not tabs:
        return brief
    out, first = dict(brief), tabs[0]
    out["sections"] = list(first.get("sections") or [])
    if not out.get("intent") and (first.get("occasion") or first.get("title")):
        out["intent"] = first.get("occasion") or first.get("title")
    if out.get("shape") is None and first.get("shape") is not None:
        out["shape"] = first["shape"]
    out["_first_tab_drawn"] = True
    if first.get("title"):
        out["_tab_title"] = str(first["title"])
    return out


def resolve(sel, ids, jud, flags):
    """A selector is a state, a group, a prefix, or an exact id. Evaluated now, not frozen -
    which is the whole reason a section keeps up with the record without being edited."""
    sel = str(sel).strip()
    if sel in STATES:
        return {k for k, f in flags.items() if sel in f}
    if sel == "flagged":
        return {k for k, f in flags.items() if f}
    if sel == "judgments":
        return set(jud)
    if sel == "all":
        return set(ids)
    if sel in ids:
        return {sel}
    pre = sel.rstrip(".")
    hits = {k for k in ids if k.split(".")[0] == pre or k.startswith(pre + ".")}
    return hits


# ── anchoring: the reference belongs in the sentence, not in a chip beside it ──
# A row of monospace ids under a card is the graph leaking onto the reading surface.
# Bind what the prose already says - the id if it is written out, the value if it is
# quoted - and the sentence becomes the interface. Candidates are limited to *this*
# judgment's own dependencies: anchoring against the whole record invents links, and a
# false link is worse than a missing one.
NUMISH = re.compile(r"^-?\d+(\.\d+)?$")


def as_date(v):
    if isinstance(v, datetime.datetime):
        return v.date()
    if isinstance(v, datetime.date):
        return v
    s = str(v).strip()
    for f in ("%Y-%m-%d", "%d/%m/%Y", "%d.%m.%Y"):
        try:
            return datetime.datetime.strptime(s, f).date()
        except ValueError:
            pass
    return None


def renderings(v):
    """The strings a value can plausibly appear as in prose. Rounded restatements are
    deliberately not among them - binding '~180,000' to 179,842 would be a lie."""
    d = as_date(v)
    if d:
        out = [d.strftime("%d/%m/%Y"), d.strftime("%d/%m"), d.isoformat(), d.strftime("%d.%m.%Y")]
    else:
        s = str(v).strip()
        if not s:
            return []
        bare = s.replace(",", "")
        if NUMISH.match(bare):
            out = [s, bare] + ([f"{int(bare):,}"] if bare.lstrip("-").isdigit() else [])
        else:
            out = [s] if len(s) >= 6 else []
    return [x for x in dict.fromkeys(out) if len(x) >= 3]


def anchor(text, deps, E):
    """-> (html, set of deps that found a place in the text)."""
    if not text:
        return "", set()
    owner, seen_str = {}, set()
    for d in deps:
        for cand in [d] + renderings((E.get(d) or {}).get("v")):
            if cand in seen_str and owner.get(cand) != d:
                owner[cand] = None                      # two deps could explain it: neither does
            else:
                owner.setdefault(cand, d)
            seen_str.add(cand)
    hits, taken = [], []
    for cand in sorted((c for c, o in owner.items() if o), key=len, reverse=True):
        for m in re.finditer(re.escape(cand), text):
            a, b = m.span()
            if any(a < y and x < b for x, y in taken):
                continue
            taken.append((a, b))
            hits.append((a, b, owner[cand]))
    hits.sort()
    out, pos = [], 0
    for a, b, d in hits:
        out.append(html.escape(text[pos:a]))
        out.append(f'<span class="fx in" data-id="{html.escape(d)}">{html.escape(text[a:b])}</span>')
        pos = b
    out.append(html.escape(text[pos:]))
    return "".join(out), {d for _, _, d in hits}


def human(k):
    return k.split(".")[-1].replace("_", " ")


# ── renderers: a closed set, each declaring the shape of data it can carry ────
# Layout is where intent shows. But a renderer that silently accepts data it cannot
# express produces a page that looks arranged and is not, so each one says what it
# needs and a mismatch is a failure, not a shrug.
def fits(kind, keys, jud, E):
    if kind in ("table", "lines", "cards"):
        return None
    if kind == "timeline":
        bad = [k for k in keys if k not in jud and as_date((E.get(k) or {}).get("v")) is None]
        return (f"timeline needs date values; {len(bad)} of {len(keys)} are not dates "
                f"({', '.join(bad[:4])})") if bad else None
    if kind == "headline":
        n = [k for k in keys if k not in jud]
        if not 1 <= len(n) <= 4:
            return f"headline carries one to four values, not {len(n)}"
        blank = [k for k in n if (E.get(k) or {}).get("v") is None]
        return (f"headline needs values; {', '.join(blank)} "
                f"{'is derived and this reader does not evaluate rules' if len(blank) == 1 else 'are derived'}"
                ) if blank else None
    if kind == "fronts":
        g = {k.split(".")[0] for k in keys if k not in jud}
        return f"fronts lays groups side by side; these are all one group ({g})" if len(g) < 2 else None
    if kind == "alerts":
        e = [k for k in keys if k not in jud]
        return f"alerts ranks judgments; {len(e)} of these are entries" if e else None
    return f"unknown renderer '{kind}'"


URGENCY = {"broken": (100, "stop"), "falsified": (95, "stop"), "unchecked": (80, "stop"),
           "moved": (70, "warn"), "blocked": (60, "warn"), "no_predicate": (40, "mut")}

# What each state means, said the way a person would say it. The machine name stays -
# in the hover, where the keys and the rules live. Nothing on the reading surface is
# named after how the thing is built.
SAYS = {"broken": "rests on something that is not in this record",
        "falsified": "its own condition for being wrong now holds",
        "moved": "something it rests on no longer matches what it last saw",
        "unchecked": "has never been checked against one of the things it rests on",
        "blocked": "waiting on something nobody has recorded yet",
        "no_predicate": "nothing here would show it to be wrong"}

# A human name for an entry belongs to the entry, not to a session: what a thing is
# does not change because someone opened the page for a different reason. This is the
# one field the method asks for by name rather than inferring by shape - a sentence has
# no distinctive shape - and a record without it still works, less well.
NAMES = ("name", "title", "label", "what", "desc")


def named(body):
    if not isinstance(body, dict):
        return ""
    return next((str(body[f]).strip() for f in NAMES if body.get(f)), "")


def blocked_of(body):
    """-> (prose, [keys]). A mapping keeps them apart; a bare string is all prose."""
    for k in P.BLOCKED:
        v = body.get(k)
        if not v:
            continue
        if isinstance(v, dict):
            m = v.get("missing") or []
            m = [m] if isinstance(m, str) else list(m)
            why = " ".join(str(x) for f, x in v.items() if f != "missing" and isinstance(x, str))
            return why, m
        return str(v), []
    return "", []


def _plain(o):
    """YAML gives real dates and numbers; JSON wants strings for the ones it cannot carry."""
    if isinstance(o, (datetime.date, datetime.datetime)):
        return o.isoformat()
    if isinstance(o, dict):
        return {k: _plain(v) for k, v in o.items()}
    if isinstance(o, list):
        return [_plain(v) for v in o]
    return o


def tree_svg(ids, jud, E, J, flags):
    """The record as one growing thing. What was read from the world is the root
    system, below the ground line; what was worked out and concluded branches up
    from it, judgments in the canopy. Same record, same tree - the layout reads
    only the graph, so nothing here moves unless the record does."""
    def jig(k, m, salt=""):
        return int(hashlib.md5((salt + k).encode()).hexdigest(), 16) % m

    parents = {}
    for k in ids:
        if k in jud:
            parents[k] = [d for d in jud[k]["deps"] if d in ids]
        else:
            e = E.get(k) or {}
            ps = [t for t in P.ID.findall(str(e.get("rule") or "")) if t in ids]
            frm = e.get("from")
            if isinstance(frm, str) and frm in ids and frm != k:
                ps.append(frm)
            parents[k] = ps
    depth = {}

    def dep(k, seen=()):
        if k in depth:
            return depth[k]
        if k in seen:
            return 0
        ps = parents.get(k) or []
        depth[k] = 0 if not ps else 1 + max(dep(p, seen + (k,)) for p in ps)
        return depth[k]
    for k in ids:
        dep(k)

    # facts sit below conclusions whatever the raw path lengths: entries keep their
    # depth, judgments stack above the tallest entry, judgment-on-judgment higher.
    de = max([depth[k] for k in ids if k not in jud], default=0)
    jd = {}

    def jdep(k, seen=()):
        if k in jd:
            return jd[k]
        if k in seen:
            return 0
        ps = [p for p in parents.get(k) or [] if p in jud]
        jd[k] = 0 if not ps else 1 + max(jdep(p, seen + (k,)) for p in ps)
        return jd[k]
    for k in jud:
        jdep(k)
    for k in ids:
        if k in jud:
            depth[k] = de + 1 + jd[k]

    roots = sorted((k for k in ids if depth[k] == 0), key=lambda k: (k.split(".")[0], k))
    upper = sorted((k for k in ids if depth[k] > 0), key=lambda k: (depth[k], k))
    # past a certain size the grove is a thicket; keep the canopy and what feeds it.
    dropped = 0
    if len(roots) + len(upper) > 110:
        feed = set()
        for k in upper:
            feed |= set(parents[k])
        kept = [k for k in roots if k in feed]
        dropped = len(roots) - len(kept)
        roots = kept

    W, M = 940, 48
    maxd = max([depth[k] for k in upper], default=1)
    LH = 150 if maxd <= 2 else (112 if maxd == 3 else 92)
    G = 72 + maxd * LH + 26
    H = G + 118
    tx = W / 2 + jig("".join(sorted(ids))[:64], 30) - 15      # the trunk leans, per record
    x, y = {}, {}
    for i, k in enumerate(roots):
        x[k] = M + (i + 0.5) * (W - 2 * M) / max(1, len(roots)) + jig(k, 9) - 4
        y[k] = G + 30 + jig(k, 40, "d")
    for d in range(1, maxd + 1):
        layer = [k for k in upper if depth[k] == d]
        gap = 92 if any(k in jud for k in layer) else 48
        for k in layer:
            ps = [p for p in parents[k] if p in x]
            x[k] = (sum(x[p] for p in ps) / len(ps) if ps
                    else M + jig(k, W - 2 * M)) + jig(k, 21, "x") - 10
        layer.sort(key=lambda k: x[k])
        for i in range(1, len(layer)):          # min gap, one deterministic sweep
            x[layer[i]] = max(x[layer[i]], x[layer[i - 1]] + gap)
        off = max(0, (x[layer[-1]] - (W - M)) / 2) if layer else 0
        for k in layer:
            x[k] = min(W - M, max(M, x[k] - off))
            # the canopy is a dome: the further a node sits from the trunk, the
            # lower it hangs.
            y[k] = G - d * LH - jig(k, 16, "y") + ((x[k] - tx) ** 2) * 30 / (W / 2) ** 2

    def lbl(k, n):
        b = jud.get(k)
        t = (b["body"].get("verdict") or b["body"].get("title") or k.split(".")[-1]) if b \
            else (E.get(k, {}).get("name") or k.split(".")[-1])
        t = str(t)
        return t if len(t) <= n else t[:n] + "…"

    o = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="the record as a tree">']
    o.append(f'<rect class="tsoil" x="0" y="{G:.0f}" width="{W}" height="{H - G:.0f}"/>')
    o.append(f'<path class="tground" d="M0 {G:.0f} '
             + " ".join(f"Q {gx + 30} {G + (3 if (gx // 60) % 2 else -3):.0f} {gx + 60} {G:.0f}"
                        for gx in range(0, W, 60)) + '"/>')
    top = G - LH * 0.82
    o.append(f'<path class="ttrunk" d="M{tx - 22:.0f} {G:.0f} '
             f'C{tx - 17:.0f} {G - LH * .32:.0f} {tx - 7:.0f} {G - LH * .5:.0f} '
             f'{tx - 4:.0f} {top:.0f} L{tx + 4:.0f} {top:.0f} '
             f'C{tx + 7:.0f} {G - LH * .5:.0f} {tx + 17:.0f} {G - LH * .32:.0f} '
             f'{tx + 22:.0f} {G:.0f} Z"/>')
    for k in upper:
        for p in parents[k]:
            if p not in x:
                continue
            # a fact reaches its consumers through the trunk: the root curve already
            # carried it to the base, so its limb emerges from the wood - only an
            # above-ground parent branches from where it actually stands.
            if depth[p] == 0:
                px, py = tx + jig(p, 13) - 6, G - LH * 0.45
            else:
                px, py = x[p], y[p]
            cx, cy = x[k], y[k]
            w = 1.5 + min(2.6, 0.4 * len((E.get(p, {}) or {}).get("used", [])))
            m2x = cx * 0.55 + tx * 0.45
            o.append(f'<path class="tlimb" data-lf="{html.escape(p)}" data-lt="{html.escape(k)}" '
                     f'stroke-width="{w:.1f}" d="M{px:.0f} {py:.0f} '
                     f'C{px:.0f} {py - LH * .35:.0f} {m2x:.0f} {cy + LH * .5:.0f} '
                     f'{cx:.0f} {cy:.0f}"/>')
    for k in roots:                              # the root fan spreads from the trunk base
        s = -6 if x[k] < tx else 6
        o.append(f'<path class="troot" data-lt="{html.escape(k)}" '
                 f'stroke-width="2.2" d="M{tx + s:.0f} {G + 2:.0f} '
                 f'C{tx + s * 5:.0f} {G + 26:.0f} {(x[k] + tx) / 2:.0f} {y[k] - 4:.0f} '
                 f'{x[k]:.0f} {y[k]:.0f}"/>')
    show_root_lbl = len(roots) <= 16
    boughs = [k for k in upper if k not in jud]
    show_bough_lbl = len(boughs) <= 10
    ci = ri = 0
    for k in roots + upper:
        f = flags.get(k, set())
        sev = " stopf" if f - {"blocked", "moved"} else (" warnf" if f else "")
        kind = "crown" if k in jud else ("root" if depth[k] == 0 else "bough")
        r = 9 if kind == "crown" else (5 if kind == "root" else 4)
        o.append(f'<g class="tn {kind}{sev}" data-id="{html.escape(k)}">')
        if kind == "crown":
            o.append(f'<circle class="halo" cx="{x[k]:.0f}" cy="{y[k]:.0f}" r="18"/>')
        o.append(f'<circle cx="{x[k]:.0f}" cy="{y[k]:.0f}" r="{r}"/>')
        if kind == "crown":
            ty = y[k] - 22 - 13 * (ci % 2); ci += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 20))}</text>')
        elif kind == "root" and show_root_lbl:
            ty = y[k] + 16 + 11 * (ri % 3); ri += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 12))}</text>')
        elif kind == "bough" and show_bough_lbl:
            ty = y[k] + 15 + 12 * (ci % 2); ci += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 16))}</text>')
        o.append("</g>")
    if dropped:
        o.append(f'<text x="{M}" y="{H - 8:.0f}" class="tn root"><tspan>'
                 f'... and {dropped} more roots below the grass</tspan></text>')
    o.append("</svg>")
    return "".join(o)


def build(paths, brief_path=None):
    doc = P.load(paths)
    ids, jud, fields = P.infer(doc)
    meta = doc.get("meta") or {}
    built = P.builtins(doc, ids, jud, fields, P.bodies(doc))
    raw0 = P.bodies(doc)
    raw0.update(built)
    flags = flags_of(ids, jud, fields, raw0)
    shape = shape_of(ids, jud, flags)
    brief = {}
    if brief_path and os.path.exists(brief_path):
        brief = effective(yaml.safe_load(io.open(brief_path, encoding="utf-8").read()) or {})

    groups = {}
    for k in sorted(ids):
        if k in jud:
            continue
        groups.setdefault(k.split(".")[0] if "." in k else "-", []).append(k)

    # the payload: entries and judgments, keyed by id. one copy, shared by every element
    # on every tab.
    raw = {}
    for k, v in doc.items():
        if isinstance(v, dict):
            for nid, b in v.items():
                if isinstance(b, dict):
                    raw[nid] = b
                elif nid in ids:
                    raw[nid] = {"v": b}
    raw.update(built)
    used = {}
    for name, j in sorted(jud.items()):
        for d in j["deps"]:
            used.setdefault(d, []).append(name)
    E, J = {}, {}
    for k in sorted(ids):
        if k in jud:
            continue
        b = raw.get(k, {})
        E[k] = {kk: b.get(kk) for kk in ("v", "rule", "from", "at", "of", "read", "quoted", "url", "file")
                if b.get(kk) is not None}
        if b.get("quoted") and "v" not in E[k]:
            E[k]["v"] = b["quoted"]
        v = E[k].get("v")
        if isinstance(v, str) and P.EXPR.search(v) and [t for t in P.ID.findall(v) if t in ids]:
            E[k].setdefault("rule", v)
            del E[k]["v"]
        E[k]["used"] = sorted(used.get(k, []))
        ps = [t for t in P.ID.findall(str(E[k].get("rule") or "")) if t in ids and t != k]
        frm = b.get("from")
        if isinstance(frm, str) and frm in ids and frm != k:
            ps.append(frm)
        if ps:
            E[k]["par"] = sorted(set(ps))
        if named(b):
            E[k]["name"] = named(b)
        n = next((str(b[f]) for f in ("via", "note", "why") if b.get(f)), "")
        if 0 < len(n) <= 150:
            E[k]["note"] = n
    for name, j in sorted(jud.items()):
        b = j["body"]
        why, keys = blocked_of(b)
        J[name] = {"deps": j["deps"], "used": sorted(used.get(name, [])), "pred": j["pred"],
                   "verdict": b.get("verdict") or b.get("title") or "",
                   "because": (b.get("because") or b.get("breaks_if") or "")[:400],
                   "blocked": why, "waiting": keys}

    labels = (brief.get("labels") or {}) if brief else {}
    # a grouping is declared under whatever name the session gives it; `fronts` is one
    # record's name for its grouping, and still reads as one
    fronts_decl = (brief.get("groups") or brief.get("fronts") or {}) if brief else {}
    front_map = {}
    for fname, sels in fronts_decl.items():
        for sel in ([sels] if isinstance(sels, str) else sels):
            for k in resolve(sel, ids, jud, flags):
                front_map.setdefault(k, str(fname))

    def fx(k, text=None, cls="fx"):
        return (f'<span class="{cls}" data-id="{html.escape(k)}">'
                f'{html.escape(str(text if text is not None else k))}</span>')

    def lbl(k):
        """What to call this on a surface that is not about keys. A label is a
        presentation choice, so the brief may set one; the record only supplies one if
        it happens to carry a name of its own."""
        if k in labels:
            return str(labels[k])
        return named(raw.get(k)) or human(k)

    def refs(text):
        """Link every entry id the text literally names. No inference: the id is there."""
        out, pos = [], 0
        for m in P.ID.finditer(text):
            if m.group(0) not in E and m.group(0) not in J:
                continue
            out.append(html.escape(text[pos:m.start()]))
            out.append(f'<span class="fx in" data-id="{html.escape(m.group(0))}">'
                       f'{html.escape(m.group(0))}</span>')
            pos = m.end()
        out.append(html.escape(text[pos:]))
        return "".join(out)

    def shown(k, pretty=False):
        """-> html for this entry's value, with a rule's own references made live."""
        e = E.get(k) or {}
        if e.get("v") is not None:
            return html.escape(fmt(e["v"]) if pretty else str(e["v"]))[:400]
        return ("= " + refs(str(e["rule"]))) if e.get("rule") else ""

    def front(k):
        return front_map.get(k, "")

    def kicker(k, seen_fronts):
        """Which front this row is under - shown only where it is not already obvious."""
        f = front(k)
        return f'<span class="grp">{html.escape(f)}</span>' if f and len(seen_fronts) > 1 else ""

    def note(k):
        n = (E.get(k) or {}).get("note")
        return f'<div class="nt" dir="auto">{html.escape(n)}</div>' if n else ""

    def val(k):
        e = E.get(k) or {}
        if e.get("v") is not None:
            return str(e["v"])
        return ("= " + str(e["rule"])) if e.get("rule") else ""

    def has_value(k):
        return (E.get(k) or {}).get("v") is not None

    # ── renderers ────────────────────────────────────────────────────────────
    anchored = [0, 0]

    def r_cards(names):
        o = []
        for name in names:
            j, b = jud[name], J[name]
            verdict, h1 = anchor(b["verdict"], j["deps"], E)
            because, h2 = anchor(b["because"], j["deps"], E)
            miss = [d for d in j["deps"] if d not in E and d not in J]
            rest = [d for d in j["deps"] if d not in (h1 | h2) and d not in miss]
            anchored[0] += len(h1 | h2)
            anchored[1] += len([d for d in j["deps"] if d not in miss])
            o.append('<div class="card">'
                     f'<div class="vd fx" data-id="{html.escape(name)}" dir="auto">{verdict}</div>'
                     + (f'<div class="bc" dir="auto">{because}</div>' if because else "")
                     # what is missing stays visible; what is present is reachable by
                     # hovering the words that already mention it.
                     + ('<div class="deps">' + "".join(
                         (f'<span class="dep wait" title="declared missing - this judgment '
                          f'is waiting on it">{html.escape(d)}</span>' if b["blocked"] else
                          f'<span class="dep dead" title="not an entry in this record">'
                          f'{html.escape(d)}</span>') for d in miss) + "</div>" if miss else "")
                     + (f'<div class="rest">rests on {len(rest)} more &mdash; hover the line above</div>'
                        if rest else "")
                     + "</div>")
        return "".join(o)

    def r_alerts(names):
        def rank(n):
            return -max((URGENCY[f][0] for f in flags.get(n, ()) if f in URGENCY), default=0)
        o = ['<div class="alerts">']
        for name in sorted(names, key=lambda n: (rank(n), n)):
            fs = sorted(flags.get(name, ()), key=lambda f: -URGENCY.get(f, (0, ""))[0])
            # the dot shows the worst state the judgment is in, whatever order the
            # reasons are listed in
            tones = {URGENCY.get(f, (0, "ok"))[1] for f in fs}
            tone = next((t for t in ("stop", "warn", "mut") if t in tones), "ok")
            v, _ = anchor(J[name]["verdict"], jud[name]["deps"], E)
            # the key it is waiting on is the whole content of a blocked line - it is
            # what someone has to go and get, so it stays visible here
            miss = J[name]["waiting"] or [d for d in jud[name]["deps"] if d not in E and d not in J]
            why = J[name]["blocked"] or "; ".join(SAYS.get(f, f) for f in fs) or "holds"
            if miss and not J[name]["blocked"]:
                why += " \u2014 " + ", ".join(lbl(d) for d in miss)
            fr = front(name) or next((front(d) for d in jud[name]["deps"] if front(d)), "")
            o.append(f'<div class="al"><span class="dot" style="background:var(--{tone})"></span>'
                     f'<span class="at">'
                     + (f'<span class="grp">{html.escape(fr)}</span>' if fr else "")
                     + f'<span class="fx" data-id="{html.escape(name)}" dir="auto">'
                     f'{v}</span><div class="aw" dir="auto">{html.escape(why)}</div>'
                     f'</span></div>')
        return "".join(o) + "</div>"

    DERIVED = '<span class="derived">worked out</span>'

    def r_table(keys, raw_keys=False):
        rows = []
        for k in keys:
            cell = shown(k, True) if has_value(k) else DERIVED
            head = fx(k) if raw_keys else fx(k, lbl(k))
            cls = "k" if raw_keys else "kl"
            rows.append(f'<tr><td class="{cls}" dir="auto">{head}</td>'
                        f'<td class="v" dir="auto">{cell}</td></tr>')
        return "<table>" + "".join(rows) + "</table>"

    def r_lines(keys):
        return '<div class="deps">' + "".join(
            f'<span class="fx dep" data-id="{html.escape(k)}" dir="auto">{html.escape(lbl(k))}</span>'
            for k in keys) + "</div>"

    def r_timeline(keys):
        today = datetime.date.today()
        seq = sorted(((as_date(val(k)), k) for k in keys), key=lambda t: t[0])
        fs = {front(k) for k in keys if front(k)}
        o, marked = ['<div class="tl">'], False
        for d, k in seq:
            if not marked and d >= today:
                o.append(f'<div class="tlr mark">today &middot; {today.strftime("%d/%m/%Y")}</div>')
                marked = True
            o.append(f'<div class="tlr{" past" if d < today else ""}">'
                     f'<span class="when">{d.strftime("%d/%m/%Y")}</span>'
                     f'<span class="what" dir="auto">{kicker(k, fs)}{fx(k, lbl(k))}'
                     f'{note(k)}</span></div>')
        if not marked:
            o.append(f'<div class="tlr mark">today &middot; {today.strftime("%d/%m/%Y")}</div>')
        return "".join(o) + "</div>"

    def r_fronts(keys):
        g = {}
        for k in keys:
            g.setdefault(front(k) or labels.get(k.split(".")[0])
                         or (k.split(".")[0] if "." in k else "-"), []).append(k)
        o = ['<div class="grid">']
        for name, ks in sorted(g.items(), key=lambda kv: (-len(kv[1]), kv[0])):
            o.append(f'<div class="front"><h3 dir="auto">{html.escape(name)}</h3>' + "".join(
                (f'<div class="kv"><span class="kl">{fx(k, lbl(k))}</span>'
                 f'<span class="kvv">{shown(k, True)}</span></div>'
                 if has_value(k) else
                 f'<div class="kv"><span class="kl">{fx(k, lbl(k))}</span>'
                 f'<span class="kvv">{DERIVED}</span></div>')
                for k in ks) + "</div>")
        return "".join(o) + "</div>"

    def r_headline(keys):
        return '<div class="heads">' + "".join(
            f'<div class="head"><div class="big">{fx(k, fmt((E.get(k) or {}).get("v")))}</div>'
            f'<div class="cap" dir="auto">{kicker(k, {front(x) for x in keys if front(x)})}'
            f'{html.escape(lbl(k))}</div>{note(k)}</div>' for k in keys) + "</div>"

    ENTRY_R = {"table": r_table, "lines": r_lines, "timeline": r_timeline,
               "fronts": r_fronts, "headline": r_headline}
    JUD_R = {"cards": r_cards, "alerts": r_alerts}
    cards = r_cards

    # ── the session's tab ───────────────────────────────────────
    now_html, covered, empty_sections, misfit = [], set(), [], []
    # the record's own name heads the page; a record without one borrows the brief's
    # title, which is presentation and so lives in the brief
    rec_name = named(meta) or str(brief.get("title") or "").strip()
    h1 = (f'<h1 dir="auto">{html.escape(rec_name)}</h1>' if rec_name
          else '<h1 dir="ltr">What is known here</h1>')
    if brief:
        now_html.append(h1)
        if brief.get("intent"):
            # the page is named for the record; the intent is the arrangement's aim,
            # not a title - it is another task on the way, and it reads like one.
            now_html.append('<p class="purpose" dir="auto">Everything on this tab was picked '
                            f'for one purpose — <b dir="auto">{html.escape(str(brief["intent"]))}</b></p>'
                            + '<p class="sub" dir="ltr">'
                            + f'The Record tab has all {len(ids)} entries and judgments, '
                            f'arranged by nothing.</p>')
        was = brief.get("shape") or {}
        if not was:
            now_html.append('<div class="banner">This arrangement records no shape, so nothing '
                            'can tell whether it went stale. Add a <code>shape:</code> block '
                            '(printed on stderr when this page was generated).</div>')
        else:
            moved = [f"{k}: {was[k]} &rarr; {shape[k]}" for k in shape if k in was and was[k] != shape[k]]
            if moved:
                now_html.append('<div class="banner" dir="auto">The record has changed shape since '
                                'this arrangement was written &mdash; ' + "; ".join(moved) +
                                '. The sections below still fill themselves, but the sections '
                                'themselves may no longer be the right ones.</div>')
        for sec in brief.get("sections") or []:
            picks = sec.get("pick")
            picks = [picks] if isinstance(picks, str) else list(picks or [])
            got = set()
            for x in picks:
                got |= resolve(x, ids, jud, flags)
            covered |= got
            jn = sorted(x for x in got if x in jud)
            en = sorted(x for x in got if x not in jud)
            title = str(sec.get("title") or ",".join(picks))
            kind = str(sec.get("as") or "").strip()
            wrong = fits(kind, sorted(got), jud, E) if kind else None
            if wrong:
                misfit.append((title, wrong))
                kind = ""
            if not got:
                empty_sections.append(title)
            now_html.append(f'<h2 dir="auto">{html.escape(title)} <span class="n">{len(got)}</span></h2>')
            if sec.get("why"):
                now_html.append(f'<div class="why" dir="auto">{html.escape(str(sec["why"]))}</div>')
            if wrong:
                now_html.append(f'<div class="bad">{html.escape(wrong)} &mdash; fell back to the '
                                f'default shape</div>')
            if not got:
                now_html.append('<div class="why">Nothing in the record matches this section. '
                                'It is about something the record no longer holds.</div>')
            if jn:
                now_html.append((JUD_R.get(kind) or r_cards)(jn))
            if en:
                now_html.append((ENTRY_R.get(kind) or r_table)(en))
        # It may order. It may not drop. This section is not optional and the brief
        # cannot switch it off: an arrangement that hides what it did not anticipate
        # is worth less than no arrangement.
        spill = sorted(k for k, f in flags.items() if f and k not in covered)
        if spill:
            now_html.append(f'<h2 class="spill" dir="ltr">Not covered by this arrangement '
                            f'<span class="n">{len(spill)}</span></h2>'
                            '<div class="why">Flagged, and no section above picked it up. '
                            'This section is written by the page, not by the brief.</div>')
            now_html.append(r_alerts(spill))
        # the one page count the page can already give: what fell through the arrangement
        if "page.spill" in E:
            E["page.spill"]["v"] = len(spill)

    # what the brief declares beyond what this page draws - checked now, drawn later
    contract = {"tabs": 0, "texts": 0, "bad": []}
    if brief:
        tabs = [t for t in (brief.get("tabs") or []) if isinstance(t, dict)]
        contract["tabs"] = len(tabs)
        for t in tabs:
            for s in (t.get("serves") or []):
                if s not in E and s not in J:
                    contract["bad"].append(f"tab '{t.get('title') or '?'}' serves {s}, which is "
                                           f"not an entry")
        secs = [s for s in (brief.get("sections") or []) if isinstance(s, dict)]
        for t in (tabs[1:] if brief.get("_first_tab_drawn") else tabs):
            secs += [s for s in (t.get("sections") or []) if isinstance(s, dict)]
        for sec in secs:
            title = str(sec.get("title") or "?")
            if sec.get("text"):
                contract["texts"] += 1
                for r in P.refs_in(sec["text"]):
                    if r not in E and r not in J:
                        contract["bad"].append(f"section '{title}': text references {r}, which "
                                               f"is not an entry")
            for k in (sec.get("seen") or {}):
                if k not in E and k not in J:
                    contract["bad"].append(f"section '{title}': seen names {k}, which is not "
                                           f"an entry")

    # ── the record's own tab ─────────────────────────────────────────────────
    rec_html = []
    if jud:
        rec_html.append(f'<h2>Judgments <span class="n">{len(jud)}</span></h2>')
        rec_html.append(cards(sorted(jud)))
    for g, keys in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        rec_html.append(f'<h2 id="g-{html.escape(g)}">{html.escape(g)}</h2>' + r_table(keys, True))

    # ── the page ─────────────────────────────────────────────────────────────
    ns = '<nav class="ns" dir="ltr">' + "".join(
        f'<a href="#g-{html.escape(g)}">{html.escape(g)} ({len(v)})</a>'
        for g, v in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0]))) + "</nav>"
    head = [h1]
    if meta.get("scope"):
        head.append(f'<p class="scope" dir="auto">{html.escape(str(meta["scope"]).strip())}</p>')
    head.append(f'<p class="meta" dir="ltr">{shape["entries"]} entries and {shape["judgments"]} '
                f'judgments'
                + (f'. {shape["flagged"]} need a person' if shape["flagged"] else "")
                + (f'. Last updated {html.escape(str(meta["updated"]))}' if meta.get("updated") else "")
                + "</p>" + ns)

    out = ['<!doctype html><html><head><meta charset="utf-8">',
           '<meta name="viewport" content="width=device-width,initial-scale=1">',
           f'<title>{html.escape(str(brief.get("title") or rec_name or meta.get("scope") or "record")[:60])}</title>',
           f'<style>{CSS}</style></head><body><div class="wrap" dir="{direction(doc)}">']

    tree = (h1 + '<p class="purpose" dir="ltr">The whole record as one growing thing — '
            'roots are what was read from the world, the canopy is what was concluded '
            'from it. Hover anything.</p>'
            '<div class="treewrap">' + tree_svg(ids, jud, E, J, flags) + '</div>'
            '<p class="sub" dir="ltr" style="margin-top:8px">roots — read from the world '
            '&middot; branches — worked out &middot; blossoms — concluded &middot; '
            'the trunk is where they meet</p>')
    tabs = ['<div class="tabs" role="tablist">']
    if brief:
        # a tab written as a tab carries its own name; a bare brief is "Now"
        tabs.append('<button type="button" data-tab="now" aria-selected="true">'
                    + html.escape(str(brief.get("_tab_title") or "Now"))
                    + (f' <span class="n">{len(covered)}</span>' if covered else "") + "</button>")
    tabs.append(f'<button type="button" data-tab="record" aria-selected='
                f'"{"false" if brief else "true"}">Record <span class="n">{len(ids)}</span></button>')
    tabs.append('<button type="button" data-tab="tree" aria-selected="false">Tree</button></div>')
    out.append("".join(tabs))
    if brief:
        out.append('<section id="panel-now">' + "".join(now_html) + "</section>")
    out.append(f'<section id="panel-record"{" hidden" if brief else ""}>'
               + "".join(head + rec_html) + "</section>")
    out.append('<section id="panel-tree" hidden>' + tree + "</section>")

    out.append('<footer dir="ltr">Hover any key for where it came from. Click to pin, click a dependency '
               'to walk to it, Esc to step back. While a card is open, a solid outline marks '
               'everything that rests on it and a dashed one what it rests on. Generated from '
               'the record - nothing here was typed twice.' + (' The <b>Now</b> tab is an arrangement someone chose; '
               '<b>Record</b> is everything, arranged by nothing.' if brief else '') + "</footer>")
    # sorted keys, so two builds of an unchanged record are the same bytes - the one thing
    # a generated page is for is being diffed against the last one
    out.append("</div><script>window.__E=" + json.dumps(_plain(E), ensure_ascii=False, sort_keys=True)
               + ";window.__J=" + json.dumps(_plain(J), ensure_ascii=False, sort_keys=True) + ";</script>")
    out.append(f"<script>{JS}</script></body></html>")
    return "\n".join(out), E, J, ids, {"shape": shape, "empty": empty_sections,
                                       "misfit": misfit, "brief": bool(brief), "anchored": tuple(anchored),
                                       "unnamed": sorted(k for k in E if not E[k].get("name")
                                                         and k not in labels),
                                       "covered": covered, "flags": flags, "contract": contract}


def verify(paths, brief_path=None):
    """Deterministic, no browser. What only looking can catch is a separate job."""
    page, E, J, ids, info = build(paths, brief_path)
    fail = []
    # what the page SHOWS is markup, not script - the provenance layer's own source
    # mentions the attribute it binds to, and that is not an element.
    dom = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
    shown = set(re.findall(r'data-id="([^"]+)"', dom))
    for k in shown:
        if k not in E and k not in J:
            fail.append(f"page shows {k}, which is not in the record")
    for k in E:
        if k not in shown:
            fail.append(f"{k} is in the payload but nothing on the page shows it")
    note = []
    for name, j in J.items():
        for d in j["deps"]:
            if d not in E and d not in J:
                (note if j["blocked"] else fail).append(
                    f"{name} links to {d}, which the payload does not carry"
                    + (" - declared, so the page shows it as awaited" if j["blocked"] else ""))
    if "window.__E=" not in page or "window.__J=" not in page:
        fail.append("payload missing")
    if info["brief"]:
        u = info["unnamed"]
        if u:
            note.append(f"{len(u)} entries carry no human name, so the page has to fall back to "
                        f"their keys: {', '.join(u[:6])}"
                        + (f" and {len(u) - 6} more" if len(u) > 6 else ""))
        a, t = info["anchored"]
        if t:
            note.append(f"{a} of {t} live dependencies are named in the prose that cites them; "
                        f"the rest are reachable only by hovering the judgment")
        if 'data-tab="now" aria-selected="true"' not in page:
            fail.append("a brief exists but the session tab is not the default")
        c = info["contract"]
        fail += c["bad"]
        if c["tabs"] > 1:
            note.append(f"{c['tabs']} tabs declared; the page draws the first and keeps the rest")
        if c["texts"]:
            note.append(f"text on {c['texts']} section{'s' if c['texts'] != 1 else ''} is "
                        f"checked and not yet drawn")
        # An authored section that picks nothing is the alert row about something already
        # closed: it costs trust on everything else on the page.
        for t, why in info["misfit"]:
            fail.append(f"section '{t}': {why}")
        for t in info["empty"]:
            fail.append(f"section '{t}' picks nothing - it is about something the record "
                        f"no longer holds")
        missed = [k for k, f in info["flags"].items() if f and
                  f'data-id="{k}"' not in dom]
        for k in missed:
            fail.append(f"{k} is flagged but does not appear on the Now tab at all")
    for n in note:
        print("NOTE", n)
    for f in fail:
        print("FAIL", f)
    print(f"{len(shown)} elements, {len(E)} entries, {len(J)} judgments, "
          + ("2 tabs, " if info["brief"] else "1 tab, ") + f"{len(fail)} problems")
    return 1 if fail else 0


if __name__ == "__main__":
    a = sys.argv[1:]
    brief = a[a.index("--brief") + 1] if "--brief" in a else None
    files = [x for x in a if x.endswith((".yaml", ".yml")) and x != brief] or P.default_paths()
    brief = find_brief(files, brief)
    if "--verify" in a:
        sys.exit(verify(files, brief))
    page, _, _, _, info = build(files, brief)
    if info["brief"] and not effective(yaml.safe_load(io.open(brief, encoding="utf-8").read()) or {}).get("shape"):
        sys.stderr.write("# no shape recorded in the brief. paste this into it, so a later\n"
                         "# render can tell you the arrangement went stale:\nshape:\n"
                         + "".join(f"  {k}: {v}\n" for k, v in info["shape"].items()))
    sys.stdout.write(page)
