"""Color system conversions for Molt (Python 3.12+)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_rgb_to_hsv = _require_intrinsic("molt_colorsys_rgb_to_hsv", globals())
_hsv_to_rgb = _require_intrinsic("molt_colorsys_hsv_to_rgb", globals())
_rgb_to_hls = _require_intrinsic("molt_colorsys_rgb_to_hls", globals())
_hls_to_rgb = _require_intrinsic("molt_colorsys_hls_to_rgb", globals())
_rgb_to_yiq = _require_intrinsic("molt_colorsys_rgb_to_yiq", globals())
_yiq_to_rgb = _require_intrinsic("molt_colorsys_yiq_to_rgb", globals())

__all__ = [
    "rgb_to_hsv",
    "hsv_to_rgb",
    "rgb_to_hls",
    "hls_to_rgb",
    "rgb_to_yiq",
    "yiq_to_rgb",
]


def rgb_to_hsv(r, g, b):
    return _rgb_to_hsv(r, g, b)


def hsv_to_rgb(h, s, v):
    return _hsv_to_rgb(h, s, v)


def rgb_to_hls(r, g, b):
    return _rgb_to_hls(r, g, b)


def hls_to_rgb(h, l, s):
    return _hls_to_rgb(h, l, s)


def rgb_to_yiq(r, g, b):
    return _rgb_to_yiq(r, g, b)


def yiq_to_rgb(y, i, q):
    return _yiq_to_rgb(y, i, q)
