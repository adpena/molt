"""Color conversion utilities (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "rgb_to_hsv",
    "hsv_to_rgb",
    "rgb_to_hls",
    "hls_to_rgb",
    "rgb_to_yiq",
    "yiq_to_rgb",
]

_MOLT_COLORSYS_RGB_TO_HSV = _require_intrinsic("molt_colorsys_rgb_to_hsv", globals())
_MOLT_COLORSYS_HSV_TO_RGB = _require_intrinsic("molt_colorsys_hsv_to_rgb", globals())
_MOLT_COLORSYS_RGB_TO_HLS = _require_intrinsic("molt_colorsys_rgb_to_hls", globals())
_MOLT_COLORSYS_HLS_TO_RGB = _require_intrinsic("molt_colorsys_hls_to_rgb", globals())
_MOLT_COLORSYS_RGB_TO_YIQ = _require_intrinsic("molt_colorsys_rgb_to_yiq", globals())
_MOLT_COLORSYS_YIQ_TO_RGB = _require_intrinsic("molt_colorsys_yiq_to_rgb", globals())


def _validate_triplet(name: str, value: object) -> tuple[float, float, float]:
    if not isinstance(value, tuple) or len(value) != 3:
        raise RuntimeError(f"colorsys.{name} intrinsic returned invalid value")
    if not all(isinstance(item, float) for item in value):
        raise RuntimeError(f"colorsys.{name} intrinsic returned invalid value")
    return value


def rgb_to_hsv(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _validate_triplet("rgb_to_hsv", _MOLT_COLORSYS_RGB_TO_HSV(r, g, b))


def hsv_to_rgb(h: object, s: object, v: object) -> tuple[float, float, float]:
    return _validate_triplet("hsv_to_rgb", _MOLT_COLORSYS_HSV_TO_RGB(h, s, v))


def rgb_to_hls(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _validate_triplet("rgb_to_hls", _MOLT_COLORSYS_RGB_TO_HLS(r, g, b))


def hls_to_rgb(h: object, l: object, s: object) -> tuple[float, float, float]:
    return _validate_triplet("hls_to_rgb", _MOLT_COLORSYS_HLS_TO_RGB(h, l, s))


def rgb_to_yiq(r: object, g: object, b: object) -> tuple[float, float, float]:
    return _validate_triplet("rgb_to_yiq", _MOLT_COLORSYS_RGB_TO_YIQ(r, g, b))


def yiq_to_rgb(y: object, i: object, q: object) -> tuple[float, float, float]:
    return _validate_triplet("yiq_to_rgb", _MOLT_COLORSYS_YIQ_TO_RGB(y, i, q))
