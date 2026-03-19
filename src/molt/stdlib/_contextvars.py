"""Intrinsic-backed `_contextvars` compatibility surface."""

from _intrinsics import require_intrinsic as _require_intrinsic
from contextvars import Context
from contextvars import ContextVar
from contextvars import Token
from contextvars import copy_context

_MOLT_CANCEL_TOKEN_GET_CURRENT = _require_intrinsic("molt_cancel_token_get_current")

__all__ = ["Context", "ContextVar", "Token", "copy_context"]
