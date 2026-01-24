"""Capability-gated contextvars shim for Molt."""

from __future__ import annotations

from typing import Any, Callable

from molt.concurrency import current_token

__all__ = ["Context", "ContextVar", "Token", "copy_context"]

_MISSING = object()
_CONTEXTS: dict[int, dict["_ContextVar", Any]] = {}


def _current_context_id() -> int:
    return current_token().token_id()


def _current_context() -> dict["_ContextVar", Any]:
    return _CONTEXTS.setdefault(_current_context_id(), {})


def _set_context_for_token(token_id: int, ctx: "_Context") -> None:
    _CONTEXTS[token_id] = dict(ctx._data)


def _clear_context_for_token(token_id: int) -> None:
    _CONTEXTS.pop(token_id, None)


class _Token:
    def __init__(
        self,
        var: "_ContextVar",
        old_value: Any,
        had_value: bool,
        context_id: int,
    ) -> None:
        self._var = var
        self._old_value = old_value
        self._had_value = had_value
        self._context_id = context_id
        self._used = False

    def __repr__(self) -> str:
        status = "used" if self._used else "active"
        return f"<Token var={self._var.name!r} {status}>"


class _ContextVar:
    def __init__(self, name: str, *, default: Any = _MISSING) -> None:
        if not isinstance(name, str):
            raise TypeError("ContextVar name must be a str")
        self.name = name
        self._default = default

    def get(self, default: Any = _MISSING) -> Any:
        ctx = _CONTEXTS.get(_current_context_id(), {})
        if self in ctx:
            return ctx[self]
        if default is not _MISSING:
            return default
        if self._default is not _MISSING:
            return self._default
        raise LookupError(self.name)

    def set(self, value: Any) -> "_Token":
        ctx = _current_context()
        had_value = self in ctx
        old_value = ctx.get(self, _MISSING)
        ctx[self] = value
        return _Token(self, old_value, had_value, _current_context_id())

    def reset(self, token: "_Token") -> None:
        if token._var is not self:
            raise ValueError("Token was created by a different ContextVar")
        if token._used:
            raise RuntimeError("Token has already been used once")
        if token._context_id != _current_context_id():
            raise ValueError("Token was created in a different Context")
        ctx = _current_context()
        if token._had_value:
            ctx[self] = token._old_value
        else:
            ctx.pop(self, None)
        token._used = True

    def __repr__(self) -> str:
        return f"<ContextVar name={self.name!r}>"


class _Context:
    def __init__(self, data: dict[_ContextVar, Any] | None = None) -> None:
        self._data = dict(data or {})

    def get(self, var: _ContextVar, default: Any = _MISSING) -> Any:
        data = self._data
        if var in data:
            return data[var]
        if default is not _MISSING:
            return default
        if var._default is not _MISSING:
            return var._default
        raise LookupError(var.name)

    def run(self, func: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Any:
        ctx_id = _current_context_id()
        prior = _CONTEXTS.get(ctx_id)
        _CONTEXTS[ctx_id] = dict(self._data)
        try:
            return func(*args, **kwargs)
        finally:
            if prior is None:
                _CONTEXTS.pop(ctx_id, None)
            else:
                _CONTEXTS[ctx_id] = prior

    def copy(self) -> "_Context":
        return _Context(self._data)


def copy_context() -> "_Context":
    return _Context(_CONTEXTS.get(_current_context_id(), {}))


Token = _Token
ContextVar = _ContextVar
Context = _Context
