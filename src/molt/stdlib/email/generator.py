"""Public API surface shim for ``email.generator``."""

from __future__ import annotations

import re

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_EMAIL_MESSAGE_AS_STRING = _require_intrinsic(
    "molt_email_message_as_string"
)

UNDERSCORE = "_"
NL = "\n"
NLCRE = re.compile(r"\r\n|\r|\n")
fcre = re.compile(r"^From ")
NEWLINE_WITHOUT_FWSP = re.compile(r"\n")


class HeaderWriteError(Exception):
    pass


class Generator:
    def __init__(
        self,
        outfp,
        mangle_from_: bool | None = None,
        maxheaderlen: int | None = None,
        *,
        policy=None,
    ):
        self._fp = outfp
        self._mangle_from_ = mangle_from_
        self._maxheaderlen = maxheaderlen
        self.policy = policy

    def flatten(self, msg, unixfrom: bool = False, linesep: str | None = None):
        del unixfrom
        handle = getattr(msg, "_handle", None)
        if handle is not None:
            text = _MOLT_EMAIL_MESSAGE_AS_STRING(handle)
        else:
            text = str(msg)
        if linesep is not None:
            text = NLCRE.sub(linesep, text)
        self._fp.write(text)


class BytesGenerator(Generator):
    def flatten(self, msg, unixfrom: bool = False, linesep: str | None = None):
        del unixfrom
        handle = getattr(msg, "_handle", None)
        if handle is not None:
            text = _MOLT_EMAIL_MESSAGE_AS_STRING(handle)
        else:
            text = str(msg)
        if linesep is not None:
            text = NLCRE.sub(linesep, text)
        if isinstance(text, str):
            text = text.encode("utf-8", "replace")
        self._fp.write(text)


class DecodedGenerator(Generator):
    pass
