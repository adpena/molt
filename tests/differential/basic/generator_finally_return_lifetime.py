"""Pending return owners survive cleanup and die when their transfer is canceled."""

events = []


class Token:
    def __init__(self, name):
        if name == "replacement":
            assert "drop:pending" not in events, events
        self.name = name

    def __del__(self):
        events.append("drop:" + self.name)


class Pause:
    def __await__(self):
        # A bare suspension can be driven without asyncio or a scheduler.
        yield None


class SyncManager:
    def __init__(self, name, alive, fail=False):
        self.name = name
        self.alive = alive
        self.fail = fail

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        assert ("drop:pending" not in events) == self.alive, events
        events.append(
            "exit:" + self.name + ":" + ("None" if kind is None else kind.__name__)
        )
        if self.fail:
            raise LookupError("manager")
        return False


class AsyncManager(SyncManager):
    async def __aenter__(self):
        return self

    async def __aexit__(self, kind, value, traceback):
        assert ("drop:pending" not in events) == self.alive, events
        await Pause()
        assert ("drop:pending" not in events) == self.alive, events
        events.append(
            "exit:" + self.name + ":" + ("None" if kind is None else kind.__name__)
        )
        if self.fail:
            raise LookupError("manager")
        return False


def finish_generator(generator):
    try:
        next(generator)
    except StopIteration as finished:
        return finished.value
    raise AssertionError("unexpected extra yield")


def finish_coroutine(coroutine):
    while True:
        try:
            assert coroutine.send(None) is None
        except StopIteration as finished:
            return finished.value


def failing_value():
    raise LookupError("return expression")


def generator_transfer(mode):
    for outer in (0,):
        try:
            return Token("pending")
        finally:
            yield "cleanup"
            assert events == [], events
            if mode == "continue":
                continue
            if mode == "break":
                break
            if mode == "return":
                return Token("replacement")
            if mode == "return-error":
                return failing_value()
            if mode == "raise":
                raise LookupError("finally")
    yield "after"


for mode in ("continue", "break"):
    events.clear()
    generator = generator_transfer(mode)
    assert next(generator) == "cleanup"
    assert events == [], events
    assert next(generator) == "after"
    # The generator is suspended and remains strongly referenced here.
    assert events == ["drop:pending"], events
    assert finish_generator(generator) is None

for mode, result_name in (("normal", "pending"), ("return", "replacement")):
    events.clear()
    generator = generator_transfer(mode)
    assert next(generator) == "cleanup"
    result = finish_generator(generator)
    assert result.name == result_name
    expected = [] if mode == "normal" else ["drop:pending"]
    assert events == expected, events
    del result
    assert events == expected + ["drop:" + result_name], events

for mode in ("raise", "return-error"):
    events.clear()
    generator = generator_transfer(mode)
    assert next(generator) == "cleanup"
    try:
        next(generator)
    except LookupError:
        assert events == ["drop:pending"], events
    else:
        raise AssertionError("finally error was lost")
print("generator transfers passed")


def generator_retained_return():
    try:
        return Token("pending")
    finally:
        for inner in (0, 1):
            if inner == 0:
                continue
            break
        yield "retained"
        try:
            return failing_value()
        except LookupError:
            events.append("caught")
        yield "retained-after-error"


events.clear()
generator = generator_retained_return()
assert next(generator) == "retained"
assert events == [], events
assert next(generator) == "retained-after-error"
assert events == ["caught"], events
result = finish_generator(generator)
assert result.name == "pending"
assert events == ["caught"], events
del result
assert events == ["caught", "drop:pending"], events
print("generator retained return passed")


def generator_manager_order(mode):
    with SyncManager("outer", False):
        for outer in (0,):
            try:
                return Token("pending")
            finally:
                with SyncManager("inner", True):
                    yield "cleanup"
                if mode == "continue":
                    continue
                break
    yield "after"


def generator_manager_failure():
    with SyncManager("outer", False):
        with SyncManager("failing", True, fail=True):
            yield "body"
            return Token("pending")


def generator_manager_success():
    with SyncManager("success", True):
        try:
            return Token("pending")
        finally:
            yield "cleanup"


for mode in ("continue", "break"):
    events.clear()
    generator = generator_manager_order(mode)
    assert next(generator) == "cleanup"
    assert next(generator) == "after"
    assert events == ["exit:inner:None", "drop:pending", "exit:outer:None"], events
    assert finish_generator(generator) is None

events.clear()
generator = generator_manager_failure()
assert next(generator) == "body"
try:
    next(generator)
except LookupError:
    assert events == ["exit:failing:None", "drop:pending", "exit:outer:LookupError"], (
        events
    )
else:
    raise AssertionError("manager error was lost")

events.clear()
generator = generator_manager_success()
assert next(generator) == "cleanup"
result = finish_generator(generator)
assert result.name == "pending"
assert events == ["exit:success:None"], events
del result
assert events == ["exit:success:None", "drop:pending"], events
print("generator manager ordering passed")


async def coroutine_transfer(mode):
    for outer in (0,):
        try:
            return Token("pending")
        finally:
            await Pause()
            assert events == [], events
            if mode == "continue":
                continue
            if mode == "break":
                break
            if mode == "return":
                return Token("replacement")
            if mode == "return-error":
                return failing_value()
            if mode == "raise":
                raise LookupError("finally")
    assert events == ["drop:pending"], events
    await Pause()
    return "after"


async def coroutine_retained_return():
    try:
        return Token("pending")
    finally:
        for inner in (0, 1):
            if inner == 0:
                continue
            break
        await Pause()
        assert events == [], events
        try:
            return failing_value()
        except LookupError:
            events.append("caught")
        await Pause()
        assert events == ["caught"], events


for mode in ("continue", "break"):
    events.clear()
    coroutine = coroutine_transfer(mode)
    assert finish_coroutine(coroutine) == "after"
    assert events == ["drop:pending"], events

for mode, result_name in (("normal", "pending"), ("return", "replacement")):
    events.clear()
    coroutine = coroutine_transfer(mode)
    result = finish_coroutine(coroutine)
    assert result.name == result_name
    expected = [] if mode == "normal" else ["drop:pending"]
    assert events == expected, events
    del result
    assert events == expected + ["drop:" + result_name], events

for mode in ("raise", "return-error"):
    events.clear()
    coroutine = coroutine_transfer(mode)
    try:
        finish_coroutine(coroutine)
    except LookupError:
        assert events == ["drop:pending"], events
    else:
        raise AssertionError("coroutine finally error was lost")

events.clear()
coroutine = coroutine_retained_return()
result = finish_coroutine(coroutine)
assert result.name == "pending"
assert events == ["caught"], events
del result
assert events == ["caught", "drop:pending"], events
print("coroutine transfers passed")


async def coroutine_async_manager(mode):
    with SyncManager("outer", mode == "normal"):
        for outer in (0,):
            try:
                return Token("pending")
            finally:
                async with AsyncManager("inner", True):
                    await Pause()
                if mode == "continue":
                    continue
                if mode == "break":
                    break
    await Pause()
    return "after"


async def coroutine_manager_failure(asynchronous):
    async with AsyncManager("outer", False):
        if asynchronous:
            async with AsyncManager("failing", True, fail=True):
                return Token("pending")
        else:
            with SyncManager("failing", True, fail=True):
                return Token("pending")


async def coroutine_manager_success(asynchronous):
    if asynchronous:
        async with AsyncManager("success", True):
            return Token("pending")
    else:
        with SyncManager("success", True):
            await Pause()
            return Token("pending")


for mode in ("continue", "break"):
    events.clear()
    coroutine = coroutine_async_manager(mode)
    assert finish_coroutine(coroutine) == "after"
    assert events == ["exit:inner:None", "drop:pending", "exit:outer:None"], events

events.clear()
coroutine = coroutine_async_manager("normal")
result = finish_coroutine(coroutine)
assert result.name == "pending"
assert events == ["exit:inner:None", "exit:outer:None"], events
del result
assert events == ["exit:inner:None", "exit:outer:None", "drop:pending"], events

for asynchronous in (False, True):
    events.clear()
    coroutine = coroutine_manager_failure(asynchronous)
    try:
        finish_coroutine(coroutine)
    except LookupError:
        assert events == [
            "exit:failing:None",
            "drop:pending",
            "exit:outer:LookupError",
        ], events
    else:
        raise AssertionError("coroutine manager error was lost")

    events.clear()
    coroutine = coroutine_manager_success(asynchronous)
    result = finish_coroutine(coroutine)
    assert result.name == "pending"
    assert events == ["exit:success:None"], events
    del result
    assert events == ["exit:success:None", "drop:pending"], events
print("coroutine manager ordering passed")
