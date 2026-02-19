"""Conversion functions between RGB and other color systems."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "rgb_to_yiq",
    "yiq_to_rgb",
    "rgb_to_hls",
    "hls_to_rgb",
    "rgb_to_hsv",
    "hsv_to_rgb",
]

_MOLT_RGB_TO_YIQ = _require_intrinsic("molt_colorsys_rgb_to_yiq", globals())
_MOLT_YIQ_TO_RGB = _require_intrinsic("molt_colorsys_yiq_to_rgb", globals())
_MOLT_RGB_TO_HLS = _require_intrinsic("molt_colorsys_rgb_to_hls", globals())
_MOLT_HLS_TO_RGB = _require_intrinsic("molt_colorsys_hls_to_rgb", globals())
_MOLT_RGB_TO_HSV = _require_intrinsic("molt_colorsys_rgb_to_hsv", globals())
_MOLT_HSV_TO_RGB = _require_intrinsic("molt_colorsys_hsv_to_rgb", globals())
_MOLT_V = _require_intrinsic("molt_colorsys_v", globals())

# Some floating-point constants
ONE_THIRD = 1.0 / 3.0
ONE_SIXTH = 1.0 / 6.0
TWO_THIRD = 2.0 / 3.0


def rgb_to_yiq(r, g, b):
    return _MOLT_RGB_TO_YIQ(r, g, b)


def yiq_to_rgb(y, i, q):
    return _MOLT_YIQ_TO_RGB(y, i, q)


def rgb_to_hls(r, g, b):
    return _MOLT_RGB_TO_HLS(r, g, b)


def hls_to_rgb(h, l, s):
    return _MOLT_HLS_TO_RGB(h, l, s)


def rgb_to_hsv(r, g, b):
    return _MOLT_RGB_TO_HSV(r, g, b)


def hsv_to_rgb(h, s, v):
    return _MOLT_HSV_TO_RGB(h, s, v)


def _v(m1, m2, hue):
    return _MOLT_V(m1, m2, hue)
