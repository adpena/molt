from __future__ import annotations

import colorsys
import math


def assert_triplet_close(actual, expected, *, tol=1e-12):
    assert len(actual) == 3
    assert len(expected) == 3
    for left, right in zip(actual, expected):
        assert math.isclose(left, right, rel_tol=tol, abs_tol=tol)


def test_colorsys_known_values():
    assert_triplet_close(colorsys.rgb_to_yiq(0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
    assert_triplet_close(colorsys.rgb_to_yiq(1.0, 1.0, 1.0), (1.0, 0.0, 0.0))
    assert_triplet_close(colorsys.rgb_to_hls(1.0, 0.0, 0.0), (0.0, 0.5, 1.0))
    assert_triplet_close(colorsys.rgb_to_hsv(1.0, 0.0, 0.0), (0.0, 1.0, 1.0))
    assert_triplet_close(colorsys.hls_to_rgb(0.0, 0.5, 1.0), (1.0, 0.0, 0.0))
    assert_triplet_close(colorsys.hsv_to_rgb(0.0, 1.0, 1.0), (1.0, 0.0, 0.0))


def test_colorsys_roundtrip():
    for rgb in ((0.2, 0.4, 0.6), (0.9, 0.1, 0.3), (0.05, 0.95, 0.5)):
        h, l, s = colorsys.rgb_to_hls(*rgb)
        assert_triplet_close(colorsys.hls_to_rgb(h, l, s), rgb, tol=1e-10)
        h, s, v = colorsys.rgb_to_hsv(*rgb)
        assert_triplet_close(colorsys.hsv_to_rgb(h, s, v), rgb, tol=1e-10)


def test_colorsys_yiq_clamp():
    r, g, b = colorsys.yiq_to_rgb(0.5, 1.0, 1.0)
    assert 0.0 <= r <= 1.0
    assert 0.0 <= g <= 1.0
    assert 0.0 <= b <= 1.0
