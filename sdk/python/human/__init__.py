"""Deprecated alias for the :mod:`handoff` module.

`handoff-human` 0.1.x exposed its module as `human`. From 0.2.0 the module is `handoff`; the
distribution name on PyPI is unchanged. This shim forwards the public names so existing imports
keep resolving, and warns once on import.

**It is removed in 0.3.0.** Change `import human` to `import handoff`.

Two names from 0.1.x are gone rather than forwarded, because the protocol the SDK now speaks has
no way to express what they did:

``Handoff`` / ``create_request``
    A request is raised with a declaration — the shape of the answer, the capabilities the person
    must be handed, the authority required to give it — rather than with a ``kind`` string. Use
    :func:`handoff.raise_request`, :func:`handoff.ask`, or :func:`handoff.approve`.

``clear_wall``
    It took a live-view URL. The protocol never carries a resolvable address by value, so the old
    signature cannot be honoured. It still imports and raises with the equivalent declaration in
    the message, rather than disappearing without explanation.
"""

from __future__ import annotations

import warnings

from handoff import (  # noqa: F401
    DEFAULT_BASE_URL,
    Client,
    HandoffError,
    HandoffTimeout,
    Outcome,
    PendingRequest,
    Waiter,
    __version__,
    approve,
    ask,
    clear_wall,
    configure,
    raise_request,
    redeem,
    resume,
    waiter,
)

__all__ = [
    "configure",
    "ask",
    "approve",
    "raise_request",
    "redeem",
    "resume",
    "waiter",
    "clear_wall",
    "Client",
    "Waiter",
    "PendingRequest",
    "Outcome",
    "HandoffTimeout",
    "HandoffError",
    "DEFAULT_BASE_URL",
    "__version__",
]

warnings.warn(
    "The `human` module is deprecated and will be removed in handoff-human 0.3.0. "
    "Use `import handoff` instead.",
    DeprecationWarning,
    stacklevel=2,
)
