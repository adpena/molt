"""Purpose: differential coverage for test unittest basic."""

import unittest


class Sample(unittest.TestCase):
    def test_ok(self) -> None:
        return None


def test_testcase_constructor_accepts_method_name() -> None:
    case = Sample("test_ok")
    assert case._testMethodName == "test_ok"


def test_skip_decorator_on_method_is_honored() -> None:
    class SkipCase(unittest.TestCase):
        @unittest.skipUnless(False, "nope")
        def test_skip(self) -> None:
            raise AssertionError("should not run")

    suite = unittest.defaultTestLoader.loadTestsFromTestCase(SkipCase)
    result = unittest.TestResult()
    suite.run(result)
    assert result.testsRun == 1
    assert result.skipped
