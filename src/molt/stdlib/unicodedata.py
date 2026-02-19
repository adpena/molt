"""Minimal `unicodedata` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_UNICODEDATA_RUNTIME_READY = _require_intrinsic(
    "molt_unicodedata_runtime_ready", globals()
)

_COMPOSE_MAP = {
    "a\u0301": "\u00e1",
    "A\u0301": "\u00c1",
    "e\u0301": "\u00e9",
    "E\u0301": "\u00c9",
    "i\u0301": "\u00ed",
    "I\u0301": "\u00cd",
    "o\u0301": "\u00f3",
    "O\u0301": "\u00d3",
    "u\u0301": "\u00fa",
    "U\u0301": "\u00da",
    "n\u0303": "\u00f1",
    "N\u0303": "\u00d1",
}
_DECOMPOSE_MAP = {value: key for key, value in _COMPOSE_MAP.items()}


def _compose(text: str) -> str:
    out = text
    for decomposed, composed in _COMPOSE_MAP.items():
        out = out.replace(decomposed, composed)
    return out


def _decompose(text: str) -> str:
    out = text
    for composed, decomposed in _DECOMPOSE_MAP.items():
        out = out.replace(composed, decomposed)
    return out


def normalize(form: str, unistr: str) -> str:
    _MOLT_UNICODEDATA_RUNTIME_READY()
    if not isinstance(form, str) or not isinstance(unistr, str):
        raise TypeError("normalize() arguments must be str")
    form_upper = form.upper()
    if form_upper == "NFC":
        return _compose(unistr)
    if form_upper == "NFD":
        return _decompose(unistr)
    raise ValueError("invalid normalization form")


class UCD:
    def normalize(self, form: str, unistr: str) -> str:
        return normalize(form, unistr)

    def name(self, unichr: str, default=None):
        del unichr
        return default

    def lookup(self, name: str):
        raise KeyError(name)

    def category(self, unichr: str) -> str:
        del unichr
        return "Cn"

    def bidirectional(self, unichr: str) -> str:
        del unichr
        return ""

    def combining(self, unichr: str) -> int:
        del unichr
        return 0

    def east_asian_width(self, unichr: str) -> str:
        del unichr
        return "N"

    def mirrored(self, unichr: str) -> int:
        del unichr
        return 0

    def decomposition(self, unichr: str) -> str:
        del unichr
        return ""

    def numeric(self, unichr: str, default=None):
        del unichr
        return default

    def decimal(self, unichr: str, default=None):
        del unichr
        return default

    def digit(self, unichr: str, default=None):
        del unichr
        return default


ucd_3_2_0 = UCD()


__all__ = ["normalize", "ucd_3_2_0"]
