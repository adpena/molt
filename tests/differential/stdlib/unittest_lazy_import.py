"""Deferred package availability must not eagerly initialize its dependencies."""

import sys
import unittest

print("bare", "asyncio" in sys.modules, "unittest.async_case" in sys.modules)
case = unittest.IsolatedAsyncioTestCase
print("selected", case.__name__, issubclass(case, unittest.TestCase))
print("loaded", "asyncio" in sys.modules, "unittest.async_case" in sys.modules)
print("identity", case is unittest.IsolatedAsyncioTestCase)
