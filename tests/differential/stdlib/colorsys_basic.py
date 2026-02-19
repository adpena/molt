"""Purpose: differential coverage for colorsys."""

import colorsys


def _round_triplet(values, places=12):
    return tuple(round(v, places) for v in values)


def _close_triplet(a, b, tol=1e-9):
    ax, ay, az = a
    bx, by, bz = b
    return (
        abs(ax - bx) <= tol
        and abs(ay - by) <= tol
        and abs(az - bz) <= tol
    )


def main():
    samples = [
        (0.0, 0.0, 0.0),
        (1.0, 1.0, 1.0),
        (1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.2, 0.4, 0.6),
        (0.9, 0.2, 0.7),
    ]
    for rgb in samples:
        print("rgb_to_hls", rgb, _round_triplet(colorsys.rgb_to_hls(*rgb)))
        print("rgb_to_hsv", rgb, _round_triplet(colorsys.rgb_to_hsv(*rgb)))
        print("rgb_to_yiq", rgb, _round_triplet(colorsys.rgb_to_yiq(*rgb)))

    hls_samples = [
        (0.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.5, 0.4, 0.7),
        (0.75, 0.2, 0.8),
    ]
    for hls in hls_samples:
        print("hls_to_rgb", hls, _round_triplet(colorsys.hls_to_rgb(*hls)))

    hsv_samples = [
        (0.0, 0.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.3, 0.2, 0.8),
        (0.9, 0.6, 0.3),
    ]
    for hsv in hsv_samples:
        print("hsv_to_rgb", hsv, _round_triplet(colorsys.hsv_to_rgb(*hsv)))

    yiq_samples = [
        (0.0, 0.0, 0.0),
        (1.0, 0.0, 0.0),
        (0.5, 0.2, 0.1),
        (0.75, -0.2, 0.3),
    ]
    for yiq in yiq_samples:
        print("yiq_to_rgb", yiq, _round_triplet(colorsys.yiq_to_rgb(*yiq)))

    roundtrip_rgb = [
        (0.1, 0.2, 0.3),
        (0.9, 0.1, 0.4),
        (0.0, 1.0, 0.5),
    ]
    for rgb in roundtrip_rgb:
        hls = colorsys.rgb_to_hls(*rgb)
        rgb_back_hls = colorsys.hls_to_rgb(*hls)
        print("roundtrip_hls", rgb, _close_triplet(rgb, rgb_back_hls))
        hsv = colorsys.rgb_to_hsv(*rgb)
        rgb_back_hsv = colorsys.hsv_to_rgb(*hsv)
        print("roundtrip_hsv", rgb, _close_triplet(rgb, rgb_back_hsv))
        yiq = colorsys.rgb_to_yiq(*rgb)
        rgb_back_yiq = colorsys.yiq_to_rgb(*yiq)
        print("roundtrip_yiq", rgb, _close_triplet(rgb, rgb_back_yiq))


if __name__ == "__main__":
    main()
