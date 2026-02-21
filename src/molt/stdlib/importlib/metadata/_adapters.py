"""Intrinsic-backed adapters for `importlib.metadata` message handling."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import email as email
import email.message as _email_message
import functools as functools
import re as re
import textwrap as textwrap
import warnings as warnings

_require_intrinsic("molt_stdlib_probe", globals())


class Message(_email_message.Message):
    pass
