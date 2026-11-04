"""Mixed float/int comparisons preserve exact Python ordering."""

import sys


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


def compare_wide(text):
    floating = float(text)
    integer = 9007199254740993
    return (
        integer == floating, integer != floating,
        integer < floating, integer <= floating,
        integer > floating, integer >= floating,
        floating == integer, floating != integer,
        floating < integer, floating <= integer,
        floating > integer, floating >= integer,
    )


text = sys.argv[1] if len(sys.argv) > 1 else '9007199254740992'
print('wide', compare_wide(text))
