"""Purpose: differential coverage for colorsys conversions."""

import colorsys


def show_triplet(label, triplet):
    print(label, tuple(round(x, 10) for x in triplet))


def show_value(label, value):
    print(label, round(value, 10))


show_triplet("rgb_to_yiq", colorsys.rgb_to_yiq(0.2, 0.4, 0.6))
show_triplet("yiq_to_rgb", colorsys.yiq_to_rgb(0.4, 0.1, -0.2))

show_triplet("rgb_to_hls", colorsys.rgb_to_hls(0.2, 0.4, 0.6))
show_triplet("hls_to_rgb", colorsys.hls_to_rgb(0.6, 0.5, 0.25))

show_triplet("rgb_to_hsv", colorsys.rgb_to_hsv(0.2, 0.4, 0.6))
show_triplet("hsv_to_rgb", colorsys.hsv_to_rgb(0.6, 0.5, 0.7))

show_triplet("hsv_to_rgb_wrap", colorsys.hsv_to_rgb(1.6, 0.5, 0.7))
show_triplet("hls_to_rgb_wrap", colorsys.hls_to_rgb(-0.1, 0.5, 0.25))

show_triplet("hls_gray", colorsys.hls_to_rgb(0.3, 0.25, 0.0))
show_triplet("hsv_gray", colorsys.hsv_to_rgb(0.3, 0.0, 0.25))

show_value("_v", colorsys._v(0.1, 0.9, 0.25))

try:
    colorsys.rgb_to_hsv("nope", 0.2, 0.3)
except Exception as exc:
    print(type(exc).__name__, exc)


class Floaty:
    def __float__(self):
        return 0.2


try:
    colorsys.rgb_to_yiq(Floaty(), 0.4, 0.6)
except Exception as exc:
    print(type(exc).__name__, exc)


class Indexy:
    def __index__(self):
        return 1


try:
    colorsys.rgb_to_yiq(Indexy(), 0.2, 0.3)
except Exception as exc:
    print(type(exc).__name__, exc)


class BadIndex:
    def __index__(self):
        return 1.5


try:
    colorsys.rgb_to_yiq(BadIndex(), 0.2, 0.3)
except Exception as exc:
    print(type(exc).__name__, exc)
