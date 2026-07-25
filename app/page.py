"""Server-rendered HTML for Handoff. No build step, no framework, no CDN."""

from __future__ import annotations

import html
import json
import os

CSS = """
*,*::before,*::after{box-sizing:border-box}
:root{
  --canvas:#FAFAF8; --stone:#F0EDE7; --card:#FFFFFF; --ink:#26241F; --muted:#6E6A60;
  --line:#E3DFD7; --green:#3D6B44; --green-deep:#325838; --amber:#B98A4A; --scrim:#2A2118;
}
html,body{margin:0;padding:0}
body{
  background-color:var(--canvas);
  background-image:radial-gradient(circle at 1px 1px, rgba(42,33,24,.055) 1px, transparent 0);
  background-size:22px 22px;
  color:var(--ink);
  font:16px/1.55 "Schibsted Grotesk",-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;
  -webkit-font-smoothing:antialiased;
  text-rendering:optimizeLegibility;
}
.wrap{max-width:940px;margin:0 auto;padding:34px 22px 90px}
a{color:var(--green);text-decoration:none}
a:hover{text-decoration:underline}
.mark{display:inline-flex;align-items:center;gap:9px;font-size:19px;font-weight:700;
  letter-spacing:-.015em;color:var(--ink)}
.mark:hover{text-decoration:none}
.mark svg{width:.78em;height:.76em;color:var(--green)}
h1{font-size:clamp(31px,5.1vw,50px);line-height:1.08;letter-spacing:-.028em;margin:0;font-weight:700;
  overflow-wrap:break-word}
h2{font-size:15px;letter-spacing:.005em;color:var(--muted);font-weight:600;margin:0 0 12px}
p{margin:0 0 14px}
.lede{color:var(--muted);font-size:17.5px;line-height:1.6;max-width:60ch}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:24px}
.card+.card{margin-top:18px}
.row{display:flex;gap:14px;align-items:center;flex-wrap:wrap}
.between{display:flex;gap:16px;align-items:center;justify-content:space-between;flex-wrap:wrap}

/* one quiet entrance, transform+opacity only */
@keyframes rise{from{opacity:0;transform:translateY(11px)}to{opacity:1;transform:none}}
.r{animation:rise .58s cubic-bezier(.22,.72,.18,1) both}
@media (prefers-reduced-motion:reduce){.r{animation:none}}

/* the record: who did what, in order */
.trace{margin:0;display:grid;grid-template-columns:max-content 1fr;gap:0;
  border-top:1px solid var(--line)}
.trace dt{padding:13px 26px 13px 0;font-size:13.5px;font-weight:600;color:var(--muted)}
.trace dd{margin:0;padding:13px 0;font-size:16.5px;border-bottom:1px solid var(--line)}
.trace dt{border-bottom:1px solid var(--line)}
.trace dd b{font-weight:600}

.facts{display:flex;flex-wrap:wrap;gap:9px 26px;margin:18px 0 0;font-size:13.5px;
  font-variant-numeric:tabular-nums}
.facts div{display:flex;gap:7px}
.facts dt{color:var(--muted)}
.facts dd{margin:0;font-weight:600}
.facts code{font-size:.95em;color:var(--muted);overflow-wrap:anywhere}

.reason{font-size:20px;line-height:1.35;font-weight:600;margin:0 0 4px;letter-spacing:-.01em}
.state{display:inline-flex;align-items:center;gap:7px;font-size:13px;font-weight:600;
  padding:6px 12px;border-radius:999px;border:1px solid var(--line);background:var(--stone);color:var(--muted);
  white-space:nowrap}
.state.pending{color:#8A6224;border-color:#E6D6BC;background:#FBF5EA}
.state.resolved{color:var(--green);border-color:#CFE0D2;background:#EEF4EF}
.state.expired{color:#8C4A3F;border-color:#E8D2CD;background:#F9EFED}
.dot{width:7px;height:7px;border-radius:50%;background:currentColor}
.state.pending .dot{animation:pulse 1.5s ease-in-out infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.28}}
@media (prefers-reduced-motion:reduce){.state.pending .dot{animation:none}}

.cta{display:inline-flex;align-items:center;gap:12px;background:var(--green);color:#fff;border:0;
  font:700 17px/1 inherit;padding:13px 15px 13px 22px;border-radius:999px;cursor:pointer;
  text-decoration:none;
  box-shadow:0 1px 2px rgba(42,33,24,.16),0 8px 22px rgba(42,33,24,.13);transition:background .16s,transform .16s}
.cta .chip{width:32px;height:32px;border-radius:50%;background:#fff;color:var(--green);
  display:grid;place-items:center;transition:transform .16s;flex:none}
.cta:hover{background:var(--green-deep);transform:translateY(-2px);text-decoration:none}
.cta:hover .chip{transform:translateX(3px)}
.cta:disabled{background:var(--muted);cursor:default;box-shadow:none;transform:none}
.ghost{background:none;border:1px solid var(--line);color:var(--ink);border-radius:8px;
  font:600 14px/1 inherit;padding:11px 14px;cursor:pointer}
.ghost:hover{border-color:var(--muted)}
textarea{width:100%;min-height:112px;border:1px solid var(--line);border-radius:10px;padding:13px 14px;
  font:16.5px/1.5 inherit;color:var(--ink);background:var(--canvas);resize:vertical}
textarea:focus{outline:2px solid var(--green);outline-offset:1px;border-color:var(--green)}

.frame{margin:22px 0 0;border:1px solid var(--line);border-radius:12px;overflow:hidden;background:var(--scrim)}
.frame iframe{display:block;width:100%;height:540px;border:0;background:#fff}
.frame .bar{display:flex;align-items:center;justify-content:space-between;gap:10px;
  padding:10px 14px;background:var(--stone);border-bottom:1px solid var(--line);font-size:13px;color:var(--muted)}
.none{border:1px dashed var(--line);border-radius:12px;padding:26px;text-align:center;color:var(--muted);font-size:14px}
code,kbd{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.92em}
pre{background:var(--scrim);color:#F4F1EA;border-radius:12px;padding:20px;overflow-x:auto;
  font-size:13.5px;line-height:1.65;margin:0}
pre b{color:#9CC7A3;font-weight:600}
pre i{color:#B7B0A2;font-style:normal}
table{width:100%;border-collapse:collapse;font-size:14.5px}
th,td{text-align:left;padding:11px 14px;border-bottom:1px solid var(--line);vertical-align:middle}
th{color:var(--muted);font-weight:600;font-size:13px}
tbody tr:last-child td{border-bottom:0}
td:first-child{width:1%;white-space:nowrap}
.foot{margin-top:36px;color:var(--muted);font-size:13.5px}
.hr{height:1px;background:var(--line);margin:34px 0}
.section{margin-top:38px}
@media (max-width:640px){
  .wrap{padding:24px 18px 72px}
  .frame iframe{height:56vh}
  .frame .bar span{display:none}
  .frame .bar{justify-content:flex-end}
  .cta{width:100%;justify-content:space-between;padding:14px 15px 14px 22px}
  .trace{grid-template-columns:1fr;gap:0}
  .trace dt{padding:14px 0 0;border-bottom:0}
  .trace dd{padding:2px 0 14px}
  table{font-size:13.5px}
  th:nth-child(3),td:nth-child(3){display:none}
}
"""

OMEGA_SVG = (
    '<svg viewBox="0 0 358 358" fill="none" aria-hidden="true"><path fill="currentColor" '
    'd="M179 7C273.924 7 350.876 84.007 350.876 179C350.876 224.943 332.875 266.677 303.548 297.529H343.751'
    "C351.621 297.529 358 303.912 358 311.788V336.741C358 344.616 351.621 351 343.751 351H219.075V290.57"
    "C264.767 274.131 297.443 230.385 297.443 179C297.443 113.539 244.414 60.4715 179 60.4715C113.586 60.4715"
    " 60.5572 113.539 60.5572 179C60.5572 230.385 93.2333 274.131 138.925 290.57V351H14.2488C6.37939 351 "
    '4.01632e-07 344.616 0 336.741V311.788C0 303.912 6.37939 297.529 14.2488 297.529H54.4521C25.125 266.677 '
    '7.12438 224.943 7.12438 179C7.12438 84.007 84.0757 7 179 7Z"/></svg>'
)

ARROW = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h13M12 5l7 7-7 7"/></svg>'

# Landing-only styles. Every selector is prefixed `lp-` or nested under a band root,
# following the omegas.dev scoping rule: no bare generic names at global scope.
LANDING_CSS = """
.lp-nav{max-width:1180px;margin:0 auto;padding:22px clamp(20px,4vw,44px);
  display:flex;align-items:center;justify-content:space-between;gap:16px}
.lp-navlink{font-size:15px;font-weight:600;color:var(--ink)}
.lp-navlink:hover{color:var(--green);text-decoration:none}
.lp-band{padding:clamp(66px,10vh,124px) clamp(20px,4vw,44px)}
.lp-band-hero{padding-top:clamp(40px,8vh,96px);padding-bottom:clamp(64px,10vh,116px);text-align:center}
.lp-band-hero .lp-in{max-width:940px}
.lp-band-hero .lp-h1{max-width:17ch;margin-inline:auto}
.lp-band-hero .lp-lede,.lp-band-hero .lp-copy{margin-inline:auto;text-align:center}
.lp-band-hero .lp-copy{max-width:52ch}
/* the accent half of the headline always starts its own line */
.lp-band-hero .lp-h1 .tt{display:block}
.lp-band-hero .lp-actions{justify-content:center}
.lp-band-stone{background-color:var(--stone);border-top:1px solid var(--line);border-bottom:1px solid var(--line);
  background-image:radial-gradient(circle at 1px 1px, rgba(42,33,24,.062) 1px, transparent 0);
  background-size:22px 22px}
.lp-in{max-width:1180px;margin:0 auto}
.lp-in-narrow{max-width:760px;margin:0 auto}
.lp-two{display:grid;grid-template-columns:minmax(0,.86fr) minmax(0,1fr);
  gap:clamp(34px,4.6vw,72px);align-items:center}
.lp-h1{margin:0;font-size:clamp(37px,5.3vw,63px);line-height:1.04;letter-spacing:-.03em;
  font-weight:700;max-width:16ch;text-wrap:balance}
.lp-h2{margin:0;font-size:clamp(28px,3.5vw,45px);line-height:1.1;letter-spacing:-.022em;
  font-weight:700;max-width:19ch;text-wrap:balance}
.tt{color:var(--green)}
.lp-lede{margin:20px 0 0;font-size:clamp(16px,1.35vw,19px);line-height:1.55;color:var(--muted);max-width:56ch}
.lp-copy{margin:16px 0 0;font-size:clamp(15.5px,1.25vw,17.5px);line-height:1.65;max-width:46ch}
.lp-actions{margin-top:32px;display:flex;align-items:center;gap:20px;flex-wrap:wrap}
a.lp-quiet{font-size:14.5px;color:var(--green);font-weight:600}
a.lp-quiet:hover{text-decoration:underline;text-underline-offset:4px}
.lp-panel{background:var(--card);border:1px solid var(--line);border-radius:12px;
  padding:4px 20px 8px;box-shadow:0 18px 50px rgba(42,33,24,.07)}
.lp-panel-h{display:flex;align-items:baseline;justify-content:space-between;gap:14px;
  padding:15px 0 11px;border-bottom:1px solid var(--line);font-size:13px;font-weight:600;color:var(--muted)}
.lp-panel-h span{font-weight:500}
.lp-row{display:grid;grid-template-columns:92px 1fr;gap:14px;padding:14px 0;
  border-bottom:1px solid var(--line);align-items:baseline}
.lp-row:last-child{border-bottom:0}
.lp-state{font-size:10.5px;font-weight:700;letter-spacing:.07em;color:var(--green)}
.lp-state.wait{color:var(--amber)}
.lp-main{display:block;font-size:14.5px;font-weight:600;line-height:1.4}
.lp-det{display:block;margin-top:3px;font-size:12.5px;line-height:1.5;color:var(--muted)}
.lp-panel table{margin:4px 0}
.lp-panel th:first-child,.lp-panel td:first-child{padding-left:0}
.lp-panel th:last-child,.lp-panel td:last-child{padding-right:0}
.lp-open{text-align:right}
.lp-chan{margin:0;border-top:1px solid var(--line)}
.lp-chan-row{display:grid;grid-template-columns:minmax(180px,max-content) 1fr;gap:8px 30px;
  padding:18px 0;border-bottom:1px solid var(--line);align-items:baseline}
.lp-chan-name{font-size:17.5px;font-weight:600;letter-spacing:-.01em}
.lp-chan-state{margin-top:4px;font-size:12.5px;font-weight:600;color:var(--muted)}
.lp-chan-state.on{color:var(--green)}
.lp-chan-note{font-size:15.5px;line-height:1.55;color:var(--muted)}
.lp-chan-note b{color:var(--ink);font-weight:600}
.lp-center{text-align:center}
.lp-center .lp-h2,.lp-center .lp-lede{margin-left:auto;margin-right:auto}
.lp-center .lp-actions{justify-content:center}
.lp-foot{max-width:1180px;margin:0 auto;padding:34px clamp(20px,4vw,44px) 52px;
  display:flex;align-items:baseline;justify-content:space-between;gap:14px 34px;flex-wrap:wrap;
  font-size:13.5px;color:var(--muted)}
.lp-foot .mark{font-size:16px}
/* The hidden state is gated on `jsrv`, which the head script sets only when it is
   going to reveal them again. No JS, a thrown error, or reduced motion: the page
   renders final states instead of a blank column. */
.jsrv .rv{opacity:0;transform:translateY(14px);
  transition:opacity .55s cubic-bezier(.2,.6,.2,1),transform .55s cubic-bezier(.2,.6,.2,1)}
.jsrv .rv.in{opacity:1;transform:none}
.jsrv .bf{opacity:0;transform:translateY(11px);transition:opacity .5s ease-out,transform .5s ease-out}
.jsrv .bf2{transition-delay:.07s}.jsrv .bf3{transition-delay:.14s}.jsrv .bf4{transition-delay:.21s}
.jsrv .bf5{transition-delay:.28s}.jsrv .bf6{transition-delay:.35s}
.jsrv .in .bf{opacity:1;transform:none}
@media (prefers-reduced-motion:reduce){
  .jsrv .rv,.jsrv .bf,.jsrv .rv.in,.jsrv .in .bf{opacity:1;transform:none;transition:none}
}
@media (max-width:900px){
  .lp-two{grid-template-columns:1fr;gap:34px}
  .lp-h2{max-width:24ch}
  .lp-copy{max-width:56ch}
}
@media (max-width:640px){
  .lp-nav{padding:18px}
  .lp-band{padding:clamp(52px,8vh,72px) 18px}
  .lp-band-hero{padding-top:14px}
  .lp-panel{padding:2px 15px 6px}
  .lp-row{grid-template-columns:76px 1fr;gap:10px}
  .lp-chan-row{grid-template-columns:1fr;gap:6px;padding:16px 0}
  .lp-chan-note{font-size:15px}
  .lp-foot{padding:28px 18px 44px}
  .lp-actions .cta{width:100%;justify-content:space-between}
  .lp-panel thead{display:none}
  .lp-panel table,.lp-panel tbody,.lp-panel tr,.lp-panel td{display:block;width:auto}
  .lp-panel tr{padding:15px 0;border-bottom:1px solid var(--line)}
  .lp-panel tbody tr:last-child{border-bottom:0}
  .lp-panel td{padding:0;border:0;white-space:normal}
  .lp-panel td:nth-child(2){margin-top:9px;font-size:15px;line-height:1.45}
  .lp-panel td:nth-child(3){display:block;margin-top:4px;font-size:13px;color:var(--muted)}
  .lp-open{margin-top:8px;text-align:left}
}
"""


def _shell(title: str, body: str, head: str = "", full: bool = False) -> str:
    """`full` skips the centered column so a page can run edge-to-edge bands."""
    open_wrap, close_wrap = ("", "") if full else ("<div class=wrap>", "</div>")
    return (
        "<!doctype html><html lang=en><head><meta charset=utf-8>"
        '<meta name=viewport content="width=device-width,initial-scale=1">'
        f"<title>{html.escape(title)}</title><style>{CSS}</style>{head}"
        f"</head><body>{open_wrap}{body}{close_wrap}</body></html>"
    )


def _brand() -> str:
    return f'<a class=mark href="/">{OMEGA_SVG}<span>Handoff</span></a>'


def _state_pill(status: str) -> str:
    label = {"pending": "Agent is waiting", "resolved": "Handed back", "expired": "Timed out"}.get(status, status)
    return f'<span class="state {html.escape(status)}"><span class=dot></span>{html.escape(label)}</span>'


# ------------------------------------------------------------------ request page


def render_request_page(r: dict) -> str:
    rid = html.escape(r["id"])
    status = r["status"]
    is_question = r["kind"] == "question"
    prompt_text = r["question"] if is_question and r["question"] else r["reason"]
    agent = html.escape(r["agent"])

    paged = r.get("paged") or ""
    if paged == "ringing":
        page_note = "Your phone was called."
    elif paged.startswith("skipped"):
        page_note = "Paging skipped for this request."
    elif paged.startswith("failed"):
        page_note = "Phone paging failed, so you are reading this from the link instead."
    else:
        page_note = "Placing the call."

    if status == "pending":
        subline = (
            f"<b>{agent}</b> stopped here and is holding its run open until you act. {html.escape(page_note)}"
        )
    elif status == "resolved":
        subline = f"A person cleared this and <b>{agent}</b> carried on from where it stopped."
    else:
        subline = f"Nobody arrived in time, so <b>{agent}</b> took its own fallback path."

    # Only claim this is the agent's own browser when the caller said it is. A live view
    # can legitimately be just the page the agent is stuck on, and the stronger sentence
    # would be a lie there.
    caption = (
        "The agent's own browser &middot; your clicks and keystrokes are relayed"
        if r.get("live_view_is_agent_browser")
        else "The page the agent is stuck on &middot; your clicks are relayed"
    )
    live = ""
    if status == "pending":
        if r.get("live_view_url"):
            live = (
                '<div class="frame r" style=animation-delay:.16s><div class=bar>'
                f"<span>{caption}</span>"
                f'<a href="{html.escape(r["live_view_url"])}" target=_blank rel=noopener>Open full size</a></div>'
                f'<iframe src="{html.escape(r["live_view_url"])}" title="Agent browser live view"'
                ' allow="clipboard-read; clipboard-write"></iframe></div>'
            )
        elif not is_question:
            live = (
                '<div class=none style=margin-top:22px>This request did not attach a live view.'
                " Clear the wall wherever the agent is working, then press the button below.</div>"
            )

    if is_question:
        action = (
            "<h2>Your answer goes straight back to the agent</h2>"
            f'<textarea id=answer placeholder="Type what the agent should do..." '
            f'{"disabled" if status != "pending" else "autofocus"}></textarea>'
            "<div class=row style=margin-top:16px>"
            f'<button class=cta id=resolve {"disabled" if status != "pending" else ""}>'
            f"<span>Send answer</span><span class=chip>{ARROW}</span></button>"
            "</div>"
        )
    else:
        action = (
            "<h2>When the wall is gone, hand control back</h2>"
            "<p class=lede style=font-size:15.5px;max-width:56ch>Clear it in the live view, then press this."
            " The agent's blocked call returns immediately and the run continues from where it stopped.</p>"
            "<div class=row>"
            f'<button class=cta id=resolve {"disabled" if status != "pending" else ""}>'
            f"<span>I cleared it</span><span class=chip>{ARROW}</span></button>"
            "</div>"
        )

    if status == "resolved":
        answer_block = ""
        if r.get("answer"):
            answer_block = (
                '<p style="margin:12px 0 0"><span style=color:var(--muted)>You answered</span> '
                f"<b>{html.escape(r['answer'])}</b></p>"
            )
        resumed = r.get("resume_posted")
        resume_line = ""
        if r.get("has_resume"):
            resume_line = (
                '<p style="margin:8px 0 0;color:var(--muted);font-size:14px">'
                + ("Resume signal delivered to the agent's browser sandbox."
                   if resumed else "Resume signal could not be delivered; the agent's poll still returned.")
                + "</p>"
            )
        action = (
            "<h2>Done</h2>"
            "<p class=reason style=margin:0;color:var(--green)>The agent is running again.</p>"
            + answer_block + resume_line
        )
    elif status == "expired":
        action = (
            "<h2>Too late</h2>"
            "<p class=reason style=margin:0>The agent gave up waiting after "
            f"{int(r['timeout_s'])} seconds.</p>"
            '<p style="margin:8px 0 0;color:var(--muted);font-size:14px">Nothing you do here reaches it now.'
            " The next page will arrive the moment another run stops.</p>"
        )

    waiting_label = "Waiting" if status == "pending" else "Waited"
    timeout_label = "Gives up after" if status == "pending" else "Gave up after"

    body = f"""
{_brand()}
<div class="between r" style=margin-top:30px>
  <h1 style=max-width:24ch>{html.escape(prompt_text or "An agent is blocked.")}</h1>
  {_state_pill(status)}
</div>
<p class="lede r" style=animation-delay:.07s;margin-top:16px>{subline}</p>
<dl class="facts r" style=animation-delay:.11s>
  <div><dt>Asked for</dt><dd>{"an answer" if is_question else "a wall to be cleared"}</dd></div>
  <div><dt>{waiting_label}</dt><dd id=age>{int(r["age_s"])}s</dd></div>
  <div><dt>{timeout_label}</dt><dd>{int(r["timeout_s"])}s</dd></div>
  <div><dt>Request</dt><dd><code>{rid}</code></dd></div>
</dl>
{live}
<div class="card r" id=action style=animation-delay:.2s;margin-top:22px>{action}</div>
<p class=foot>Handoff pages a real person when an agent hits a wall. <a href="/">What this is</a>. Part of <a href="https://omegas.dev" target=_blank rel="noopener noreferrer">Omegas</a>.</p>
"""

    script = f"""<script>
const ID={json.dumps(r["id"])}, T0={r["created_at"]}, STATUS={json.dumps(status)};
const age=document.getElementById('age');
if(STATUS==='pending'){{
  setInterval(()=>{{ if(age) age.textContent=Math.round(Date.now()/1000-T0)+'s'; }},1000);
  // If the agent times out or someone else resolves it, reflect that without a manual reload.
  (async function watch(){{
    while(true){{
      try{{
        const res=await fetch('/v1/requests/'+ID+'?wait=25');
        const j=await res.json();
        if(j.status!=='pending'){{ location.reload(); return; }}
      }}catch(e){{ await new Promise(r=>setTimeout(r,2000)); }}
    }}
  }})();
}}
const btn=document.getElementById('resolve');
if(btn) btn.addEventListener('click',async()=>{{
  const ta=document.getElementById('answer');
  btn.disabled=true; btn.querySelector('span').textContent='Handing back...';
  try{{
    await fetch('/v1/requests/'+ID+'/resolve',{{method:'POST',
      headers:{{'content-type':'application/json'}},
      body:JSON.stringify({{answer: ta?ta.value:null, cleared:true, by:'human'}})}});
    location.reload();
  }}catch(e){{
    btn.disabled=false; btn.querySelector('span').textContent='Retry';
  }}
}});
</script>"""

    heading = "A human is needed" if status == "pending" else (
        "Handed back to the agent" if status == "resolved" else "This request timed out"
    )
    return _shell(f"Handoff — {heading}", body + script)


# ---------------------------------------------------------------------- landing


GITHUB = "https://github.com/NoureddinBakir/handoff"

# app/main.py owns this flag and 404s /demo when it is off. The landing reads the
# same switch so its primary CTA never points at a route that is not there.
DEMO_ENABLED = os.environ.get("DEMO_ENABLED", "").lower() in ("1", "true", "yes", "on")

# Runs before the body parses, so nothing is ever hidden unless this landed.
REVEAL_GATE = (
    "<script>try{if(!matchMedia('(prefers-reduced-motion: reduce)').matches"
    "&&'IntersectionObserver' in window)"
    "document.documentElement.classList.add('jsrv')}catch(e){}</script>"
)

# The record of the run this whole thing exists for. Four states, no invented
# timestamps: the page claims only what the server actually does.
RUN_RECORD = [
    ("STOPPED", "A verification checkbox refused the agent's click",
     "The agent held its run open instead of guessing its way past"),
    ("CALLED", "A phone rang and read out what was blocking the run",
     "The agent asked for a person; it never named a channel"),
    ("CLEARED", "A person ticked the box inside the agent's own browser",
     "Not a reply. They acted inside the session the agent was stuck in"),
    ("FINISHED", "The blocked call returned and the run carried on",
     "The number came from behind the wall, after clearance"),
]

CHANNELS = [
    ("Voice, with the browser", "Built and running", True,
     "The phone rings, the person hears what stopped the run, and they get the agent's own "
     "session to act in. <b>The one channel where a person can act instead of reply.</b>"),
    ("SMS, Slack, email, calendar", "Designed, not built", False,
     "The same request object reaching the same person a different way. None of these work "
     "today. When they do, nothing in the SDK above changes."),
]


def render_landing(recent: list[dict]) -> str:
    if recent:
        rows = "".join(
            f"<tr><td>{_state_pill(r['status'])}</td>"
            f"<td>{html.escape((r['question'] or r['reason'] or '')[:70])}</td>"
            f"<td>{html.escape(r['agent'])}</td>"
            f'<td class=lp-open><a href="/r/{html.escape(r["id"])}">Open</a></td></tr>'
            for r in recent
        )
        queue = (
            '<div class="lp-panel rv"><table><thead><tr><th>State</th>'
            "<th>What stopped the run</th><th>Agent</th><th></th></tr></thead>"
            f"<tbody>{rows}</tbody></table></div>"
        )
    else:
        queue = '<div class="none rv">No agent has asked for a person yet.</div>'

    if DEMO_ENABLED:
        cta_href, cta_label = "/demo", "See it run"
        nav_link = '<a class=lp-navlink href="/demo">See it run</a>'
    elif recent:
        cta_href = "/r/" + html.escape(recent[0]["id"])
        cta_label = "Open the last handoff"
        nav_link = f'<a class=lp-navlink href="{GITHUB}" target=_blank rel="noopener noreferrer">Source</a>'
    else:
        cta_href, cta_label = GITHUB, "Read the code"
        nav_link = f'<a class=lp-navlink href="{GITHUB}" target=_blank rel="noopener noreferrer">Source</a>'
    cta = (
        f'<a class=cta href="{cta_href}"><span>{cta_label}</span>'
        f"<span class=chip>{ARROW}</span></a>"
    )

    waiting = next((r for r in recent if r["status"] == "pending"), None)
    live_line = (
        f'<a class=lp-quiet href="/r/{html.escape(waiting["id"])}">A run is waiting on a person '
        "right now.</a>"
        if waiting
        else (
            '<a class=lp-quiet href="' + GITHUB + '" target=_blank rel="noopener noreferrer">'
            "Read the code</a>"
            if cta_href != GITHUB
            else ""
        )
    )

    record = "".join(
        f'<div class="lp-row bf bf{i + 2}"><span class=lp-state>{state}</span>'
        f"<span><span class=lp-main>{main}</span><span class=lp-det>{det}</span></span></div>"
        for i, (state, main, det) in enumerate(RUN_RECORD)
    )

    channels = "".join(
        f'<div class="lp-chan-row rv"><div><div class=lp-chan-name>{name}</div>'
        f'<div class="lp-chan-state{" on" if built else ""}">{state}</div></div>'
        f"<p class=lp-chan-note>{note}</p></div>"
        for name, state, built, note in CHANNELS
    )

    sample = (
        "<b>import human</b>\n\n"
        "<i># The agent cannot tick a verification checkbox. It says so and waits.</i>\n"
        "cleared = human.<b>clear_wall</b>(\n"
        '    reason="A verification checkbox is blocking checkout",\n'
        "    live_view_url=browser.live_url,   <i># the session to hand over</i>\n"
        "    resume_url=browser.resume_url,    <i># told when the wall is gone</i>\n"
        "    timeout_s=600,\n"
        ")\n\n"
        "<i># Or ask for a judgement and wait for the answer.</i>\n"
        'address = human.<b>ask</b>("Which address should I ship to?")'
    )

    body = f"""
<header class=lp-nav>
  {_brand()}
  {nav_link}
</header>

<section class="lp-band lp-band-hero">
  <div class=lp-in>
    <h1 class="lp-h1 rv">The communication layer between agents
      <span class=tt>and the people they depend on.</span></h1>
    <p class="lp-lede rv">Agents run around the clock. The people accountable for their work do not.
    Handoff is the one call an agent makes when it needs someone: it reaches a real person on
    whatever channel fits, and holds the run open until they answer.</p>
    <p class="lp-copy rv">An agent stopped at a verification checkbox tonight, rang a phone, handed
    over its own browser, and finished the run the moment a person ticked the box.</p>
    <div class="lp-actions rv">
      {cta}
      {live_line}
    </div>
  </div>
</section>

<section class="lp-band lp-band-stone">
  <div class="lp-in lp-two">
    <div>
      <h2 class="lp-h2 rv">The agent stopped. <span class=tt>A person finished it.</span></h2>
      <p class="lp-copy rv">One call blocks the run: <code>human.clear_wall(...)</code>. Handoff
      reaches a person, they act inside the agent's own session, and the call returns with the run
      intact. The agent never guessed, and never faked its way past.</p>
      <p class="lp-copy rv">Text approvals in chat already exist. What they leave out is a phone
      that rings and a browser a person can take the wheel of.</p>
    </div>
    <div class="lp-panel rv">
      <div class=lp-panel-h>Run record<span>handoff.omegas.dev</span></div>
      {record}
    </div>
  </div>
</section>

<section class=lp-band>
  <div class=lp-in>
    <h2 class="lp-h2 rv">One request. <span class=tt>Any way of reaching someone.</span></h2>
    <p class="lp-lede rv">An agent should not hardcode a channel or own the waiting. It says a
    person is needed and how urgent that is. The layer decides whether that becomes a call, a
    message, or an invite, and who is even awake to take it.</p>
    <div class=lp-chan style=margin-top:38px>
      {channels}
    </div>
  </div>
</section>

<section class="lp-band lp-band-stone">
  <div class=lp-in>
    <h2 class="lp-h2 rv">A request belongs to <span class=tt>a person, not a run.</span></h2>
    <p class="lp-lede rv">Requests land in a person's queue rather than firing once per blocked
    run, so one human answer can settle several waiting agents. Every handoff this server has been
    asked for is below.</p>
    <div style=margin-top:36px>{queue}</div>
  </div>
</section>

<section class=lp-band>
  <div class="lp-in lp-two">
    <div>
      <h2 class="lp-h2 rv">The whole SDK <span class=tt>is two calls.</span></h2>
      <p class="lp-copy rv">One file, standard library only. <code>clear_wall</code> blocks until a
      person clears the way; <code>ask</code> blocks until someone answers. A timeout raises, or
      returns the default you passed, so a run never hangs forever.</p>
      <p class="lp-copy rv">Nothing in these signatures names a channel. That is the point.</p>
    </div>
    <pre class=rv>{sample}</pre>
  </div>
</section>

<section class="lp-band lp-band-stone lp-center">
  <div class=lp-in-narrow>
    <h2 class="lp-h2 rv">The wall is still there. <span class=tt>Someone has to clear it.</span></h2>
    <p class="lp-lede rv">Handoff is how the agent reaches that someone, and how the run stays
    alive while they are on their way.</p>
    <div class="lp-actions rv">
      {cta}
    </div>
  </div>
</section>

<footer class=lp-foot>
  <span>Handoff is part of
  <a href="https://omegas.dev" target=_blank rel="noopener noreferrer">Omegas</a>.</span>
  <span>Built at Night Hack, 2026-07-24. MIT.
  <a href="{GITHUB}" target=_blank rel="noopener noreferrer">Source</a>.
  Requests live in one process on purpose.</span>
</footer>

<script>
(function(){{
  var els=document.querySelectorAll('.rv');
  var still=window.matchMedia('(prefers-reduced-motion: reduce)').matches
    || !('IntersectionObserver' in window);
  if(still){{ els.forEach(function(el){{ el.classList.add('in'); }}); return; }}
  // Fire once per element, then stop watching it. Nothing runs after the page settles.
  var io=new IntersectionObserver(function(entries){{
    entries.forEach(function(en){{
      if(!en.isIntersecting) return;
      en.target.classList.add('in');
      io.unobserve(en.target);
    }});
  }},{{threshold:.2}});
  els.forEach(function(el){{ io.observe(el); }});
}})();
</script>
"""
    return _shell(
        "Handoff — the communication layer between agents and people",
        body,
        head=f"<style>{LANDING_CSS}</style>{REVEAL_GATE}",
        full=True,
    )
