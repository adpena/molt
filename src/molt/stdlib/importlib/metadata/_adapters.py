"""Intrinsic-backed adapters for `importlib.metadata` message handling."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORTLIB_IMPORT_REQUIRED = _require_intrinsic(
    "molt_importlib_import_required", globals()
)

email = _MOLT_IMPORTLIB_IMPORT_REQUIRED("email")
_email_message = _MOLT_IMPORTLIB_IMPORT_REQUIRED("email.message")
functools = _MOLT_IMPORTLIB_IMPORT_REQUIRED("functools")
re = _MOLT_IMPORTLIB_IMPORT_REQUIRED("re")
textwrap = _MOLT_IMPORTLIB_IMPORT_REQUIRED("textwrap")
warnings = _MOLT_IMPORTLIB_IMPORT_REQUIRED("warnings")


class Message(_email_message.Message):
    pass
