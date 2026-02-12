# MOLT_ENV: MOLT_CAPABILITIES=ffi.unsafe
"""Purpose: differential coverage for ctypes basics."""

import ctypes


class Point(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int)]


p = Point(3, 4)
print(ctypes.sizeof(Point), p.x, p.y)
ptr = ctypes.pointer(p)
ptr.contents.x = 10
print(p.x)
arr = (ctypes.c_int * 3)(1, 2, 3)
print(list(arr))
print(ctypes.sizeof(arr))
