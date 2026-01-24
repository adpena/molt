"""Minimal unittest subset for Molt (partial)."""

from __future__ import annotations

from typing import Any, Callable, Iterable
import re
import sys
import traceback
import types

__all__ = [
    "SkipTest",
    "skip",
    "skipIf",
    "skipUnless",
    "TestCase",
    "TestSuite",
    "TestResult",
    "TestLoader",
    "TextTestRunner",
    "defaultTestLoader",
    "main",
]


# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement full unittest runner, result reporting, and skip decorators.


class SkipTest(Exception):
    """Signal a skipped test."""


def _set_skip(target, reason: str):
    setattr(target, "__unittest_skip__", True)
    setattr(target, "__unittest_skip_why__", reason)
    return target


def skip(reason: str):
    def decorator(target):
        return _set_skip(target, reason)

    return decorator


def skipIf(condition: bool, reason: str):
    if condition:
        return skip(reason)

    def decorator(target):
        return target

    return decorator


def skipUnless(condition: bool, reason: str):
    if not condition:
        return skip(reason)

    def decorator(target):
        return target

    return decorator


class TestResult:
    def __init__(self) -> None:
        self.failures: list[tuple[Any, str]] = []
        self.errors: list[tuple[Any, str]] = []
        self.skipped: list[tuple[Any, str]] = []
        self.testsRun = 0

    def addFailure(self, test: Any, err: str) -> None:
        self.failures.append((test, err))

    def addError(self, test: Any, err: str) -> None:
        self.errors.append((test, err))

    def addSkip(self, test: Any, reason: str) -> None:
        self.skipped.append((test, reason))

    def wasSuccessful(self) -> bool:
        return not self.failures and not self.errors


def _exc_text() -> str:
    return traceback.format_exc()


class TestCase:
    def __init__(self, methodName: str = "runTest") -> None:
        self._testMethodName = methodName

    def setUp(self) -> None:
        return None

    def tearDown(self) -> None:
        return None

    def runTest(self) -> None:
        return None

    def run(self, result: TestResult | None = None) -> TestResult:
        if result is None or not hasattr(result, "testsRun"):
            result = TestResult()
        result.testsRun += 1
        try:
            method = getattr(self, self._testMethodName)
            if getattr(self, "__unittest_skip__", False):
                reason = getattr(self, "__unittest_skip_why__", "")
                result.addSkip(self, reason)
                return result
            if getattr(method, "__unittest_skip__", False):
                reason = getattr(method, "__unittest_skip_why__", "")
                result.addSkip(self, reason)
                return result
            func = getattr(method, "__func__", None)
            if func is not None and getattr(func, "__unittest_skip__", False):
                reason = getattr(func, "__unittest_skip_why__", "")
                result.addSkip(self, reason)
                return result
            class_func = getattr(self.__class__, self._testMethodName, None)
            if class_func is not None and getattr(
                class_func, "__unittest_skip__", False
            ):
                reason = getattr(class_func, "__unittest_skip_why__", "")
                result.addSkip(self, reason)
                return result
            self.setUp()
            method()
            self.tearDown()
        except SkipTest as exc:
            result.addSkip(self, str(exc))
        except AssertionError:
            result.addFailure(self, _exc_text())
        except Exception:
            result.addError(self, _exc_text())
        return result

    def fail(self, msg: str | None = None) -> None:
        raise AssertionError(msg or "Test failed")

    def assertTrue(self, expr: Any, msg: str | None = None) -> None:
        if not expr:
            self.fail(msg or "expression is not true")

    def assertFalse(self, expr: Any, msg: str | None = None) -> None:
        if expr:
            self.fail(msg or "expression is not false")

    def assertEqual(self, a: Any, b: Any, msg: str | None = None) -> None:
        if a != b:
            self.fail(msg or f"{a!r} != {b!r}")

    def assertNotEqual(self, a: Any, b: Any, msg: str | None = None) -> None:
        if a == b:
            self.fail(msg or f"{a!r} == {b!r}")

    def assertAlmostEqual(
        self,
        first: Any,
        second: Any,
        places: int = 7,
        msg: str | None = None,
        delta: float | None = None,
    ) -> None:
        if delta is not None:
            diff = abs(first - second)
            if diff > delta:
                self.fail(
                    msg
                    or f"{first!r} != {second!r} within {delta!r} delta ({diff!r} difference)"
                )
            return None
        diff = round(abs(first - second), places)
        if diff != 0:
            self.fail(
                msg
                or f"{first!r} != {second!r} within {places} places ({diff!r} difference)"
            )
        return None

    def assertIs(self, a: Any, b: Any, msg: str | None = None) -> None:
        if a is not b:
            self.fail(msg or f"{a!r} is not {b!r}")

    def assertIsNot(self, a: Any, b: Any, msg: str | None = None) -> None:
        if a is b:
            self.fail(msg or f"{a!r} is {b!r}")

    def assertIsInstance(self, obj: Any, cls: Any, msg: str | None = None) -> None:
        if not isinstance(obj, cls):
            name = getattr(cls, "__name__", repr(cls))
            self.fail(msg or f"{obj!r} is not an instance of {name}")

    def assertIsNone(self, obj: Any, msg: str | None = None) -> None:
        if obj is not None:
            self.fail(msg or f"{obj!r} is not None")

    def assertIsNotNone(self, obj: Any, msg: str | None = None) -> None:
        if obj is None:
            self.fail(msg or "unexpected None")

    def assertIn(self, member: Any, container: Any, msg: str | None = None) -> None:
        if member not in container:
            self.fail(msg or f"{member!r} not found in {container!r}")

    def assertNotIn(self, member: Any, container: Any, msg: str | None = None) -> None:
        if member in container:
            self.fail(msg or f"{member!r} unexpectedly found in {container!r}")

    def assertRaises(
        self,
        exc_type: type[BaseException],
        func: Callable[..., Any] | None = None,
        *args: Any,
        **kwargs: Any,
    ):
        if func is None:
            return _AssertRaisesContext(self, exc_type)
        try:
            func(*args, **kwargs)
        except exc_type:
            return None
        except BaseException as exc:
            self.fail(f"Expected {exc_type.__name__}, got {type(exc).__name__}")
        self.fail(f"{exc_type.__name__} not raised")

    def assertRaisesRegex(
        self,
        exc_type: type[BaseException],
        pattern: str,
        func: Callable[..., Any] | None = None,
        *args: Any,
        **kwargs: Any,
    ):
        if func is None:
            return _AssertRaisesRegexContext(self, exc_type, pattern)
        try:
            func(*args, **kwargs)
        except exc_type as exc:
            if re.search(pattern, str(exc)) is None:
                self.fail(f"Exception message does not match {pattern!r}")
            return None
        except BaseException as exc:
            self.fail(f"Expected {exc_type.__name__}, got {type(exc).__name__}")
        self.fail(f"{exc_type.__name__} not raised")

    def assertRaisesRegexp(
        self,
        exc_type: type[BaseException],
        pattern: str,
        func: Callable[..., Any] | None = None,
        *args: Any,
        **kwargs: Any,
    ):
        return self.assertRaisesRegex(exc_type, pattern, func, *args, **kwargs)

    def assertRegex(
        self, text: str, expected_regex: str, msg: str | None = None
    ) -> None:
        if re.search(expected_regex, text) is None:
            self.fail(msg or f"{expected_regex!r} not found in {text!r}")

    def assertNotRegex(
        self, text: str, expected_regex: str, msg: str | None = None
    ) -> None:
        if re.search(expected_regex, text) is not None:
            self.fail(msg or f"{expected_regex!r} found in {text!r}")


class _AssertRaisesContext:
    def __init__(self, case: TestCase, exc_type: type[BaseException]) -> None:
        self._case = case
        self._exc_type = exc_type

    def __enter__(self) -> _AssertRaisesContext:
        return self

    def __exit__(self, exc_type, exc, tb) -> bool:
        if exc_type is None:
            self._case.fail(f"{self._exc_type.__name__} not raised")
        if not issubclass(exc_type, self._exc_type):
            return False
        return True


class _AssertRaisesRegexContext:
    def __init__(
        self, case: TestCase, exc_type: type[BaseException], pattern: str
    ) -> None:
        self._case = case
        self._exc_type = exc_type
        self._pattern = pattern

    def __enter__(self) -> _AssertRaisesRegexContext:
        return self

    def __exit__(self, exc_type, exc, tb) -> bool:
        if exc_type is None:
            self._case.fail(f"{self._exc_type.__name__} not raised")
        if not issubclass(exc_type, self._exc_type):
            return False
        if re.search(self._pattern, str(exc)) is None:
            self._case.fail(f"Exception message does not match {self._pattern!r}")
        return True


class TestSuite:
    def __init__(self, tests: Iterable[Any] | None = None) -> None:
        self._tests: list[Any] = list(tests) if tests else []

    def addTest(self, test: Any) -> None:
        self._tests.append(test)

    def addTests(self, tests: Iterable[Any]) -> None:
        for test in tests:
            self.addTest(test)

    def run(self, result: TestResult | None = None) -> TestResult:
        if result is None or not hasattr(result, "testsRun"):
            result = TestResult()
        for test in list(self._tests):
            if isinstance(test, TestSuite):
                test.run(result)
            else:
                test.run(result)
        return result


class TestLoader:
    def getTestCaseNames(self, case: type[TestCase]) -> list[str]:
        return [name for name in dir(case) if name.startswith("test")]

    def loadTestsFromTestCase(self, case: type[TestCase]) -> TestSuite:
        suite = TestSuite()
        for name in self.getTestCaseNames(case):
            suite.addTest(case(name))
        return suite

    def loadTestsFromModule(self, module) -> TestSuite:
        suite = TestSuite()
        for value in module.__dict__.values():
            if (
                isinstance(value, type)
                and issubclass(value, TestCase)
                and value is not TestCase
            ):
                suite.addTest(self.loadTestsFromTestCase(value))
        return suite


class TextTestRunner:
    def __init__(self) -> None:
        return None

    def run(self, test: TestSuite) -> TestResult:
        result = TestResult()
        test.run(result)
        if result.failures or result.errors:
            for case, err in result.failures:
                print(f"FAIL: {case!r}")
                print(err)
            for case, err in result.errors:
                print(f"ERROR: {case!r}")
                print(err)
        return result


defaultTestLoader = TestLoader()


def main(module: str | None = None, exit: bool = True) -> TestResult:
    if module is None:
        mod = sys.modules.get("__main__")
        if mod is None:
            try:
                frame = sys._getframe(1)
            except (AttributeError, ValueError):
                frame = None
            if frame is not None:
                mod = types.ModuleType(frame.f_globals.get("__name__", "__main__"))
                mod.__dict__.update(frame.f_globals)
    else:
        mod = sys.modules.get(module)
    if mod is None:
        raise RuntimeError("unittest.main() requires a module")
    suite = defaultTestLoader.loadTestsFromModule(mod)
    load_tests = getattr(mod, "load_tests", None)
    if callable(load_tests):
        suite = load_tests(defaultTestLoader, suite, None)
    result = TextTestRunner().run(suite)
    if exit:
        raise SystemExit(0 if result.wasSuccessful() else 1)
    return result
