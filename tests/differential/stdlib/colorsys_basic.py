"""Purpose: differential coverage for colorsys conversions."""

import colorsys


def main() -> None:
    print("rgb_to_hsv", colorsys.rgb_to_hsv(0.2, 0.4, 0.6))
    print("hsv_to_rgb", colorsys.hsv_to_rgb(0.5, 0.25, 0.9))
    print("rgb_to_hls", colorsys.rgb_to_hls(0.2, 0.4, 0.6))
    print("hls_to_rgb", colorsys.hls_to_rgb(0.5, 0.4, 0.2))
    print("rgb_to_yiq", colorsys.rgb_to_yiq(0.2, 0.4, 0.6))
    print("yiq_to_rgb", colorsys.yiq_to_rgb(0.2, 0.1, -0.1))
    print("grey_hsv", colorsys.rgb_to_hsv(0.3, 0.3, 0.3))
    print("grey_rgb", colorsys.hsv_to_rgb(0.75, 0.0, 0.3))
    print("neg_hue", colorsys.hsv_to_rgb(-0.1, 0.5, 0.5))


if __name__ == "__main__":
    main()
