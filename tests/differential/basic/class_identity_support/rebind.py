"""Rebinds a class name that another module defines (HF-220)."""

import class_identity_support.shapes as shapes


def swap():
    shapes.P = shapes.Q
