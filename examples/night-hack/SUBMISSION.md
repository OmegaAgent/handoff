# Night Hack III — submission answers (copy-paste)

**First name(s):** Noureddin

**Team name:** Omegas.dev

**Email:** noureddin@omegas.dev

**1-liner (50 char max):**
Handoff: your agent calls you when it's stuck
(45 chars)

**Short project description:**
Handoff is an open-source "await human()" API for AI agents. When an agent hits a wall (captcha, 2FA, login) it calls handoff.ask(): a real phone call goes out to a human, an AI voice says exactly what's blocked, the human opens a live view of the agent's own browser, clears the wall or types an answer, and the blocked call returns so the agent finishes the job. One-file Python SDK + hosted API, MIT. Existed before kickoff: our product Omega's internal sandbox/live-view plumbing and personal API accounts. Everything else (the SDK, API, handoff pages, phone paging, demo) was built tonight. Built with Claude via AWS Bedrock, Retell AI, and Fly.io.

**What we did in 5 hours (3-5 points):**
1. Built the Handoff tool for my Omega agent: an open-source await human() API, so when the agent hits a captcha/login/2FA wall it hands off to a real person instead of failing.
2. E2E flow for agent <-> human call: the agent gets stuck, my phone actually rings with an AI voice saying what's blocked, I open a live view of the agent's own browser, clear the wall or type an answer, and the agent picks right back up. Rang my phone from prod tonight.
3. Extended and optimized the Pause feature so work reacts to human intervention: a blocked agent resumes about 3 seconds after the human resolves, and "I cleared it" wires up the sandbox's resume endpoint that existed but nothing had ever called.
4. Shipped it into my production product too: Omega on omegas.dev now phones me when its browser agent parks on a wall.

**Demo video:** (record via Loom/phone — 4 beats are in ~/human/RUNBOOK.md, keep under 60s)

**Demo Link (no auth):** https://handoff-human.fly.dev/try
(303s straight into a live handoff page; landing is https://handoff-human.fly.dev)

**Live project/Website URL:** https://handoff-human.fly.dev

**Team photo:** (yours — phone selfie is fine)

**Github repo:** https://github.com/OmegaAgent/handoff
