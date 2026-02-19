"""Image type detection helpers (intrinsic-backed)."""

from __future__ import annotations

from os import PathLike
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMGHDR_TEST = _require_intrinsic("molt_imghdr_test", globals())
_MOLT_IMGHDR_WHAT = _require_intrinsic("molt_imghdr_what", globals())


def _emit_deprecation_if_user_module_import() -> None:
    import sys as _sys

    frame = _sys._getframe(1)
    while frame is not None:
        module_name = frame.f_globals.get("__name__", "")
        if not module_name.startswith("importlib"):
            break
        frame = frame.f_back
    if frame is None:
        return
    module_name = frame.f_globals.get("__name__", "")
    if module_name not in {"__main__", "__mp_main__"}:
        return
    if frame.f_code.co_name != "<module>":
        return
    warnings.warn_explicit(
        "'imghdr' is deprecated and slated for removal in Python 3.13",
        DeprecationWarning,
        frame.f_code.co_filename,
        frame.f_lineno,
        module_name,
    )


_emit_deprecation_if_user_module_import()


tests: list = []


def test_jpeg(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("jpeg", h):
        return "jpeg"


def test_png(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("png", h):
        return "png"


def test_gif(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("gif", h):
        return "gif"


def test_tiff(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("tiff", h):
        return "tiff"


def test_rgb(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("rgb", h):
        return "rgb"


def test_pbm(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("pbm", h):
        return "pbm"


def test_pgm(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("pgm", h):
        return "pgm"


def test_ppm(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("ppm", h):
        return "ppm"


def test_rast(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("rast", h):
        return "rast"


def test_xbm(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("xbm", h):
        return "xbm"


def test_bmp(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("bmp", h):
        return "bmp"


def test_webp(h, f):
    _ = f
    if _MOLT_IMGHDR_TEST("webp", h):
        return "webp"


def test_exr(h, f):
    _ = f
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
            if isinstance(file, (str, PathLike)):
                f = open(file, "rb")
                h = f.read(32)
            else:
                location = file.tell()
                h = file.read(32)
                file.seek(location)

        if tests[: len(_BUILTIN_TESTS)] == list(_BUILTIN_TESTS):
            # CPython's default builtin test order raises this exact error
            # for str headers when test_png calls str.startswith(bytes).
            if isinstance(h, str):
                raise TypeError(
                    "startswith first arg must be str or a tuple of str, not bytes"
                )
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
