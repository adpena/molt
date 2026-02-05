"""warnings_helper helpers for Molt."""

from __future__ import annotations

import contextlib
import functools
import importlib
import re
import sys
import warnings


def import_deprecated(name: str):
    """Import name while suppressing DeprecationWarning."""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", category=DeprecationWarning)
        return importlib.import_module(name)


def _check_syntax_error(
    testcase,
    statement,
    errtext: str = "",
    lineno: int | None = 1,
    offset: int | None = None,
):
    try:
        compile(statement, "<testcase>", "exec")
    except SyntaxError as exc:
        if errtext:
            testcase.assertRegex(str(exc), errtext)
        if lineno is not None:
            testcase.assertEqual(exc.lineno, lineno)
        if offset is not None:
            testcase.assertEqual(exc.offset, offset)
        return
    testcase.fail("SyntaxError not raised")


def check_syntax_warning(
    testcase,
    statement,
    errtext: str = "",
    *,
    lineno: int | None = 1,
    offset: int | None = None,
):
    from test import support as test_support

    with warnings.catch_warnings(record=True) as warns:
        warnings.simplefilter("always", SyntaxWarning)
        compile(statement, "<testcase>", "exec")
    testcase.assertEqual(len(warns), 1, warns)

    warn = warns[0]
    testcase.assertIsSubclass(warn.category, SyntaxWarning)
    if errtext:
        testcase.assertRegex(str(warn.message), errtext)
    testcase.assertEqual(warn.filename, "<testcase>")
    testcase.assertIsNotNone(warn.lineno)
    if lineno is not None:
        testcase.assertEqual(warn.lineno, lineno)

    with warnings.catch_warnings(record=True) as warns:
        warnings.simplefilter("error", SyntaxWarning)
        check_syntax_error = getattr(test_support, "check_syntax_error", None)
        if callable(check_syntax_error):
            check_syntax_error(
                testcase, statement, errtext, lineno=lineno, offset=offset
            )
        else:
            _check_syntax_error(testcase, statement, errtext, lineno, offset)
    testcase.assertEqual(warns, [])


def ignore_warnings(*, category):
    """Decorator to suppress warnings."""

    def decorator(test):
        @functools.wraps(test)
        def wrapper(self, *args, **kwargs):
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", category=category)
                return test(self, *args, **kwargs)

        return wrapper

    return decorator


class WarningsRecorder:
    """Convenience wrapper for warnings from catch_warnings(record=True)."""

    def __init__(self, warnings_list):
        self._warnings = warnings_list
        self._last = 0

    def __getattr__(self, attr):
        if len(self._warnings) > self._last:
            return getattr(self._warnings[-1], attr)
        if attr in warnings.WarningMessage._WARNING_DETAILS:
            return None
        raise AttributeError(f"{self!r} has no attribute {attr!r}")

    @property
    def warnings(self):
        return self._warnings[self._last :]

    def reset(self) -> None:
        self._last = len(self._warnings)


@contextlib.contextmanager
def check_warnings(*filters, **kwargs):
    """Context manager to silence warnings.

    Accept 2-tuples as positional arguments:
        ("message regexp", WarningCategory)
    """
    quiet = kwargs.get("quiet")
    if not filters:
        filters = (("", Warning),)
        if quiet is None:
            quiet = True
    return _filterwarnings(filters, quiet)


@contextlib.contextmanager
def check_no_warnings(
    testcase, message: str = "", category=Warning, force_gc: bool = False
):
    from test.support import gc_collect

    with warnings.catch_warnings(record=True) as warns:
        warnings.filterwarnings("always", message=message, category=category)
        yield
        if force_gc:
            gc_collect()
    testcase.assertEqual(warns, [])


@contextlib.contextmanager
def check_no_resource_warning(testcase):
    with check_no_warnings(testcase, category=ResourceWarning, force_gc=True):
        yield


@contextlib.contextmanager
def _filterwarnings(filters, quiet: bool = False):
    frame = None
    getframe = getattr(sys, "_getframe", None)
    if callable(getframe):
        try:
            frame = getframe(2)
        except Exception:
            frame = None
    if frame is not None:
        registry = frame.f_globals.get("__warningregistry__")
        if registry:
            registry.clear()
    wmod = sys.modules["warnings"]
    with wmod.catch_warnings(record=True) as w:
        wmod.simplefilter("always")
        yield WarningsRecorder(w)
    reraise = list(w)
    missing = []
    for msg, cat in filters:
        seen = False
        for warn in reraise[:]:
            warning = warn.message
            if re.match(msg, str(warning), re.I) and issubclass(warning.__class__, cat):
                seen = True
                reraise.remove(warn)
        if not seen and not quiet:
            missing.append((msg, cat.__name__))
    if reraise:
        raise AssertionError(f"unhandled warning {reraise[0]}")
    if missing:
        raise AssertionError(
            f"filter ({missing[0][0]!r}, {missing[0][1]}) did not catch any warning"
        )


@contextlib.contextmanager
def save_restore_warnings_filters():
    old_filters = warnings.filters[:]
    try:
        yield
    finally:
        warnings.filters[:] = old_filters


def _warn_about_deprecation():
    warnings.warn(
        "This is used in test_support test to ensure"
        " support.ignore_deprecations_from() works as expected."
        " You should not be seeing this.",
        DeprecationWarning,
        stacklevel=0,
    )


__all__ = [
    "WarningsRecorder",
    "_warn_about_deprecation",
    "check_no_resource_warning",
    "check_no_warnings",
    "check_syntax_warning",
    "check_warnings",
    "ignore_warnings",
    "import_deprecated",
    "save_restore_warnings_filters",
]
