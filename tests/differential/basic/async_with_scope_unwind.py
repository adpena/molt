"""Sync/async context exits and finally transfers share scoped exception custody."""

import asyncio
import sys


events = []


class Manager:
    def __init__(self, name, value=None, suppress=False, fail=False):
        self.name = name
        self.value = value
        self.suppress = suppress
        self.fail = fail

    async def __aenter__(self):
        events.append((self.name, "enter"))
        await asyncio.sleep(0)
        return self.value

    async def __aexit__(self, kind, value, traceback):
        events.append((self.name, "exit", None if kind is None else kind.__name__))
        if value is not None:
            assert sys.exception() is value
            assert traceback is value.__traceback__
            assert traceback is not None
        await asyncio.sleep(0)
        if value is not None:
            assert sys.exception() is value
        if self.fail:
            raise RuntimeError("exit")
        return self.suppress


async def return_nested():
    async with Manager("outer"), Manager("inner"):
        return [41, 42]


async def loops():
    for i in range(3):
        async with Manager("loop", i) as value:
            if value == 0:
                continue
            if value == 1:
                break
    events.append(("loop", "done"))


async def assignment():
    async with Manager("assignment", (1,), suppress=True) as (left, right):
        raise AssertionError("unreachable")
    events.append(("assignment", "suppressed"))


async def failed_exit():
    try:
        async with Manager("failed", fail=True):
            raise ValueError("body")
    except RuntimeError as error:
        events.append(("failed", type(error.__context__).__name__))


async def finally_after_failed_exit():
    try:
        try:
            raise ValueError("handled")
        except ValueError:
            async with Manager("handler-failed", fail=True):
                return 1
        finally:
            events.append(("handler-failed", "finally"))
    except RuntimeError:
        events.append(("handler-failed", "caught"))


async def outer_suppresses_finally():
    async with Manager("finally-outer", suppress=True):
        try:
            return 1
        finally:
            raise RuntimeError("finally")
    return 2


async def finally_return_override():
    async with Manager("override-outer"):
        try:
            return 1
        finally:
            async with Manager("override-inner"):
                return 2


class MutatingManager(Manager):
    async def __aenter__(self):
        type(self).__aexit__ = replacement_exit
        return await super().__aenter__()


async def replacement_exit(self, kind, value, traceback):
    raise AssertionError("exit must have been captured before entry")


async def captured_exit():
    async with MutatingManager("captured"):
        pass


class HandlerManager(Manager):
    async def __aexit__(self, kind, value, traceback):
        assert kind is None
        assert isinstance(sys.exception(), LookupError)
        await asyncio.sleep(0)
        assert isinstance(sys.exception(), LookupError)
        events.append((self.name, "outer-handler"))


async def in_handler():
    try:
        raise LookupError("outer")
    except LookupError as error:
        for i in range(2):
            async with HandlerManager("handler-loop"):
                if i == 0:
                    continue
                break
        assert sys.exception() is error
        async with HandlerManager("handler-return"):
            return str(error)


class Suppression:
    def __init__(self, mode, error, fail=False):
        self.mode = mode
        self.error = error
        self.fail = fail

    def __bool__(self):
        assert self.error is not None, "normal exit must not truth-test its result"
        assert sys.exception() is self.error
        events.append((self.mode + "-truth", type(sys.exception()).__name__))
        if self.fail:
            raise TypeError("truth")
        return True


class SyncManager:
    def __init__(self, fail=False):
        self.fail = fail

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        if value is not None:
            assert value.args == ("body",)
        return Suppression("sync", value, self.fail)


class AsyncTruthManager:
    def __init__(self, fail=False):
        self.fail = fail

    async def __aenter__(self):
        await asyncio.sleep(0)
        return self

    async def __aexit__(self, kind, value, traceback):
        await asyncio.sleep(0)
        return Suppression("async", value, self.fail)


def sync_suppression():
    with SyncManager():
        pass
    with SyncManager():
        raise ValueError("body")
    try:
        with SyncManager(fail=True):
            raise ValueError("body")
    except TypeError as error:
        assert isinstance(error.__context__, ValueError)
        events.append(("sync-truth-error", type(error.__context__).__name__))


async def async_suppression():
    async with AsyncTruthManager():
        pass
    async with AsyncTruthManager():
        raise ValueError("body")
    try:
        async with AsyncTruthManager(fail=True):
            raise ValueError("body")
    except TypeError as error:
        assert isinstance(error.__context__, ValueError)
        events.append(("async-truth-error", type(error.__context__).__name__))


class FailedEntry:
    def __enter__(self):
        raise LookupError("entry")

    def __exit__(self, kind, value, traceback):
        raise AssertionError("exit called after failed entry")

    async def __aenter__(self):
        await asyncio.sleep(0)
        raise LookupError("entry")

    async def __aexit__(self, kind, value, traceback):
        raise AssertionError("async exit called after failed entry")


async def failed_entries():
    try:
        with FailedEntry():
            raise AssertionError("body entered")
    except LookupError as error:
        events.append(("sync-entry", error.args[0]))
    try:
        async with FailedEntry():
            raise AssertionError("body entered")
    except LookupError as error:
        events.append(("async-entry", error.args[0]))
    assert sys.exception() is None


def failed_entry_in_finally():
    try:
        try:
            raise KeyError("old")
        except KeyError:
            return 1
    finally:
        with FailedEntry():
            raise AssertionError("body entered")


async def failed_async_entry_in_finally():
    try:
        try:
            raise KeyError("old")
        except KeyError:
            return 1
    finally:
        async with FailedEntry():
            raise AssertionError("body entered")


async def finally_loop_transfer(continue_outer):
    seen = []
    for outer in (0, 1):
        try:
            for inner in [0]:
                return ("unreachable", inner)
        finally:
            await asyncio.sleep(0)
            seen.append(outer)
            if continue_outer:
                continue
            break
    else:
        seen.append("else")
    return seen


async def loop_items():
    for item in (0, 1):
        yield item


async def async_for_finally_transfer():
    seen = []
    async for outer in loop_items():
        try:
            for inner in [0]:
                return ("unreachable", inner)
        finally:
            await asyncio.sleep(0)
            seen.append(outer)
            continue
    else:
        seen.append("else")
    return seen


def finally_capture():
    try:
        raise ValueError("saved")
    except ValueError as error:  # noqa: F841 - the returned closure tests deletion.
        try:
            return None
        finally:
            return lambda: error  # noqa: F821 - read only after handler cleanup.


def finally_reraise():
    try:
        raise ValueError("outer")
    except ValueError:
        try:
            pass
        finally:
            raise


def escaped_finally():
    try:
        try:
            raise ValueError("old")
        finally:
            return 1
    finally:
        raise


def live_finally():
    try:
        raise ValueError("live")
    finally:
        try:
            return 1
        finally:
            raise


def finally_cases():
    callback = finally_capture()
    try:
        callback()
    except NameError:
        events.append(("finally-capture", "deleted"))
    else:
        raise AssertionError("escaped handler binding survived")
    for name, function, expected in (
        ("finally-reraise", finally_reraise, ValueError),
        ("escaped-finally", escaped_finally, RuntimeError),
        ("live-finally", live_finally, ValueError),
    ):
        try:
            function()
        except expected as error:
            events.append((name, type(error).__name__))
        else:
            raise AssertionError("missing reraise")


async def main():
    assert await return_nested() == [41, 42]
    await loops()
    await assignment()
    await failed_exit()
    await finally_after_failed_exit()
    assert await outer_suppresses_finally() == 2
    assert await finally_return_override() == 2
    await captured_exit()
    assert await in_handler() == "outer"
    sync_suppression()
    await async_suppression()
    await failed_entries()
    assert await finally_loop_transfer(False) == [0]
    assert await finally_loop_transfer(True) == [0, 1, "else"]
    assert await async_for_finally_transfer() == [0, 1, "else"]
    events.append(("async-finally-loop", "passed"))
    try:
        failed_entry_in_finally()
    except LookupError as error:
        events.append(("sync-finally-entry", error.args[0]))
    else:
        raise AssertionError("failed entry did not override return")
    try:
        await failed_async_entry_in_finally()
    except LookupError as error:
        events.append(("async-finally-entry", error.args[0]))
    else:
        raise AssertionError("failed async entry did not override return")
    finally_cases()
    assert sys.exception() is None


asyncio.run(main())
for event in events:
    print(event)
