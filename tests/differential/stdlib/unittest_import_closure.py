"""Purpose: differential coverage for the unittest package import closure.

`unittest.mock` imports `unittest.util` and `unittest.async_case` imports
`unittest.case` at module level, as CPython 3.12 does, so both must import and
share one TestCase/TestResult authority with the package.
"""

import asyncio
import unittest
import unittest.async_case
import unittest.case
import unittest.mock
import unittest.result
import unittest.util


print(unittest.TestCase is unittest.case.TestCase)
print(unittest.TestResult is unittest.result.TestResult)
print(unittest.IsolatedAsyncioTestCase is unittest.async_case.IsolatedAsyncioTestCase)
print(issubclass(unittest.IsolatedAsyncioTestCase, unittest.TestCase))
print(unittest.util.safe_repr("x" * 100, short=True))
print(unittest.mock.safe_repr is unittest.util.safe_repr)

events = []


class Probe(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        events.append("setUp")

    async def asyncSetUp(self):
        events.append("asyncSetUp")
        self.addAsyncCleanup(self.cleanup, "asyncCleanup")

    async def cleanup(self, label):
        events.append(label)

    async def test_awaits(self):
        events.append("test_awaits")
        self.assertEqual(await asyncio.sleep(0, result=3), 3)

    async def test_fails(self):
        events.append("test_fails")
        self.assertEqual(1, 2)

    async def asyncTearDown(self):
        events.append("asyncTearDown")

    def tearDown(self):
        events.append("tearDown")


suite = unittest.defaultTestLoader.loadTestsFromTestCase(Probe)
result = unittest.TestResult()
suite.run(result)
print(result.testsRun, len(result.failures), len(result.errors))
print(result.wasSuccessful())
print("AssertionError: 1 != 2" in result.failures[0][1])
print(events)

mocked = unittest.mock.AsyncMock(return_value=7)
print(asyncio.run(mocked(1)), mocked.await_count)
mocked.assert_awaited_once_with(1)
