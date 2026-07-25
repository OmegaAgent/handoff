# Paging UX — the escalation ladder (owner-approved direction, 2026-07-24)

Owner's brief: "I don't want the agent calling me at 3am — but if I'm up, just call me. A text is
fine: 'Hey, I have a few things I want to clear up with you — can I call you now, or tomorrow?'"
Tie into workflow automations.

**NOT for tonight's demo** (the straight call IS the stage theater). This is the v1 product design
immediately after — and worth one sentence in the submission's "potential" story.

## Principles
1. Agent declares **urgency**, never the channel: `blocking-expiring` (OTP wall dying in minutes) |
   `blocking` (parked but stable) | `can-wait`. Reaching the human is Handoff's policy, not the
   agent's choice.
2. **Quiet hours are the only hard rail** (default 22:00–08:00 local: never call). Everything else
   is model judgment + presence signals (dashboard/Slack activity in last ~15 min ⇒ awake ⇒ calling
   is fine) — consistent with the owner's "governance in intelligence, not blocking code" rule.
3. **Text-first consent**: first touch is a message in the owner's wording — "I've got a few things
   to clear up — call now, or tomorrow?" Reply "call" → Retell rings within seconds. "Tomorrow" or
   no reply → queue into the **morning digest**: one scheduled call clears the whole queue (batching:
   "a few things", not three interruptions).
4. Exception: `blocking-expiring` during waking hours may skip consent (a 2-min OTP window doesn't
   survive a round-trip). During quiet hours even that fails gracefully to the digest.
5. **Workflow tie-in**: scheduled/overnight automations default `can-wait`+digest; interactive runs
   default immediate. Remember consent answers; learn hours/channel prefs over time.

## Channels & costs
- Tonight's free text channel = Slack DM (bot token in ~/hipocampus/.env; push still buzzes the phone).
- Real SMS: Retell add-on is $20/mo — post-hack decision; or any SMS provider later.
- Call = existing proven Retell recipe (update begin_message → create-phone-call).
