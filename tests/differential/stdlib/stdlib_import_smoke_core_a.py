"""Purpose: stdlib import smoke for core pure modules (A)."""

import calendar
import cmath
import colorsys
import configparser

modules = [
    calendar,
    cmath,
    colorsys,
    configparser,
]
print([mod.__name__ for mod in modules])
