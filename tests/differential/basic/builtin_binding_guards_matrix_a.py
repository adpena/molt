"""Shadowing len, int or isinstance switches a guarded call to generic semantics.

Rotation 0 of the matrix in builtin_binding_guard_support/matrix.py: every
name meets one module path and one builtins path first. The three rotation
files together give each (name, path) pair a first touch (HF-79).
"""

from builtin_binding_guard_support import matrix

matrix.run(rotation=0)
