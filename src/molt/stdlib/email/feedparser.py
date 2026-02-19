"""Public API surface shim for ``email.feedparser``."""

from __future__ import annotations

from collections import deque
import re

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_capabilities_has", globals())

NLCRE = re.compile(r"\r\n|\r|\n")
NLCRE_bol = re.compile(r"^(?:\r\n|\r|\n)")
NLCRE_eol = re.compile(r"(?:\r\n|\r|\n)$")
NLCRE_crack = re.compile(r"\r\n|\r|\n")
headerRE = re.compile(r"^[^:\s][^:]*:")

EMPTYSTRING = ""
NL = "\n"
NeedMoreData = object()


class Compat32:
    pass


compat32 = Compat32()
del Compat32


class BufferedSubFile:
    def __init__(self):
        self._lines = deque()

    def push(self, data):
        self._lines.append(data)

    def readline(self):
        if self._lines:
            return self._lines.popleft()
        return EMPTYSTRING


class FeedParser:
    def __init__(self, _factory=None, *, policy=compat32):
        del _factory
        self.policy = policy
        self._chunks: list[str] = []

    def feed(self, data):
        if data:
            self._chunks.append(str(data))

    def close(self):
        return EMPTYSTRING.join(self._chunks)


class BytesFeedParser(FeedParser):
    def feed(self, data):
        if isinstance(data, (bytes, bytearray)):
            data = data.decode("utf-8", "replace")
        super().feed(data)
