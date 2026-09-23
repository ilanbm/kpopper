(function(){
 var T=window.__T||{};
 // Dates are calendar days. UTC ordinals avoid DST-length days; the local date is
 // sampled anew each tick, so a page left open over midnight remains a reading.
 function count(key,n){
  var lang=document.documentElement.lang,form=n===1?'one':n===2?'two':
   lang==='ar'&&n%100>=11&&n%100<=99?'many':
   lang==='ar'&&!(n%100>=3&&n%100<=10)?'other':'';
  return T[key+(form?'_'+form:'')].replace('{n}',n);
 }
 function dayNumber(y,m,d){return Date.UTC(y,m-1,d)/86400000}
 function refreshDates(){
  var nowDate=new Date(),today=dayNumber(nowDate.getFullYear(),nowDate.getMonth()+1,nowDate.getDate());
  var iso=[nowDate.getFullYear(),String(nowDate.getMonth()+1).padStart(2,'0'),String(nowDate.getDate()).padStart(2,'0')].join('-');
  document.querySelectorAll('[data-countdown]').forEach(function(el){
   var date=el.dataset.countdown.split('-').map(Number),n=dayNumber(date[0],date[1],date[2])-today;
   var value=el.querySelector('[data-id]');
   if(value)value.textContent=n===0?T.today:count(n>0?'days_left':'days_ago',Math.abs(n));
  });
  document.querySelectorAll('.tl').forEach(function(tl){
   tl.querySelectorAll('[data-calendar-marker]').forEach(function(el){if(el.dataset.day!==iso)el.remove()});
   if(!tl.querySelector('[data-day="'+iso+'"]')){
    var day=document.createElement('div');day.className='day';day.dataset.day=iso;day.dataset.calendarMarker='true';
    var when=document.createElement('div');when.className='when';when.dataset.clock='today';
    when.textContent=iso.split('-').reverse().join('/');day.appendChild(when);
    var label=document.createElement('div');label.className='day-label';day.appendChild(label);
    var next=[].slice.call(tl.querySelectorAll('[data-day]')).find(function(el){return el.dataset.day>iso});
    tl.insertBefore(day,next||null);
   }
  });
  document.querySelectorAll('[data-day]').forEach(function(el){
   var date=el.dataset.day.split('-').map(Number),n=dayNumber(date[0],date[1],date[2])-today;
   el.classList.toggle('past',n<0);el.classList.toggle('hot',n===0);
   el.querySelector('.day-label').textContent=n===0?T.today:'';
  });
 }
 refreshDates();setInterval(refreshDates,30000);
 document.addEventListener('visibilitychange',refreshDates);

})();
