"""Mixed float/int comparisons must stay on the float lane."""


def compare(x):
    y = float(x)
    return (
        y < 0,
        y <= 0,
        y > 0,
        y >= 0,
        y == 0,
        y != 0,
    )


for value in (0.01, 0.0, -0.01, 1, True):
    print(value, compare(value))
