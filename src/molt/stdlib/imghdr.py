"""Recognize image file formats based on their first few bytes.

This module mirrors CPython 3.12 `imghdr` surface and is version-gated absent
for >=3.13 at the importlib boundary.
"""

from _intrinsics import require_intrinsic as _require_intrinsic
from os import PathLike
import warnings

__all__ = ["what"]

_MOLT_IMGHDR_DETECT = _require_intrinsic("molt_imghdr_detect", globals())


def what(file, h=None):
    f = None
    try:
        if h is None:
            if isinstance(file, (str, PathLike)):
                f = open(file, "rb")
                h = f.read(32)
            else:
                location = file.tell()
                h = file.read(32)
                file.seek(location)
        for tf in tests:
            res = tf(h, f)
            if res:
                return res
    finally:
        if f:
            f.close()
    return None


def _match(h, kind):
    detected = _MOLT_IMGHDR_DETECT(h)
    if detected == kind:
        return kind
    return None


tests = []


def test_jpeg(h, f):
    return _match(h, "jpeg")


tests.append(test_jpeg)


def test_png(h, f):
    return _match(h, "png")


tests.append(test_png)


def test_gif(h, f):
    return _match(h, "gif")


tests.append(test_gif)


def test_tiff(h, f):
    return _match(h, "tiff")


tests.append(test_tiff)


def test_rgb(h, f):
    return _match(h, "rgb")


tests.append(test_rgb)


def test_pbm(h, f):
    return _match(h, "pbm")


tests.append(test_pbm)


def test_pgm(h, f):
    return _match(h, "pgm")


tests.append(test_pgm)


def test_ppm(h, f):
    return _match(h, "ppm")


tests.append(test_ppm)


def test_rast(h, f):
    return _match(h, "rast")


tests.append(test_rast)


def test_xbm(h, f):
    return _match(h, "xbm")


tests.append(test_xbm)


def test_bmp(h, f):
    return _match(h, "bmp")


tests.append(test_bmp)


def test_webp(h, f):
    return _match(h, "webp")


tests.append(test_webp)


def test_exr(h, f):
    return _match(h, "exr")


tests.append(test_exr)


def test():
    import sys

    recursive = 0
    if sys.argv[1:] and sys.argv[1] == "-r":
        del sys.argv[1:2]
        recursive = 1
    try:
        if sys.argv[1:]:
            testall(sys.argv[1:], recursive, 1)
        else:
            testall(["."], recursive, 1)
    except KeyboardInterrupt:
        sys.stderr.write("\n[Interrupted]\n")
        sys.exit(1)


def testall(list, recursive, toplevel):  # noqa: A002
    import glob
    import os
    import sys

    for filename in list:
        if os.path.isdir(filename):
            print(filename + "/:", end=" ")
            if recursive or toplevel:
                print("recursing down:")
                names = glob.glob(os.path.join(glob.escape(filename), "*"))
                testall(names, recursive, 0)
            else:
                print("*** directory (use -r) ***")
        else:
            print(filename + ":", end=" ")
            sys.stdout.flush()
            try:
                print(what(filename))
            except OSError:
                print("*** not found ***")


if __name__ == "__main__":
    test()
