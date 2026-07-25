"""Server-rendered HTML for Handoff. No build step, no framework, no CDN."""

from __future__ import annotations

import html
import json

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
}
.wrap{max-width:920px;margin:0 auto;padding:32px 22px 80px}
a{color:var(--green);text-decoration:none}
a:hover{text-decoration:underline}
.mark{display:flex;align-items:center;gap:9px;font-size:19px;font-weight:700;letter-spacing:-.01em}
.mark svg{width:.78em;height:.76em;color:var(--green)}
h1{font-size:34px;line-height:1.14;letter-spacing:-.02em;margin:26px 0 10px;font-weight:700}
h2{font-size:15px;letter-spacing:.02em;text-transform:none;color:var(--muted);font-weight:600;margin:0 0 10px}
p{margin:0 0 14px}
.lede{color:var(--muted);font-size:17px;max-width:62ch}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:22px}
.card+.card{margin-top:18px}
.row{display:flex;gap:14px;align-items:center;flex-wrap:wrap}
.between{display:flex;gap:16px;align-items:flex-start;justify-content:space-between;flex-wrap:wrap}
.meta{display:grid;grid-template-columns:auto 1fr;gap:6px 18px;font-size:14px;margin:0}
.meta dt{color:var(--muted)}
.meta dd{margin:0;font-variant-numeric:tabular-nums}
.reason{font-size:21px;line-height:1.35;font-weight:600;margin:0 0 4px;letter-spacing:-.01em}
.state{display:inline-flex;align-items:center;gap:7px;font-size:13px;font-weight:600;
  padding:5px 11px;border-radius:999px;border:1px solid var(--line);background:var(--stone);color:var(--muted)}
.state.pending{color:var(--amber);border-color:#E6D6BC;background:#FBF5EA}
.state.resolved{color:var(--green);border-color:#CFE0D2;background:#EEF4EF}
.state.expired{color:#8C4A3F;border-color:#E8D2CD;background:#F9EFED}
.dot{width:7px;height:7px;border-radius:50%;background:currentColor}
.state.pending .dot{animation:pulse 1.4s ease-in-out infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.28}}
.cta{display:inline-flex;align-items:center;gap:12px;background:var(--green);color:#fff;border:0;
  font:700 17px/1 inherit;padding:13px 15px 13px 22px;border-radius:999px;cursor:pointer;
  box-shadow:0 1px 2px rgba(42,33,24,.16),0 8px 22px rgba(42,33,24,.13);transition:background .16s,transform .16s}
.cta .chip{width:32px;height:32px;border-radius:50%;background:#fff;color:var(--green);
  display:grid;place-items:center;transition:transform .16s}
.cta:hover{background:var(--green-deep);transform:translateY(-2px)}
.cta:hover .chip{transform:translateX(3px)}
.cta:disabled{background:var(--muted);cursor:default;box-shadow:none;transform:none}
.ghost{background:none;border:1px solid var(--line);color:var(--ink);border-radius:8px;
  font:600 14px/1 inherit;padding:11px 14px;cursor:pointer}
.ghost:hover{border-color:var(--muted)}
textarea{width:100%;min-height:104px;border:1px solid var(--line);border-radius:10px;padding:12px 13px;
  font:16px/1.5 inherit;color:var(--ink);background:var(--canvas);resize:vertical}
textarea:focus{outline:2px solid var(--green);outline-offset:1px;border-color:var(--green)}
.frame{margin-top:16px;border:1px solid var(--line);border-radius:12px;overflow:hidden;background:var(--scrim)}
.frame iframe{display:block;width:100%;height:520px;border:0;background:#fff}
.frame .bar{display:flex;align-items:center;justify-content:space-between;gap:10px;
  padding:9px 13px;background:var(--stone);border-bottom:1px solid var(--line);font-size:13px;color:var(--muted)}
.none{border:1px dashed var(--line);border-radius:12px;padding:26px;text-align:center;color:var(--muted);font-size:14px}
code,kbd{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.92em}
pre{background:var(--scrim);color:#F4F1EA;border-radius:12px;padding:18px;overflow-x:auto;font-size:13.5px;line-height:1.6}
pre b{color:#9CC7A3;font-weight:600}
table{width:100%;border-collapse:collapse;font-size:14px}
th,td{text-align:left;padding:9px 12px;border-bottom:1px solid var(--line)}
th{color:var(--muted);font-weight:600;font-size:13px}
tbody tr:last-child td{border-bottom:0}
.foot{margin-top:34px;color:var(--muted);font-size:13.5px}
.hr{height:1px;background:var(--line);margin:30px 0}
@media (max-width:640px){h1{font-size:27px}.frame iframe{height:60vh}}
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


def _shell(title: str, body: str, head: str = "") -> str:
    return (
        "<!doctype html><html lang=en><head><meta charset=utf-8>"
        '<meta name=viewport content="width=device-width,initial-scale=1">'
        f"<title>{html.escape(title)}</title><style>{CSS}</style>{head}"
        f"</head><body><div class=wrap>{body}</div></body></html>"
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
    heading = "A human is needed" if status == "pending" else (
        "Handed back to the agent" if status == "resolved" else "This request timed out"
    )

    paged = r.get("paged") or ""
    if paged == "ringing":
        page_note = "Your phone was called."
    elif paged.startswith("skipped"):
        page_note = "Paging skipped for this request."
    elif paged.startswith("failed"):
        page_note = "Phone paging failed, so you are reading this from the link instead."
    else:
        page_note = "Placing the call."

    # Live view of the agent's own browser. The viewer can drive it, not just watch.
    if r.get("live_view_url"):
        live = (
            '<div class=frame><div class=bar><span>Live view of the agent\'s browser'
            " &middot; your clicks and keystrokes are relayed</span>"
            f'<a href="{html.escape(r["live_view_url"])}" target=_blank rel=noopener>Open full size</a></div>'
            f'<iframe src="{html.escape(r["live_view_url"])}" title="Agent browser live view"'
            ' allow="clipboard-read; clipboard-write"></iframe></div>'
        )
    else:
        live = (
            "<div class=none style=margin-top:16px>This request did not attach a live view."
            " Answer from the context above.</div>"
        )

    if is_question:
        action = (
            "<h2>Your answer goes straight back to the agent</h2>"
            f'<textarea id=answer placeholder="Type what the agent should do..." '
            f'{"disabled" if status != "pending" else "autofocus"}></textarea>'
            "<div class=row style=margin-top:14px>"
            f'<button class=cta id=resolve {"disabled" if status != "pending" else ""}>'
            f"<span>Send answer</span><span class=chip>{ARROW}</span></button>"
            "</div>"
        )
    else:
        action = (
            "<h2>When the wall is gone, hand control back</h2>"
            "<p class=lede style=font-size:15px>Clear it in the live view above, then press this."
            " The agent's blocked call returns immediately and it keeps going from where it stopped.</p>"
            "<div class=row>"
            f'<button class=cta id=resolve {"disabled" if status != "pending" else ""}>'
            f"<span>I cleared it</span><span class=chip>{ARROW}</span></button>"
            "</div>"
        )

    if status == "resolved":
        answer_block = ""
        if r.get("answer"):
            answer_block = f"<p style=margin-top:10px>You answered: <b>{html.escape(r['answer'])}</b></p>"
        resumed = r.get("resume_posted")
        resume_line = ""
        if r.get("has_resume"):
            resume_line = (
                "<p style=margin-top:6px;color:var(--muted);font-size:14px>"
                + ("Resume signal delivered to the agent's browser sandbox."
                   if resumed else "Resume signal could not be delivered; the agent's poll still returned.")
                + "</p>"
            )
        action = (
            "<h2>Done</h2><p class=reason style=font-size:18px>The agent is running again.</p>"
            + answer_block + resume_line
        )
    elif status == "expired":
        action = (
            "<h2>Too late</h2><p class=lede>The agent gave up waiting after "
            f"{int(r['timeout_s'])} seconds and took its own fallback path.</p>"
        )

    body = f"""
{_brand()}
<div class=between style=margin-top:26px>
  <h1 style=margin:0>{heading}</h1>
  {_state_pill(status)}
</div>
<div class=card style=margin-top:22px>
  <p class=reason>{html.escape(prompt_text or "An agent is blocked.")}</p>
  <p style=color:var(--muted);font-size:14px;margin-bottom:18px>{html.escape(page_note)}</p>
  <dl class=meta>
    <dt>Agent</dt><dd>{html.escape(r["agent"])}</dd>
    <dt>Asked for</dt><dd>{"an answer" if is_question else "a wall to be cleared"}</dd>
    <dt>Waiting</dt><dd id=age>{r["age_s"]}s</dd>
    <dt>Gives up after</dt><dd>{int(r["timeout_s"])}s</dd>
    <dt>Request</dt><dd><code>{rid}</code></dd>
  </dl>
  {live}
</div>
<div class=card id=action>{action}</div>
<p class=foot>Handoff pages a real person when an agent hits a wall. <a href="/">What this is</a></p>
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

    return _shell(f"Handoff — {heading}", body + script)


# ---------------------------------------------------------------------- landing


def render_landing(recent: list[dict]) -> str:
    if recent:
        rows = "".join(
            f"<tr><td>{_state_pill(r['status'])}</td>"
            f"<td>{html.escape((r['reason'] or '')[:70])}</td>"
            f"<td>{html.escape(r['agent'])}</td>"
            f"<td><a href=\"/r/{html.escape(r['id'])}\">open</a></td></tr>"
            for r in recent
        )
        table = f"<table><thead><tr><th>State</th><th>What blocked it</th><th>Agent</th><th></th></tr></thead><tbody>{rows}</tbody></table>"
    else:
        table = "<div class=none>No agent has asked for a human yet.</div>"

    sample = (
        "<b>import human</b>\n\n"
        "# The agent hits a verification checkbox it cannot solve.\n"
        "cleared = human.<b>clear_wall</b>(\n"
        '    reason="A human-verification checkbox is blocking checkout",\n'
        "    live_view_url=browser.live_url,   # your sandbox's screencast\n"
        "    resume_url=browser.resume_url,    # told the moment the wall is gone\n"
        "    timeout_s=600,\n"
        ")\n\n"
        "# Or just ask a question and wait for a typed answer.\n"
        'shipping = human.<b>ask</b>("Which address should I ship to?")'
    )

    body = f"""
{_brand()}
<h1>When an agent hits a wall,<br>it can ask a person.</h1>
<p class=lede>Handoff is an <code>await human()</code> call for AI agents. The agent blocks, a real
phone rings, and whoever answers gets a live view of the agent's own browser. They clear the wall or
type the answer, press one button, and the blocked call returns so the run finishes.</p>
<div class=card style=margin-top:26px>
  <h2>The whole SDK</h2>
  <pre>{sample}</pre>
  <p style=margin:0;color:var(--muted);font-size:14px>Text approvals in chat already exist. The two
  things they leave out are a phone that actually rings and a browser a person can take the wheel of.</p>
</div>
<div class=hr></div>
<h2>Recent handoffs</h2>
{table}
<p class=foot>Built at Night Hack, 2026-07-24. MIT. Requests live in one process on purpose.</p>
"""
    return _shell("Handoff — an await human() API for AI agents", body)
