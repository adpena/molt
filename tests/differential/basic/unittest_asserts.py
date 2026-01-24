"""Purpose: differential coverage for unittest asserts."""

import unittest


case = unittest.TestCase()


def expect_ok(name, func):
    try:
        func()
        print(f"{name}:ok")
    except Exception as exc:
        print(f"{name}:err:{type(exc).__name__}")


def expect_raises(name, func):
    try:
        func()
        print(f"{name}:no-raise")
    except AssertionError:
        print(f"{name}:raised")
    except Exception as exc:
        print(f"{name}:err:{type(exc).__name__}")


expect_ok("assertIs", lambda: case.assertIs(case, case))
expect_raises("assertIs_fail", lambda: case.assertIs(1, 2))
expect_ok("assertIsNot", lambda: case.assertIsNot(1, 2))
expect_raises("assertIsNot_fail", lambda: case.assertIsNot(1, 1))
expect_ok("assertIn", lambda: case.assertIn(2, [1, 2, 3]))
expect_raises("assertIn_fail", lambda: case.assertIn(4, [1, 2, 3]))
expect_ok("assertNotIn", lambda: case.assertNotIn(4, [1, 2, 3]))
expect_raises("assertNotIn_fail", lambda: case.assertNotIn(2, [1, 2, 3]))
expect_ok("assertIsNone", lambda: case.assertIsNone(None))
expect_raises("assertIsNone_fail", lambda: case.assertIsNone(1))
expect_ok("assertIsNotNone", lambda: case.assertIsNotNone(1))
expect_raises("assertIsNotNone_fail", lambda: case.assertIsNotNone(None))
expect_ok("assertNotEqual", lambda: case.assertNotEqual(1, 2))
expect_raises("assertNotEqual_fail", lambda: case.assertNotEqual(1, 1))
expect_ok("assertAlmostEqual", lambda: case.assertAlmostEqual(1.0, 1.0))
expect_raises(
    "assertAlmostEqual_fail", lambda: case.assertAlmostEqual(1.0, 1.2, places=3)
)


def blockingioerror_context():
    err = BlockingIOError("a", "b", 3)
    case.assertEqual(err.characters_written, 3)
    err.characters_written = 5
    case.assertEqual(err.characters_written, 5)
    del err.characters_written
    ctx = case.assertRaises(AttributeError)
    with ctx:
        _ = err.characters_written


expect_ok("assertRaises_ctx_blockingio", blockingioerror_context)


def assert_raises_regex_context():
    ctx = case.assertRaisesRegex(TypeError, "list indices must be integers or slices")
    target = []
    with ctx:
        _ = target["a"]


expect_ok("assertRaisesRegex_ctx", assert_raises_regex_context)
