import colorsys


def fmt_triplet(values):
    return tuple(round(v, 6) for v in values)


print("rgb_to_hls_red", fmt_triplet(colorsys.rgb_to_hls(1.0, 0.0, 0.0)))
print("rgb_to_hls_gray", fmt_triplet(colorsys.rgb_to_hls(0.25, 0.25, 0.25)))
print("hls_to_rgb_red", fmt_triplet(colorsys.hls_to_rgb(0.0, 0.5, 1.0)))
print("hls_to_rgb_wrap", fmt_triplet(colorsys.hls_to_rgb(1.2, 0.5, 0.5)))

print("rgb_to_hsv_sample", fmt_triplet(colorsys.rgb_to_hsv(0.2, 0.4, 0.6)))
print("hsv_to_rgb_sample", fmt_triplet(colorsys.hsv_to_rgb(0.5833333333, 0.6666666667, 0.6)))
print("hsv_to_rgb_wrap", fmt_triplet(colorsys.hsv_to_rgb(-0.1, 0.5, 0.75)))

print("rgb_to_yiq_sample", fmt_triplet(colorsys.rgb_to_yiq(0.2, 0.4, 0.6)))
print("yiq_to_rgb_sample", fmt_triplet(colorsys.yiq_to_rgb(0.3, 0.1, -0.1)))
