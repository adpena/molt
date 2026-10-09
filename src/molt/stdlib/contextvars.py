"""Context variables backed by the shared native runtime owner."""
import _collections_abc
from _contextvars import Context, ContextVar, Token, copy_context

_collections_abc.Mapping.register(Context)
__all__ = ["Context", "ContextVar", "Token", "copy_context"]
