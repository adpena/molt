"""Matched float colorsys workload; identity/protocol cases are correctness-only."""

import colorsys

iterations = 100_000
index = 0
total = 0.0
while index < iterations:
    h, s, v = colorsys.rgb_to_hsv(0.25, 0.5, 0.75)
    r, g, b = colorsys.hsv_to_rgb(h, s, v)
    total += r + g + b
    index += 1
assert abs(total - 1.5 * iterations) < 0.000001
print(iterations, round(total, 6))
