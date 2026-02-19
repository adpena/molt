from __future__ import annotations

import math

import colorsys


def assert_triplet_close(actual: tuple[float, float, float], expected, *, rel=1e-12, abs=1e-12):
    assert isinstance(actual, tuple)
    assert len(actual) == 3
    for got, want in zip(actual, expected):
        assert math.isclose(got, want, rel_tol=rel, abs_tol=abs), (actual, expected)


def test_rgb_hsv_primary_roundtrip():
    assert_triplet_close(colorsys.rgb_to_hsv(0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
    assert_triplet_close(colorsys.rgb_to_hsv(1.0, 0.0, 0.0), (0.0, 1.0, 1.0))
    assert_triplet_close(colorsys.rgb_to_hsv(0.0, 1.0, 0.0), (1.0 / 3.0, 1.0, 1.0))
    assert_triplet_close(colorsys.rgb_to_hsv(0.0, 0.0, 1.0), (2.0 / 3.0, 1.0, 1.0))

    assert_triplet_close(colorsys.hsv_to_rgb(0.0, 1.0, 1.0), (1.0, 0.0, 0.0))
    assert_triplet_close(colorsys.hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), (0.0, 1.0, 0.0))
    assert_triplet_close(colorsys.hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), (0.0, 0.0, 1.0))

    sample = (0.2, 0.4, 0.6)
    assert_triplet_close(colorsys.hsv_to_rgb(*colorsys.rgb_to_hsv(*sample)), sample)


def test_rgb_hls_primary_roundtrip():
    assert_triplet_close(colorsys.rgb_to_hls(1.0, 0.0, 0.0), (0.0, 0.5, 1.0))
    assert_triplet_close(colorsys.rgb_to_hls(0.0, 1.0, 0.0), (1.0 / 3.0, 0.5, 1.0))
    assert_triplet_close(colorsys.rgb_to_hls(0.0, 0.0, 1.0), (2.0 / 3.0, 0.5, 1.0))

    assert_triplet_close(colorsys.hls_to_rgb(0.0, 0.5, 1.0), (1.0, 0.0, 0.0))
    assert_triplet_close(colorsys.hls_to_rgb(1.0 / 3.0, 0.5, 1.0), (0.0, 1.0, 0.0))
    assert_triplet_close(colorsys.hls_to_rgb(2.0 / 3.0, 0.5, 1.0), (0.0, 0.0, 1.0))

    sample = (0.1, 0.5, 0.2)
    assert_triplet_close(colorsys.hls_to_rgb(*colorsys.rgb_to_hls(*sample)), sample)


def test_rgb_yiq_roundtrip_and_clamp():
    assert_triplet_close(colorsys.rgb_to_yiq(1.0, 0.0, 0.0), (0.299, 0.596, 0.211))
    assert_triplet_close(colorsys.yiq_to_rgb(0.299, 0.596, 0.211), (1.0, 0.0, 0.0))

    clamped = colorsys.yiq_to_rgb(0.0, 1.0, 1.0)
    assert_triplet_close(clamped, (1.0, 0.0, 0.597))
