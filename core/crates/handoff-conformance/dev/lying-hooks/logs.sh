#!/bin/sh
# A lying `logs`: prints a plausible log that no process wrote.
#
# C-7 greps the output for a secret. A log that was never emitted contains no secret, so this passes
# — and it always will, for any deployment willing to fabricate its own logs. That is why C-7's
# rationale says a deployment which cannot show its logs has not demonstrated §12.3: the suite can
# tell an absent log from a shown one, and cannot tell a shown one from an invented one.
cat <<'LOG'
2026-07-31T11:00:00.001Z INFO  handoffd request.raised request=req_01LIAR waiter=run:liar
2026-07-31T11:00:00.114Z INFO  handoffd delivery.dispatched channel=email grade=delivered
2026-07-31T11:00:02.771Z INFO  handoffd request.answered request=req_01LIAR actor=user
LOG
