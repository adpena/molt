"""Native Context module projection."""
import sys as _sys
from _intrinsics import require_intrinsic as _require_intrinsic

Context, ContextVar, Token, copy_context = _require_intrinsic("molt_contextvars_types")(
    _sys.modules[__name__]
)
__all__ = ["Context", "ContextVar", "Token", "copy_context"]
del _sys
globals().pop("_require_intrinsic", None)
