"""Intrinsic-backed subset of email.message for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): extend EmailMessage to full CPython policy/headerregistry semantics via Rust intrinsics.
_molt_email_message_new = _require_intrinsic("molt_email_message_new", globals())
_molt_email_message_set = _require_intrinsic("molt_email_message_set", globals())
_molt_email_message_items = _require_intrinsic("molt_email_message_items", globals())
_molt_email_message_drop = _require_intrinsic("molt_email_message_drop", globals())


class EmailMessage:
    def __init__(self, *, policy: Any | None = None) -> None:
        self.policy = policy
        self._handle = _molt_email_message_new()

    def __setitem__(self, name: str, value: Any) -> None:
        _molt_email_message_set(self._handle, str(name), str(value))

    def items(self) -> list[tuple[str, str]]:
        raw = _molt_email_message_items(self._handle)
        if not isinstance(raw, list):
            return []
        out: list[tuple[str, str]] = []
        for item in raw:
            if not isinstance(item, tuple) or len(item) != 2:
                continue
            out.append((str(item[0]), str(item[1])))
        return out

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _molt_email_message_drop(handle)
        except Exception:
            pass


__all__ = ["EmailMessage"]
