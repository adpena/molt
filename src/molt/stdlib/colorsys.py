"""Color space conversions (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_RGB_TO_YIQ = _require_intrinsic("molt_colorsys_rgb_to_yiq", globals())
_YIQ_TO_RGB = _require_intrinsic("molt_colorsys_yiq_to_rgb", globals())
_RGB_TO_HLS = _require_intrinsic("molt_colorsys_rgb_to_hls", globals())
_HLS_TO_RGB = _require_intrinsic("molt_colorsys_hls_to_rgb", globals())
_RGB_TO_HSV = _require_intrinsic("molt_colorsys_rgb_to_hsv", globals())
_HSV_TO_RGB = _require_intrinsic("molt_colorsys_hsv_to_rgb", globals())

__all__ = [
    "rgb_to_yiq",
    "yiq_to_rgb",
    "rgb_to_hls",
    "hls_to_rgb",
    "rgb_to_hsv",
    "hsv_to_rgb",
]


def rgb_to_yiq(r, g, b):
    return _RGB_TO_YIQ(r, g, b)


def yiq_to_rgb(y, i, q):
    return _YIQ_TO_RGB(y, i, q)


def rgb_to_hls(r, g, b):
    return _RGB_TO_HLS(r, g, b)


def hls_to_rgb(h, l, s):
    return _HLS_TO_RGB(h, l, s)


def rgb_to_hsv(r, g, b):
    return _RGB_TO_HSV(r, g, b)


def hsv_to_rgb(h, s, v):
    return _HSV_TO_RGB(h, s, v)
