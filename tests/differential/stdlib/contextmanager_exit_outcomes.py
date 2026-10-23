"""What a generator context manager's __exit__ does with the exception that
throw() raised (CPython's _GeneratorContextManager.__exit__ and
_AsyncGeneratorContextManager.__aexit__).

Reduced from unittest's _Outcome.testPartExecutor: when the handler of the
thrown exception raises a new exception, the new exception propagates with the
original as its context. It must not be dropped in favor of the original."""

import asyncio
import contextlib
import sys


def show(label, body):
    try:
        print(label, "returned", body())
    except BaseException as exc:
        context = exc.__context__
        print(
            label,
            "raised",
            type(exc).__name__,
            exc,
            "| context",
            type(context).__name__ if context is not None else None,
        )


class Skip(Exception):
    pass


class Stop(Exception):
    pass


def report(exc_info):
    # Stands in for a result object whose formatting fails.
    raise AttributeError("report failed for " + exc_info[0].__name__)


@contextlib.contextmanager
def part_executor():
    try:
        yield
    except KeyboardInterrupt:
        raise
    except Skip:
        print("  skipped")
    except Stop:
        pass
    except:  # noqa: E722 - the shape under test
        report(sys.exc_info())


def handler_raises():
    with part_executor():
        raise AssertionError("test failed")
    return "suppressed"


def handler_skips():
    with part_executor():
        raise Skip("why")
    return "suppressed"


@contextlib.contextmanager
def passthrough():
    yield


def unhandled_original():
    try:
        with passthrough():
            raise KeyError("original")
    except KeyError as exc:
        names = []
        tb = exc.__traceback__
        while tb is not None:
            names.append(tb.tb_frame.f_code.co_name)
            tb = tb.tb_next
        # The generator frame of passthrough() is not in the traceback.
        return names


def stop_iteration_body():
    with passthrough():
        raise StopIteration("inside")


@contextlib.contextmanager
def converts_stop():
    try:
        yield
    except StopIteration as exc:
        raise RuntimeError("converted") from exc


def stop_iteration_converted():
    with converts_stop():
        raise StopIteration("inside")


@contextlib.contextmanager
def yields_again(log):
    try:
        yield
    except ValueError:
        yield
    finally:
        log.append("closed")


def did_not_stop_after_throw():
    log = []
    try:
        with yields_again(log):
            raise ValueError("x")
    except RuntimeError as exc:
        return str(exc), log


@contextlib.contextmanager
def yields_twice(log):
    try:
        yield
        yield
    finally:
        log.append("closed")


def did_not_stop():
    log = []
    try:
        with yields_twice(log):
            pass
    except RuntimeError as exc:
        return str(exc), log


@contextlib.contextmanager
def never_yields():
    if False:
        yield


def did_not_yield():
    with never_yields():
        pass


@contextlib.asynccontextmanager
async def async_part_executor():
    try:
        yield
    except Skip:
        print("  async skipped")
    except:  # noqa: E722 - the shape under test
        report(sys.exc_info())


async def async_handler_raises():
    async with async_part_executor():
        raise AssertionError("async test failed")
    return "suppressed"


@contextlib.asynccontextmanager
async def async_passthrough():
    yield


async def async_unhandled():
    async with async_passthrough():
        raise KeyError("async original")


@contextlib.asynccontextmanager
async def async_yields_twice():
    yield
    yield


async def async_did_not_stop():
    async with async_yields_twice():
        pass


@contextlib.asynccontextmanager
async def async_never_yields():
    if False:
        yield


async def async_did_not_yield():
    async with async_never_yields():
        pass


show("handler raises", handler_raises)
show("handler skips", handler_skips)
show("unhandled original", unhandled_original)
show("StopIteration in body", stop_iteration_body)
show("StopIteration converted", stop_iteration_converted)
show("did not stop after throw", did_not_stop_after_throw)
show("did not stop", did_not_stop)
show("did not yield", did_not_yield)
show("async handler raises", lambda: asyncio.run(async_handler_raises()))
show("async unhandled", lambda: asyncio.run(async_unhandled()))
show("async did not stop", lambda: asyncio.run(async_did_not_stop()))
show("async did not yield", lambda: asyncio.run(async_did_not_yield()))
