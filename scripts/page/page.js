(function(){
 var E=window.__E||{},J=window.__J||{},T=window.__T||{},pop=null,cur=null,hist=[],now=null,tmr=null,pin=false,
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
  var h='<div class="hd">'+(hist.length?'<button class="back" type="button">'+esc(T.back)+'</button>':'')+
        '<span class="kref">'+esc(id)+'</span>'+
        (document.getElementById('panel-tree')
          ?'<button class="tfoc" type="button" title="'+esc(T.tree_btn_title)+'">'+esc(T.tree_btn)+'</button>':'')+
        '</div>';
  if(J[id]){var j=J[id];
   return h+(j.verdict?row(T.concludes,'<b>'+esc(j.verdict)+'</b>'):'')+
    row(T.rests_on,'<span class="deps">'+(j.deps||[]).map(function(d){
      return '<span class="dep" data-go="'+esc(d)+'">'+esc(d)+'</span>'}).join('')+'</span>')+
    (j.pred?row(T.wrong_if,link(j.pred)):'')+
    (j.blocked?row(T.blocked,esc(j.blocked)):'')+
    (j.reopened?row(T.reopened_by,esc(j.reopened)):'')+
    (j.because?row(T.because,esc(j.because)):'')}
  var e=E[id]||{};
  return h+(e.name?'<div class="nm">'+esc(e.name)+'</div>':'')+
   (e.asked?row(T.asked,esc(e.asked)):'')+
   (e.v!=null?row(T.value,'<b>'+esc(e.v)+'</b>'):'')+
   (e.rule?row(T.rule,link(e.rule)):'')+
   (e.measure!=null?row(T.measure,esc(e.measure)):'')+
   (e.from?row(T.source,esc(e.from)):'')+(e.at?row(T.at,esc(e.at)):'')+
   (e.file?row(T.file,esc(e.file)):'')+(e.url?row(T.url,esc(e.url)):'')+
   (e.of||e.read?row(T.as_of,esc(e.of||e.read)):'')+
   (e.used&&e.used.length?row(T.used_by,'<span class="deps">'+e.used.map(function(d){
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
  chip.textContent=T.whole_tree;
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
  pop=document.createElement('div');pop.className='pop';pop.dir=T.dir||'ltr';document.body.appendChild(pop);
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
