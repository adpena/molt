"""Purpose: differential coverage for colorsys conversions."""

import colorsys


rgb_cases = [
    (0.0, 0.0, 0.0),
    (1.0, 1.0, 1.0),
    (1.0, 0.0, 0.0),
    (0.0, 1.0, 0.0),
    (0.0, 0.0, 1.0),
    (0.2, 0.4, 0.6),
    (0.9, 0.1, 0.7),
    (1.2, -0.1, 0.5),
]

yiq_cases = [
    (0.0, 0.0, 0.0),
    (0.5, 0.2, -0.1),
    (0.75, -0.3, 0.4),
]

hls_cases = [
    (0.0, 0.0, 0.0),
    (0.5, 0.5, 0.5),
    (0.7, 0.4, 0.9),
    (-0.1, 0.2, 0.3),
]

hsv_cases = [
    (0.0, 0.0, 0.0),
    (0.5, 0.5, 0.5),
    (0.9, 0.2, 0.8),
    (-0.2, 0.7, 0.9),
]

for r, g, b in rgb_cases:
    print("rgb_to_yiq", r, g, b, colorsys.rgb_to_yiq(r, g, b))
    print("rgb_to_hls", r, g, b, colorsys.rgb_to_hls(r, g, b))
    print("rgb_to_hsv", r, g, b, colorsys.rgb_to_hsv(r, g, b))

for y, i, q in yiq_cases:
    print("yiq_to_rgb", y, i, q, colorsys.yiq_to_rgb(y, i, q))

for h, l, s in hls_cases:
    print("hls_to_rgb", h, l, s, colorsys.hls_to_rgb(h, l, s))

for h, s, v in hsv_cases:
    print("hsv_to_rgb", h, s, v, colorsys.hsv_to_rgb(h, s, v))
