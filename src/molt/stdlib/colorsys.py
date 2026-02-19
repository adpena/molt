"""Conversion functions between RGB and other color systems (intrinsic-backed)."""

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

# Constants mirrored from CPython colorsys.
ONE_THIRD = 1.0 / 3.0
ONE_SIXTH = 1.0 / 6.0
TWO_THIRD = 2.0 / 3.0

_MOLT_RGB_TO_HSV = _require_intrinsic("molt_colorsys_rgb_to_hsv", globals())
_MOLT_HSV_TO_RGB = _require_intrinsic("molt_colorsys_hsv_to_rgb", globals())
_MOLT_RGB_TO_HLS = _require_intrinsic("molt_colorsys_rgb_to_hls", globals())
_MOLT_HLS_TO_RGB = _require_intrinsic("molt_colorsys_hls_to_rgb", globals())
_MOLT_RGB_TO_YIQ = _require_intrinsic("molt_colorsys_rgb_to_yiq", globals())
_MOLT_YIQ_TO_RGB = _require_intrinsic("molt_colorsys_yiq_to_rgb", globals())


def _expect_triplet(value, name: str) -> tuple[float, float, float]:
    if not isinstance(value, tuple) or len(value) != 3:
        raise RuntimeError(f"{name} intrinsic returned invalid value")
    a, b, c = value
    if not isinstance(a, float) or not isinstance(b, float) or not isinstance(c, float):
        raise RuntimeError(f"{name} intrinsic returned invalid value")
    return a, b, c


def rgb_to_hsv(r: float, g: float, b: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_RGB_TO_HSV(r, g, b), "colorsys.rgb_to_hsv")


def hsv_to_rgb(h: float, s: float, v: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_HSV_TO_RGB(h, s, v), "colorsys.hsv_to_rgb")


def rgb_to_hls(r: float, g: float, b: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_RGB_TO_HLS(r, g, b), "colorsys.rgb_to_hls")


def hls_to_rgb(h: float, l: float, s: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_HLS_TO_RGB(h, l, s), "colorsys.hls_to_rgb")


def rgb_to_yiq(r: float, g: float, b: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_RGB_TO_YIQ(r, g, b), "colorsys.rgb_to_yiq")


def yiq_to_rgb(y: float, i: float, q: float) -> tuple[float, float, float]:
    return _expect_triplet(_MOLT_YIQ_TO_RGB(y, i, q), "colorsys.yiq_to_rgb")
