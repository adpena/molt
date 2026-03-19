"""Intrinsic-backed colorsys module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "rgb_to_hls",
    "hls_to_rgb",
    "rgb_to_hsv",
    "hsv_to_rgb",
    "rgb_to_yiq",
    "yiq_to_rgb",
]

_MOLT_RGB_TO_HLS = _require_intrinsic("molt_colorsys_rgb_to_hls")
_MOLT_HLS_TO_RGB = _require_intrinsic("molt_colorsys_hls_to_rgb")
_MOLT_RGB_TO_HSV = _require_intrinsic("molt_colorsys_rgb_to_hsv")
_MOLT_HSV_TO_RGB = _require_intrinsic("molt_colorsys_hsv_to_rgb")
_MOLT_RGB_TO_YIQ = _require_intrinsic("molt_colorsys_rgb_to_yiq")
_MOLT_YIQ_TO_RGB = _require_intrinsic("molt_colorsys_yiq_to_rgb")


def _require_tuple3_float(out: object, name: str) -> tuple[float, float, float]:
    if not isinstance(out, tuple) or len(out) != 3:
        raise RuntimeError(f"colorsys.{name} intrinsic returned invalid value")
    a, b, c = out
    if not isinstance(a, float) or not isinstance(b, float) or not isinstance(c, float):
        raise RuntimeError(f"colorsys.{name} intrinsic returned invalid value")
    return a, b, c


def rgb_to_hls(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_RGB_TO_HLS(r, g, b), "rgb_to_hls")


def hls_to_rgb(h: object, l: object, s: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_HLS_TO_RGB(h, l, s), "hls_to_rgb")


def rgb_to_hsv(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_RGB_TO_HSV(r, g, b), "rgb_to_hsv")


def hsv_to_rgb(h: object, s: object, v: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_HSV_TO_RGB(h, s, v), "hsv_to_rgb")


def rgb_to_yiq(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_RGB_TO_YIQ(r, g, b), "rgb_to_yiq")


def yiq_to_rgb(y: object, i: object, q: object) -> tuple[float, float, float]:
    return _require_tuple3_float(_MOLT_YIQ_TO_RGB(y, i, q), "yiq_to_rgb")

globals().pop("_require_intrinsic", None)
