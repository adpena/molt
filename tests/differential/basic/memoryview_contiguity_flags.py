"""memoryview.c_contiguous, f_contiguous and contiguous over each layout kind."""


def flags(view):
    return (view.c_contiguous, view.f_contiguous, view.contiguous)


views = [
    ("bytes", memoryview(b"abcdef")),
    ("bytearray", memoryview(bytearray(b"abcdef"))),
    ("empty", memoryview(b"")),
    ("step 2", memoryview(b"abcdef")[::2]),
    ("reversed", memoryview(b"abcdef")[::-1]),
    ("empty strided", memoryview(b"abcdef")[0:0:2]),
    ("singleton strided", memoryview(b"ab")[::2]),
    ("slice", memoryview(b"abcdef")[1:4]),
    ("typed", memoryview(b"abcdefgh").cast("H")),
    ("2-D", memoryview(bytearray(6)).cast("B", (2, 3))),
    ("2-D row", memoryview(bytearray(3)).cast("B", (1, 3))),
    ("2-D column", memoryview(bytearray(3)).cast("B", (3, 1))),
    ("2-D empty strided", memoryview(bytearray(6)).cast("B", (3, 2))[0:0:2]),
    ("2-D strided rows", memoryview(bytearray(12)).cast("B", (4, 3))[::2]),
    ("0-D", memoryview(bytearray(1)).cast("B", ())),
]
for label, view in views:
    print(label, flags(view), view.ndim, view.shape, view.strides)

released = memoryview(b"xyz")
released.release()
for name in ("c_contiguous", "f_contiguous", "contiguous"):
    try:
        print(name, getattr(released, name))
    except ValueError as error:
        print(name, "ValueError", error)
