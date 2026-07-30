"""The one-click interactive demo page. Server-rendered once; the browser drives the rest.

Reuses the Daylight CSS and shell from app.page so the demo cannot drift from the product.
"""

from __future__ import annotations

# CSS is inlined by _shell; the demo adds only what the product page does not already have.
from app.page import ARROW, OMEGA_SVG, _shell

MESSAGE = "Get me the Q3 partner rebate total from the Northwind supplier portal."

EXTRA_CSS = """
.composer{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:18px 18px 14px}
.composer .msg{font-size:17.5px;line-height:1.5;color:var(--ink);margin:0}
.composer .send{display:flex;align-items:center;justify-content:space-between;gap:14px;flex-wrap:wrap;
  border-top:1px solid var(--line);margin-top:16px;padding-top:14px}
.composer .fixed{color:var(--muted);font-size:13.5px}
.log{margin:26px 0 0;display:grid;grid-template-columns:max-content 1fr;gap:0;
  border-top:1px solid var(--line)}
.log .who{padding:12px 24px 12px 0;font-size:13px;font-weight:600;color:var(--muted);
  border-bottom:1px solid var(--line);white-space:nowrap}
.log .what{padding:12px 0;font-size:16.5px;border-bottom:1px solid var(--line)}
.log .who.wall{color:#8C4A3F}
.log .who.phone{color:var(--amber)}
.log .who.human,.log .who.done{color:var(--green)}
.log .who.error{color:#8C4A3F}
.log .what.done{font-weight:600}
.runbar{display:flex;align-items:center;gap:14px;flex-wrap:wrap;margin-top:24px;
  font-size:13.5px;color:var(--muted);font-variant-numeric:tabular-nums}
.deliverable{font-size:22px;line-height:1.4;font-weight:600;letter-spacing:-.01em;margin:0}
.quiet{color:var(--muted);font-size:14px}
.hide{display:none}
@media (max-width:640px){
  .log{grid-template-columns:1fr}
  .log .who{padding:13px 0 0;border-bottom:0}
  .log .what{padding:2px 0 13px}
}
"""


def render_demo_page() -> str:
    head = f"<style>{EXTRA_CSS}</style>"

    body = f"""
<a class=mark href="/">{OMEGA_SVG}<span>Handoff</span></a>

<h1 class=r style=margin-top:32px;max-width:21ch>The agent stops. You clear it. It finishes.</h1>
<p class="lede r" style=animation-delay:.06s;margin-top:18px>Send the message below and a real agent
starts working in a real browser. It reaches a human-verification checkbox it is not allowed to tick,
so it pages a person and blocks. <b>That page is a phone call to the owner of this project, placed the
moment you press Send.</b> You get the agent's browser, you tick the box, and the run finishes in
front of you.</p>

<div class="composer r" style=animation-delay:.12s;margin-top:26px>
  <p class=msg id=msg>{MESSAGE}</p>
  <div class=send>
    <span class=fixed>Fixed for the demo, so every run is the same run.</span>
    <button class=cta id=send><span>Send</span><span class=chip>{ARROW}</span></button>
  </div>
</div>

<div class="runbar hide" id=runbar>
  <span class="state pending" id=runstate><span class=dot></span>The agent is working</span>
  <span id=elapsed>0s</span>
  <span id=trouble class=hide></span>
</div>

<dl class="log hide" id=log></dl>

<div class="card hide" id=blocked style=margin-top:24px>
  <h2>The agent cannot tick this box. You can.</h2>
  <p class=lede style=font-size:15.5px;max-width:58ch>Tick the checkbox in the page below, then press
  the button. The agent's blocked call returns and it reads the number from behind the wall.</p>
  <div class=row>
    <button class=cta id=cleared><span>I cleared it</span><span class=chip>{ARROW}</span></button>
    <a id=pagelink class=hide href="#" target=_blank rel=noopener>Open the page the phone opened</a>
  </div>
  <p class="quiet hide" id=nolive style="margin:14px 0 0">This run did not attach a live view, so
  clear the wall from the page the phone opened, then press the button.</p>
</div>

<div class="frame hide" id=frame>
  <div class=bar><span id=barnote>The page the agent is stuck on &middot; your clicks land in the run</span>
  <a id=fullsize href="#" target=_blank rel=noopener>Open full size</a></div>
  <iframe id=liveview title="Agent browser live view" allow="clipboard-read; clipboard-write"></iframe>
</div>

<div class="card hide" id=result style=margin-top:24px>
  <h2 id=resulth>What the agent came back with</h2>
  <p class=deliverable id=deliverable></p>
  <p class=quiet id=resultnote style="margin:10px 0 0"></p>
  <div class=row id=againrow style=margin-top:16px><button class=cta id=again>
    <span>Send it again</span><span class=chip>{ARROW}</span></button></div>
</div>

<p class=foot>Handoff pages a real person when an agent hits a wall. <a href="/">What this is</a>. Part of <a href="https://omegas.dev" target=_blank rel="noopener noreferrer">Omegas</a>.</p>
"""

    script = """<script>
(function(){
  var send=document.getElementById('send'), log=document.getElementById('log'),
      runbar=document.getElementById('runbar'), runstate=document.getElementById('runstate'),
      elapsed=document.getElementById('elapsed'), trouble=document.getElementById('trouble'),
      blocked=document.getElementById('blocked'), frame=document.getElementById('frame'),
      liveview=document.getElementById('liveview'), fullsize=document.getElementById('fullsize'),
      pagelink=document.getElementById('pagelink'), cleared=document.getElementById('cleared'),
      barnote=document.getElementById('barnote'),
      result=document.getElementById('result'), resulth=document.getElementById('resulth'),
      deliverable=document.getElementById('deliverable'), resultnote=document.getElementById('resultnote'),
      againrow=document.getElementById('againrow'), again=document.getElementById('again'),
      nolive=document.getElementById('nolive');

  var runId=null, handoffId=null, drawn=0, timer=null, tick=null, t0=0,
      misses=0, stopped=false, liveSet=false;
  var LABEL={agent:'Agent',wall:'Wall',phone:'Phone',human:'Human',done:'Done',error:'Error'};

  function show(el,on){ el.classList.toggle('hide',!on); }
  function label(btn,text){ btn.querySelector('span').textContent=text; }

  function addStep(s,i){
    var kind=(s&&s.kind)||'agent';
    var who=document.createElement('dt');
    who.className='who '+kind+' r';
    who.style.animationDelay=(Math.min(i,6)*0.02)+'s';
    who.textContent=LABEL[kind]||'Agent';
    var what=document.createElement('dd');
    what.className='what '+kind+' r';
    what.style.animationDelay=(Math.min(i,6)*0.02)+'s';
    what.textContent=(s&&s.text)||'';
    log.appendChild(who); log.appendChild(what);
    show(log,true);
  }

  function paint(run){
    var steps=(run&&run.steps)||[];
    for(var i=drawn;i<steps.length;i++) addStep(steps[i],i);
    if(steps.length>drawn) drawn=steps.length;

    var status=(run&&run.status)||'running';

    // Every field can arrive before or after the status flips; key off the fields, not the order.
    if(run&&run.handoff_id) handoffId=run.handoff_id;
    if(run&&run.mode==='sprite'){
      barnote.textContent="The agent's own browser \u00b7 your clicks and keystrokes are relayed";
    }
    if(run&&run.live_view_url&&!liveSet){
      liveview.src=run.live_view_url; fullsize.href=run.live_view_url; liveSet=true; show(frame,true);
    }
    if(run&&run.page_url){ pagelink.href=run.page_url; show(pagelink,true); }

    show(blocked,status==='blocked');
    if(status==='blocked'){
      runstate.lastChild.textContent='Waiting on you';
      cleared.disabled=!handoffId;
      show(nolive,!liveSet);
    }

    if(status==='done'||status==='failed'){
      stopped=true;
      if(timer){clearInterval(timer);timer=null;}
      if(tick){clearInterval(tick);tick=null;}
      show(blocked,false);
      show(frame,false);   // the run is over; the result belongs directly under the log
      runstate.className='state '+(status==='done'?'resolved':'expired');
      runstate.lastChild.textContent=status==='done'?'Finished':'Stopped';
      show(result,true);
      if(status==='done'){
        resulth.textContent='What the agent came back with';
        var got=(run&&run.deliverable)||'';
        deliverable.textContent=got ? (/[A-Za-z]/.test(got) ? got
          : 'Q3 partner rebate total: '+got) : 'The run finished without a number.';
        resultnote.textContent='A person ticked the box. Nothing behind that wall was reachable until they did.';
      }else{
        resulth.textContent='The run stopped';
        deliverable.textContent=(run&&run.error)||'The run stopped without saying why.';
        resultnote.textContent='Nothing was faked to get past it.';
      }
      show(againrow,true);
      label(send,'Send'); send.disabled=false;
      if(again) again.disabled=false;
    }
  }

  async function poll(){
    if(stopped||!runId) return;
    try{
      var res=await fetch('/demo/run/'+runId,{cache:'no-store'});
      if(!res.ok) throw new Error('status '+res.status);
      var run=await res.json();
      misses=0; show(trouble,false);
      paint(run);
    }catch(e){
      misses++;
      if(misses>=5){
        trouble.textContent='Lost contact with the run. Still retrying.';
        show(trouble,true);
      }
    }
  }

  // A run that was never allowed to start is not a failed run; say so in its own words.
  function refuse(text){
    stopped=true;
    if(tick){clearInterval(tick);tick=null;}
    if(timer){clearInterval(timer);timer=null;}
    show(runbar,false); show(log,false);
    show(result,true); show(againrow,true);
    resulth.textContent='No run started';
    // The server's own words, sentence-cased so it reads like the rest of the page.
    deliverable.textContent=text.charAt(0).toUpperCase()+text.slice(1)+(/[.!?]$/.test(text)?'':'.');
    resultnote.textContent='Every run rings a real phone, so they are capped.';
    send.disabled=false; label(send,'Send');
    if(again) again.disabled=false;
  }

  async function start(){
    send.disabled=true; label(send,'Working');
    if(again) again.disabled=true;
    log.textContent=''; drawn=0; misses=0; stopped=false; liveSet=false;
    handoffId=null; liveview.removeAttribute('src');
    cleared.disabled=false; label(cleared,'I cleared it');
    show(log,false); show(blocked,false); show(frame,false); show(result,false);
    show(againrow,false); show(pagelink,false); show(trouble,false); show(nolive,false);
    runstate.className='state pending';
    runstate.lastChild.textContent='The agent is working';
    show(runbar,true);
    t0=Date.now();
    if(tick) clearInterval(tick);
    elapsed.textContent='0s';
    tick=setInterval(function(){ elapsed.textContent=Math.round((Date.now()-t0)/1000)+'s'; },1000);
    try{
      var res=await fetch('/demo/run',{method:'POST',headers:{'content-type':'application/json'},body:'{}'});
      if(res.status===429||res.status===503){
        var why='';
        try{ var d=await res.json(); why=(d&&d.detail)||''; }catch(e2){}
        return refuse(why||(res.status===429
          ? 'A demo is already running; try again in a minute.'
          : 'The demo runner is not available right now.'));
      }
      if(!res.ok) throw new Error('status '+res.status);
      var j=await res.json();
      runId=j&&j.id;
      if(!runId) throw new Error('no run id');
    }catch(e){
      stopped=true;
      if(tick){clearInterval(tick);tick=null;}
      runstate.className='state expired';
      runstate.lastChild.textContent='Stopped';
      show(result,true); show(againrow,true);
      resulth.textContent='The run never started';
      deliverable.textContent='The demo could not reach the server ('+e.message+').';
      resultnote.textContent='';
      send.disabled=false; label(send,'Send');
      if(again) again.disabled=false;
      return;
    }
    poll();
    if(timer) clearInterval(timer);
    timer=setInterval(poll,1000);
  }

  send.addEventListener('click',start);
  if(again) again.addEventListener('click',function(){ again.disabled=true; start(); });

  cleared.addEventListener('click',async function(){
    if(!handoffId){ label(cleared,'No request to hand back yet'); return; }
    cleared.disabled=true; label(cleared,'Handing back');
    try{
      var res=await fetch('/v1/requests/'+encodeURIComponent(handoffId)+'/resolve',
        {method:'POST',headers:{'content-type':'application/json'},
         body:JSON.stringify({cleared:true,by:'visitor'})});
      if(!res.ok) throw new Error('status '+res.status);
      label(cleared,'Handed back');
      poll();
    }catch(e){
      cleared.disabled=false; label(cleared,'Retry');
    }
  });
})();
</script>"""

    return _shell("Handoff — see an agent ask for help", body + script, head)
