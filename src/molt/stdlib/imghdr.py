"""Image type detection helpers (intrinsic-backed)."""

from os import PathLike
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic
from os import PathLike
import warnings

_MOLT_IMGHDR_TEST = _require_intrinsic("molt_imghdr_test", globals())
_MOLT_IMGHDR_WHAT = _require_intrinsic("molt_imghdr_what", globals())

warnings.warn(
    "'imghdr' is deprecated and slated for removal in Python 3.13",
    DeprecationWarning,
    stacklevel=2,
)


tests: list = []


def test_jpeg(h, f):
    _ = f
    if isinstance(h, str):
        if h[6:10] in (b"JFIF", b"Exif"):
            return "jpeg"
        if h[:4] == b"\xff\xd8\xff\xdb":
            return "jpeg"
        return None
    if _MOLT_IMGHDR_TEST("jpeg", h):
        return "jpeg"


def test_png(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"\x89PNG\r\n\x1a\n"):
            return "png"
        return None
    if _MOLT_IMGHDR_TEST("png", h):
        return "png"


def test_gif(h, f):
    _ = f
    if isinstance(h, str):
        if h[:6] in (b"GIF87a", b"GIF89a"):
            return "gif"
        return None
    if _MOLT_IMGHDR_TEST("gif", h):
        return "gif"


def test_tiff(h, f):
    _ = f
    if isinstance(h, str):
        if h[:2] in (b"MM", b"II"):
            return "tiff"
        return None
    if _MOLT_IMGHDR_TEST("tiff", h):
        return "tiff"


def test_rgb(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"\x01\xda"):
            return "rgb"
        return None
    if _MOLT_IMGHDR_TEST("rgb", h):
        return "rgb"


def test_pbm(h, f):
    _ = f
    if isinstance(h, str):
        if (
            len(h) >= 3
            and h[0] == ord(b"P")
            and h[1] in b"14"
            and h[2] in b" \t\n\r"
        ):
            return "pbm"
        return None
    if _MOLT_IMGHDR_TEST("pbm", h):
        return "pbm"


def test_pgm(h, f):
    _ = f
    if isinstance(h, str):
        if (
            len(h) >= 3
            and h[0] == ord(b"P")
            and h[1] in b"25"
            and h[2] in b" \t\n\r"
        ):
            return "pgm"
        return None
    if _MOLT_IMGHDR_TEST("pgm", h):
        return "pgm"


def test_ppm(h, f):
    _ = f
    if isinstance(h, str):
        if (
            len(h) >= 3
            and h[0] == ord(b"P")
            and h[1] in b"36"
            and h[2] in b" \t\n\r"
        ):
            return "ppm"
        return None
    if _MOLT_IMGHDR_TEST("ppm", h):
        return "ppm"


def test_rast(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"\x59\xa6\x6a\x95"):
            return "rast"
        return None
    if _MOLT_IMGHDR_TEST("rast", h):
        return "rast"


def test_xbm(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"#define "):
            return "xbm"
        return None
    if _MOLT_IMGHDR_TEST("xbm", h):
        return "xbm"


def test_bmp(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"BM"):
            return "bmp"
        return None
    if _MOLT_IMGHDR_TEST("bmp", h):
        return "bmp"


def test_webp(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"RIFF") and h[8:12] == b"WEBP":
            return "webp"
        return None
    if _MOLT_IMGHDR_TEST("webp", h):
        return "webp"


def test_exr(h, f):
    _ = f
    if isinstance(h, str):
        if h.startswith(b"\x76\x2f\x31\x01"):
            return "exr"
        return None
    if _MOLT_IMGHDR_TEST("exr", h):
        return "exr"


tests.append(test_jpeg)
tests.append(test_png)
tests.append(test_gif)
tests.append(test_tiff)
tests.append(test_rgb)
tests.append(test_pbm)
tests.append(test_pgm)
tests.append(test_ppm)
tests.append(test_rast)
tests.append(test_xbm)
tests.append(test_bmp)
tests.append(test_webp)
tests.append(test_exr)

_BUILTIN_TESTS = tuple(tests)


def what(file, h=None):
    f = None
    try:
        if h is None:
            if isinstance(file, (str, bytes, PathLike)):
                f = open(file, "rb")
                h = f.read(32)
            else:
                f = file
                try:
                    h = f.read(32)
                except AttributeError:
                    return None
        if tests[: len(_BUILTIN_TESTS)] == list(_BUILTIN_TESTS) and not isinstance(h, str):
            result = _MOLT_IMGHDR_WHAT(h)
            if result is not None:
                return result
            start_index = len(_BUILTIN_TESTS)
        else:
            start_index = 0

        for test in tests[start_index:]:
            if res := test(h, f):
                return res
        return None
    finally:
        if f is not None and f is not file:
            f.close()


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


def testall(list, recursive, toplevel):
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
