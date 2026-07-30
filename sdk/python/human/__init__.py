"""Deprecated alias for the :mod:`handoff` module.

`handoff-human` 0.1.x exposed its module as `human`. From 0.2.0 the module is `handoff`; the
distribution name on PyPI is unchanged. This shim forwards every public name so existing code keeps
working, and warns once on import.

**It is removed in 0.3.0.** Change `import human` to `import handoff`.
"""

from __future__ import annotations

import warnings

from handoff import (  # noqa: F401
    DEFAULT_BASE_URL,
    Handoff,
    HandoffError,
    HandoffTimeout,
    __version__,
    ask,
    clear_wall,
    configure,
    create_request,
)

__all__ = [
    "configure",
    "ask",
    "clear_wall",
    "create_request",
    "Handoff",
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
