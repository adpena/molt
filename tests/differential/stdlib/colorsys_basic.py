from __future__ import annotations

import colorsys
import math


# CPython reference formulas for parity checks.
def _ref_v(m1, m2, h):
    h = h % 1.0
    if h < 1.0 / 6.0:
        return m1 + (m2 - m1) * h * 6.0
    if h < 0.5:
        return m2
    if h < 2.0 / 3.0:
        return m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0
    return m1


def _ref_rgb_to_hsv(r, g, b):
    maxc = max(r, g, b)
    minc = min(r, g, b)
    rangec = maxc - minc
    v = maxc
    if minc == maxc:
        return 0.0, 0.0, v
    s = rangec / maxc
    rc = (maxc - r) / rangec
    gc = (maxc - g) / rangec
    bc = (maxc - b) / rangec
    if r == maxc:
        h = bc - gc
    elif g == maxc:
        h = 2.0 + rc - bc
    else:
        h = 4.0 + gc - rc
    h = (h / 6.0) % 1.0
    return h, s, v


def _ref_hsv_to_rgb(h, s, v):
    if s == 0.0:
        return v, v, v
    i = int(h * 6.0)
    f = (h * 6.0) - i
    p = v * (1.0 - s)
    q = v * (1.0 - s * f)
    t = v * (1.0 - s * (1.0 - f))
    i = i % 6
    if i == 0:
        return v, t, p
    if i == 1:
        return q, v, p
    if i == 2:
        return p, v, t
    if i == 3:
        return p, q, v
    if i == 4:
        return t, p, v
    return v, p, q


def _ref_rgb_to_hls(r, g, b):
    maxc = max(r, g, b)
    minc = min(r, g, b)
    sumc = maxc + minc
    rangec = maxc - minc
    l = sumc / 2.0
    if minc == maxc:
        return 0.0, l, 0.0
    if l <= 0.5:
        s = rangec / sumc
    else:
        s = rangec / (2.0 - maxc - minc)
    rc = (maxc - r) / rangec
    gc = (maxc - g) / rangec
    bc = (maxc - b) / rangec
    if r == maxc:
        h = bc - gc
    elif g == maxc:
        h = 2.0 + rc - bc
    else:
        h = 4.0 + gc - rc
    h = (h / 6.0) % 1.0
    return h, l, s


def _ref_hls_to_rgb(h, l, s):
    if s == 0.0:
        return l, l, l
    if l <= 0.5:
        m2 = l * (1.0 + s)
    else:
        m2 = l + s - (l * s)
    m1 = 2.0 * l - m2
    return _ref_v(m1, m2, h + 1.0 / 3.0), _ref_v(m1, m2, h), _ref_v(m1, m2, h - 1.0 / 3.0)


def _ref_rgb_to_yiq(r, g, b):
    y = 0.30 * r + 0.59 * g + 0.11 * b
    i = 0.74 * (r - y) - 0.27 * (b - y)
    q = 0.48 * (r - y) + 0.41 * (b - y)
    return y, i, q


def _ref_yiq_to_rgb(y, i, q):
    r = y + 0.9468822170900693 * i + 0.6235565819861433 * q
    g = y - 0.27478764629897834 * i - 0.6356910791873801 * q
    b = y - 1.1085450346420322 * i + 1.7090069284064666 * q
    if r < 0.0:
        r = 0.0
    if g < 0.0:
        g = 0.0
    if b < 0.0:
        b = 0.0
    if r > 1.0:
        r = 1.0
    if g > 1.0:
        g = 1.0
    if b > 1.0:
        b = 1.0
    return r, g, b


def _outcome(fn, *args):
    try:
        return ("ok", fn(*args))
    except Exception as exc:
        return ("err", type(exc).__name__)


def _assert_tuple_close(actual, expected, rel=1e-12, abs_tol=1e-12):
    assert len(actual) == len(expected)
    for a, e in zip(actual, expected):
        assert math.isclose(a, e, rel_tol=rel, abs_tol=abs_tol)


def _assert_matches_reference(fn, ref, *args):
    got = _outcome(fn, *args)
    want = _outcome(ref, *args)
    assert got[0] == want[0]
    if got[0] == "err":
        assert got[1] == want[1]
        return
    _assert_tuple_close(got[1], want[1])


def test_rgb_to_hsv_grid_matches_reference():
    vals = [-1.0, -0.5, -0.0, 0.0, 0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0]
    for r in vals:
        for g in vals:
            for b in vals:
                _assert_matches_reference(colorsys.rgb_to_hsv, _ref_rgb_to_hsv, r, g, b)


def test_hsv_to_rgb_grid_matches_reference():
    hs = [-2.0, -1.5, -1.0, -0.25, -0.0, 0.0, 0.125, 0.5, 0.999999999999, 1.0, 1.5, 2.0]
    sv = [-1.0, -0.0, 0.0, 0.2, 0.5, 1.0, 2.0]
    for h in hs:
        for s in sv:
            for v in sv:
                _assert_matches_reference(colorsys.hsv_to_rgb, _ref_hsv_to_rgb, h, s, v)


def test_rgb_to_hls_grid_matches_reference():
    vals = [-1.0, -0.5, -0.0, 0.0, 0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0]
    for r in vals:
        for g in vals:
            for b in vals:
                _assert_matches_reference(colorsys.rgb_to_hls, _ref_rgb_to_hls, r, g, b)


def test_hls_to_rgb_grid_matches_reference():
    hs = [-2.0, -1.0, -0.5, -0.0, 0.0, 0.1, 0.5, 1.0, 1.5, 2.0]
    ls = [-1.0, -0.0, 0.0, 0.2, 0.5, 1.0, 2.0]
    ss = [-1.0, -0.0, 0.0, 0.3, 0.5, 1.0, 2.0]
    for h in hs:
        for l in ls:
            for s in ss:
                _assert_matches_reference(colorsys.hls_to_rgb, _ref_hls_to_rgb, h, l, s)


def test_rgb_to_yiq_grid_matches_reference():
    vals = [-1.0, -0.5, -0.0, 0.0, 0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0]
    for r in vals:
        for g in vals:
            for b in vals:
                _assert_matches_reference(colorsys.rgb_to_yiq, _ref_rgb_to_yiq, r, g, b)


def test_yiq_to_rgb_grid_matches_reference():
    vals = [-2.0, -1.0, -0.5, -0.0, 0.0, 0.2, 0.5, 1.0, 1.5, 2.0]
    for y in vals:
        for i in vals:
            for q in vals:
                _assert_matches_reference(colorsys.yiq_to_rgb, _ref_yiq_to_rgb, y, i, q)


def test_nan_inf_edge_behavior_matches_reference():
    edge_values = [float("nan"), float("inf"), -float("inf")]
    for h in edge_values:
        _assert_matches_reference(colorsys.hsv_to_rgb, _ref_hsv_to_rgb, h, 1.0, 1.0)
    for h in edge_values:
        _assert_matches_reference(colorsys.hls_to_rgb, _ref_hls_to_rgb, h, 0.5, 0.5)


class FloatLike:
    def __float__(self):
        return 0.25


class IndexLike:
    def __index__(self):
        return 1


def test_type_error_edges_match_reference():
    bad_values = ["x", FloatLike(), IndexLike(), object()]
    for bad in bad_values:
        _assert_matches_reference(colorsys.rgb_to_hsv, _ref_rgb_to_hsv, bad, 0.0, 0.0)
        _assert_matches_reference(colorsys.rgb_to_hls, _ref_rgb_to_hls, bad, 0.0, 0.0)
        _assert_matches_reference(colorsys.rgb_to_yiq, _ref_rgb_to_yiq, bad, 0.0, 0.0)


def test_in_range_roundtrips_stay_close():
    vals = [0.0, 0.1, 0.25, 0.5, 0.75, 1.0]
    for r in vals:
        for g in vals:
            for b in vals:
                h, s, v = colorsys.rgb_to_hsv(r, g, b)
                rr, gg, bb = colorsys.hsv_to_rgb(h, s, v)
                _assert_tuple_close((rr, gg, bb), (r, g, b))
                h, l, s2 = colorsys.rgb_to_hls(r, g, b)
                rr, gg, bb = colorsys.hls_to_rgb(h, l, s2)
                _assert_tuple_close((rr, gg, bb), (r, g, b))
                y, i, q = colorsys.rgb_to_yiq(r, g, b)
                rr, gg, bb = colorsys.yiq_to_rgb(y, i, q)
                _assert_tuple_close((rr, gg, bb), (r, g, b))
