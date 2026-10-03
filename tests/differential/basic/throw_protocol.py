"""Independent CPython oracle for shared generator/coroutine throw semantics."""

import inspect
import sys
import types
import warnings

warnings.simplefilter("ignore", DeprecationWarning)
events = []


def capture():
    try:
        yield "ready"
    except ValueError as error:
        yield error
    finally:
        events.append("closed")


def caught(arguments):
    gen = capture()
    assert next(gen) == "ready"
    error = gen.throw(*arguments)
    assert type(error) is ValueError
    result = error.args
    gen.close()
    return result


print("class", caught((ValueError,)))
print("separate-value", caught((ValueError, "payload")))
print("tuple-value", caught((ValueError, ("alpha", 7))))
same = ValueError("same")
gen = capture()
next(gen)
assert gen.throw(ValueError, same) is same
gen.close()
print("instance-value-identity", True)

try:
    raise ValueError("traceback")
except ValueError as error:
    saved = error
    traceback = error.__traceback__

gen = capture()
next(gen)
assert gen.throw(saved, None, traceback) is saved
cursor = saved.__traceback__
while cursor is not None and cursor is not traceback:
    cursor = cursor.tb_next
assert cursor is traceback
gen.close()
print("traceback-identity", True)

for label, arguments in (
    ("none", ()),
    ("too-many", (ValueError, None, None, None)),
    ("bad-type", (42,)),
    ("tuple-type", ((ValueError,),)),
    ("instance-separate", (ValueError(), "bad")),
    ("bad-traceback", (ValueError, None, 42)),
):
    gen = capture()
    next(gen)
    try:
        gen.throw(*arguments)
    except TypeError:
        print("invalid", label, inspect.getgeneratorstate(gen))
    else:
        raise AssertionError(label)
    gen.close()

gen = capture()
next(gen)
try:
    gen.throw(typ=ValueError)
except TypeError:
    print("keyword", inspect.getgeneratorstate(gen))
else:
    raise AssertionError("keyword")
gen.close()


class Delegate:
    def __iter__(self):
        return self

    def __await__(self):
        return self

    def __next__(self):
        return "delegated"

    def throw(self, *arguments):
        events.append(("delegated-args", len(arguments), arguments[0] is ValueError))
        assert arguments == (ValueError, "raw", traceback)
        raise StopIteration(97)


def delegated_generator():
    return (yield from Delegate())


async def delegated_coroutine():
    return await Delegate()


for factory in (delegated_generator, delegated_coroutine):
    value = factory()
    assert value.send(None) == "delegated"
    try:
        value.throw(ValueError, "raw", traceback)
    except StopIteration as result:
        assert result.value == 97
        print("delegation", factory.__name__, result.value)
    else:
        raise AssertionError("delegation did not finish")


@types.coroutine
def pause():
    yield "pause"


async def catches():
    try:
        await pause()
    except ValueError as error:
        return error.args


value = catches()
assert value.send(None) == "pause"
try:
    value.throw(ValueError, ("async", 23), None)
except StopIteration as result:
    assert result.value == ("async", 23)
    print("coroutine", result.value)
else:
    raise AssertionError("coroutine did not finish")

with warnings.catch_warnings(record=True) as recorded:
    warnings.simplefilter("always", DeprecationWarning)
    gen = capture()
    next(gen)
    gen.throw(ValueError, "warning")
    gen.close()
    assert recorded and all(item.category is DeprecationWarning for item in recorded)
    print("deprecated-signature", True)

print("done", events[-2:])


def returns_on_close():
    try:
        yield "ready"
    except GeneratorExit:
        return 42


gen = returns_on_close()
next(gen)
result = gen.close()
assert result == (42 if sys.version_info >= (3, 13) else None)
assert gen.close() is None
print("generator-close-return", result)

close_error = ValueError("delegate-close")


class CloseFailure(Delegate):
    def close(self):
        events.append("close-called")
        raise close_error


def catches_close_failure():
    try:
        yield from CloseFailure()
    except ValueError as error:
        assert error is close_error
        events.append("close-caught")
        return 65
    finally:
        events.append("close-finally")


gen = catches_close_failure()
next(gen)
result = gen.close()
assert result == (65 if sys.version_info >= (3, 13) else None)
assert events[-3:] == ["close-called", "close-caught", "close-finally"]
print("delegate-close-handler", result, events[-3:])


class CloseStopIteration(Delegate):
    def close(self):
        raise StopIteration("close-failure")


def close_stop_generator():
    return (yield from CloseStopIteration())


async def close_stop_coroutine():
    return await CloseStopIteration()


for factory in (close_stop_generator, close_stop_coroutine):
    for operation in ("close", "throw"):
        value = factory()
        assert value.send(None) == "delegated"
        if operation == "close":
            result = value.close()
            assert result == ("close-failure" if sys.version_info >= (3, 13) else None)
        else:
            try:
                value.throw(GeneratorExit)
            except StopIteration as finished:
                assert finished.value == "close-failure"
                assert finished.__cause__ is None
                assert finished.__context__ is None
            else:
                raise AssertionError("delegate close did not exhaust")
        print("delegate-close-stopiteration", factory.__name__, operation)


class OptionalClose(Delegate):
    @property
    def close(self):
        raise AttributeError("no close")


def optional_close():
    yield from OptionalClose()


gen = optional_close()
next(gen)
assert gen.close() is None
print("optional-close", True)


factory_calls = []
factory_results = []
factory_mode = "outside"
factory_failure = LookupError("constructor failure")


class ExceptionFactory(type):
    def __call__(cls, *arguments):
        factory_calls.append(arguments)
        if (factory_mode == "first-failure" and len(factory_calls) == 1) or (
            factory_mode == "second-failure" and len(factory_calls) == 2
        ):
            raise factory_failure
        if factory_mode == "nonexception":
            return 42
        if factory_mode == "subclass":
            result = type.__call__(ReturnedError, *arguments)
        else:
            result = ValueError(*arguments)
        factory_results.append(result)
        return result


class RequestedError(ValueError, metaclass=ExceptionFactory):
    pass


class ReturnedError(RequestedError):
    pass


def capture_constructor():
    try:
        yield "constructor-ready"
    except BaseException as error:
        yield error


async def capture_coroutine_constructor():
    try:
        await pause()
    except BaseException as error:
        return error


for factory in (capture_constructor, capture_coroutine_constructor):
    for factory_mode in (
        "outside",
        "subclass",
        "existing-subclass",
        "first-failure",
        "second-failure",
        "nonexception",
    ):
        factory_calls.clear()
        factory_results.clear()
        value = factory()
        assert value.send(None) in ("constructor-ready", "pause")
        original = type.__call__(ReturnedError, "supplied")
        argument = original if factory_mode == "existing-subclass" else ("factory", 8)
        try:
            observed = value.throw(RequestedError, argument)
        except StopIteration as completed:
            assert factory is capture_coroutine_constructor
            observed = completed.value
        else:
            assert factory is capture_constructor
        if factory_mode == "existing-subclass":
            assert observed is original
            assert not factory_calls
        elif factory_mode in ("first-failure", "second-failure"):
            assert observed is factory_failure
            assert len(factory_calls) == (1 if factory_mode == "first-failure" else 2)
        elif factory_mode == "nonexception":
            assert type(observed) is TypeError
            assert len(factory_calls) == 1
        else:
            assert len(factory_calls) == 2
            assert factory_calls[0] == ("factory", 8)
            assert factory_calls[1] == (factory_results[0],)
            assert observed is factory_results[1]
            assert observed.args == (factory_results[0],)
            assert factory_results[0].args == ("factory", 8)
            assert type(observed) is (
                ReturnedError if factory_mode == "subclass" else ValueError
            )
        value.close()
        print("exception-factory", factory.__name__, factory_mode, len(factory_calls))


class ReentryDelegate(Delegate):
    def throw(self, *arguments):
        raise AssertionError(
            "new delegation must not receive an already normalized throw"
        )

    def close(self):
        raise AssertionError(
            "new delegation must not be closed by direct continuation injection"
        )


class ReentryFactory(type):
    def __call__(cls, *arguments):
        assert not reentry_value.gi_running
        assert reentry_value.send(None) == (
            "delegated" if reentry_delegates else "second"
        )
        return type.__call__(cls, *arguments)


class ReentryError(ValueError, metaclass=ReentryFactory):
    pass


def reentry_generator():
    try:
        yield "first"
        if reentry_delegates:
            yield from ReentryDelegate()
        else:
            yield "second"
    except ReentryError:
        yield "caught"


for reentry_delegates in (False, True):
    reentry_value = reentry_generator()
    assert next(reentry_value) == "first"
    assert reentry_value.throw(ReentryError) == "caught"
    reentry_value.close()
    print("constructor-reentry", reentry_delegates)


coroutine_delegate_events = []


class CoroutineReentryDelegate(Delegate):
    def throw(self, *arguments):
        coroutine_delegate_events.append("throw")
        raise AssertionError("new await delegation must not receive this throw")

    def close(self):
        coroutine_delegate_events.append("close")


class CoroutineReentryFactory(type):
    def __call__(cls, *arguments):
        assert not coroutine_reentry_value.cr_running
        assert coroutine_reentry_value.send(None) == "delegated"
        return type.__call__(cls, *arguments)


class CoroutineReentryError(ValueError, metaclass=CoroutineReentryFactory):
    pass


async def retained_inner_coroutine():
    return await CoroutineReentryDelegate()


async def coroutine_reentry_body(child):
    try:
        await child
    except CoroutineReentryError:
        return "caught"


for retains_inner in (False, True):
    coroutine_delegate_events.clear()
    child = retained_inner_coroutine() if retains_inner else CoroutineReentryDelegate()
    coroutine_reentry_value = coroutine_reentry_body(child)
    try:
        coroutine_reentry_value.throw(CoroutineReentryError)
    except StopIteration as result:
        assert result.value == "caught"
    else:
        raise AssertionError("constructor-reentered coroutine did not return")
    assert inspect.getcoroutinestate(coroutine_reentry_value) == "CORO_CLOSED"
    assert not coroutine_delegate_events
    if retains_inner:
        assert inspect.getcoroutinestate(child) == "CORO_SUSPENDED"
        assert child.send(None) == "delegated"
        child.close()
        assert coroutine_delegate_events == ["close"]
    print("coroutine-constructor-reentry", retains_inner)


class TracebackSpoof:
    @property
    def __class__(self):
        return types.TracebackType


gen = capture()
next(gen)
try:
    gen.throw(ValueError, None, TracebackSpoof())
except TypeError:
    assert inspect.getgeneratorstate(gen) == "GEN_SUSPENDED"
    print("traceback-spoof", True)
else:
    raise AssertionError("forged traceback admitted")
gen.close()


# Exceptions thrown before first execution are preserved, while StopIteration
# escaping an entered body is converted by its actual semantic kind.
class BodyStopIteration(StopIteration):
    pass


class BodyStopAsyncIteration(StopAsyncIteration):
    pass


def body_generator(error):
    try:
        yield "body-ready"
    finally:
        raise error


async def body_coroutine(error):
    try:
        await pause()
    finally:
        raise error


async def body_async_generator(error):
    try:
        yield "body-ready"
    finally:
        raise error


def expect_body_exception(call, original, kind, convert):
    try:
        call()
    except BaseException as error:
        if convert:
            stop_name = (
                "StopIteration"
                if isinstance(original, StopIteration)
                else "StopAsyncIteration"
            )
            assert type(error) is RuntimeError
            assert str(error) == kind + " raised " + stop_name
            assert error.__cause__ is original
            assert error.__context__ is original
            assert error.__suppress_context__
            assert original.__traceback__ is not None
            assert error.__traceback__ is not None
        else:
            assert error is original
        return
    raise AssertionError("body exception was lost")


def async_step_value(step):
    try:
        step.send(None)
    except StopIteration as finished:
        return finished.value
    raise AssertionError("async generator step did not return its yielded value")


for error_type in (
    StopIteration,
    BodyStopIteration,
    StopAsyncIteration,
    BodyStopAsyncIteration,
):
    for factory, kind in (
        (body_generator, "generator"),
        (body_coroutine, "coroutine"),
    ):
        for operation in ("send", "throw", "close"):
            original = error_type("body-error")
            value = factory(original)
            assert value.send(None) in ("body-ready", "pause")
            if operation == "send":
                invoke = lambda: value.send(None)
            elif operation == "throw":
                invoke = lambda: value.throw(ValueError("injected"))
            else:
                invoke = value.close
            expect_body_exception(
                invoke, original, kind, isinstance(original, StopIteration)
            )
            print("body-boundary", kind, error_type.__name__, operation)

    for operation in ("anext", "asend", "athrow", "aclose"):
        original = error_type("async-body-error")
        value = body_async_generator(original)
        assert async_step_value(value.__anext__()) == "body-ready"
        if operation == "anext":
            step = value.__anext__()
        elif operation == "asend":
            step = value.asend(17)
        elif operation == "athrow":
            step = value.athrow(ValueError("injected"))
        else:
            step = value.aclose()
        expect_body_exception(
            lambda: step.send(None), original, "async generator", True
        )
        print("body-boundary", "async generator", error_type.__name__, operation)

    for factory, kind in (
        (body_generator, "generator"),
        (body_coroutine, "coroutine"),
        (body_async_generator, "async generator"),
    ):
        original = error_type("unstarted")
        value = factory(original)
        if kind == "async generator":
            step = value.athrow(original)
            invoke = lambda: step.send(None)
        else:
            invoke = lambda: value.throw(original)
        expect_body_exception(invoke, original, kind, False)
        print("unstarted-boundary", kind, error_type.__name__)


# Created activations expose bound arguments and captured cells, never body
# locals; a throw before first execution raises in exactly that target frame.
unstarted_effects = []


def unstarted_scope(captured):
    def created_generator(x, y=2, *rest, k=3, **kw):
        unstarted_effects.append("generator-body")
        body = x + y

        def inner():
            return body, x

        yield captured, inner

    async def created_coroutine(x, y=2, *rest, k=3, **kw):
        unstarted_effects.append("coroutine-body")
        body = captured
        return body

    async def created_async_generator(x, y=2, *rest, k=3, **kw):
        unstarted_effects.append("async-generator-body")
        body = captured
        yield body

    return created_generator, created_coroutine, created_async_generator


def rendered_locals(mapping):
    rendered = []
    for name, local in dict(mapping).items():
        if isinstance(local, (int, str, tuple, dict)):
            rendered.append((name, local))
        else:
            rendered.append((name, type(local).__name__))
    return sorted(rendered)


def unstarted_throw(activation, label):
    error = ValueError(label)
    try:
        if label == "async generator":
            activation.athrow(error).send(None)
        else:
            activation.throw(error)
    except ValueError as caught:
        assert caught is error
        return caught
    raise AssertionError(label + " swallowed the thrown exception")


def traceback_chain(error):
    entries = []
    cursor = error.__traceback__
    while cursor is not None:
        entries.append(cursor)
        cursor = cursor.tb_next
    return entries


created_generator, created_coroutine, created_async_generator = unstarted_scope("cell")

value = created_generator(19, 5, 7, k=11, extra=1)
print(
    "created-locals",
    "generator",
    rendered_locals(value.gi_frame.f_locals),
    rendered_locals(inspect.getgeneratorlocals(value)),
)
value.close()
value = created_coroutine(19, k=13)
print(
    "created-locals",
    "coroutine",
    rendered_locals(value.cr_frame.f_locals),
    rendered_locals(inspect.getcoroutinelocals(value)),
)
value.close()
value = created_async_generator(19, 6)
print(
    "created-locals",
    "async generator",
    rendered_locals(value.ag_frame.f_locals),
    rendered_locals(inspect.getasyncgenlocals(value)),
)
value = None

for label, factory, arguments in (
    ("generator", created_generator, (19, 5, 7)),
    ("coroutine", created_coroutine, (19,)),
    ("async generator", created_async_generator, (19,)),
):
    value = factory(*arguments, k=11)
    caught = unstarted_throw(value, label)
    # The traceback owns its frame snapshot after the activation is gone.
    value = None
    entries = traceback_chain(caught)
    target = entries[-1]
    print(
        "unstarted-throw",
        label,
        [entry.tb_frame.f_code.co_name for entry in entries],
        target.tb_lineno - target.tb_frame.f_code.co_firstlineno,
        target.tb_frame.f_globals is globals(),
        rendered_locals(target.tb_frame.f_locals),
    )

rebound_namespace = {
    "__builtins__": __builtins__,
    "unstarted_effects": unstarted_effects,
    "marker": "rebound",
}
rebound = types.FunctionType(
    created_generator.__code__,
    rebound_namespace,
    "rebound_generator",
    created_generator.__defaults__,
    created_generator.__closure__,
)
rebound.__kwdefaults__ = created_generator.__kwdefaults__
caught = unstarted_throw(rebound(23), "rebound")
target = traceback_chain(caught)[-1]
print(
    "unstarted-rebound",
    target.tb_frame.f_code is created_generator.__code__,
    target.tb_frame.f_globals is rebound_namespace,
    target.tb_frame.f_globals.get("marker"),
    rendered_locals(target.tb_frame.f_locals),
)
print("unstarted-effects", unstarted_effects)

value = created_generator(1, 2)
yielded = next(value)
print("suspended-locals", yielded[0], rendered_locals(value.gi_frame.f_locals))
value.close()


# Existing traceback + live locals: one fixture exercises both activation
# entry paths, ordinary explicit/bare reraises, and finalizer reentry.
def follow_origin():
    raise ValueError("retained trace")


def follow_exception(observe):
    try:
        follow_origin()
    except ValueError as error:
        tail = error.__traceback__ if observe else None
        return error, tail


def follow_scope():
    free = 41

    def follow_gen(param):
        def capture():
            return param

        body = 23
        removed = 1
        del removed
        yield free + capture()
        body = 29
        yield body

    async def follow_coro(param):
        def capture():
            return param

        body = 23
        removed = 1
        del removed
        await FollowSuspend()
        return free + capture() + body

    async def follow_agen(param):
        def capture():
            return param

        body = 23
        removed = 1
        del removed
        yield free + capture()
        body = 29
        yield body

    return follow_gen, follow_coro, follow_agen


class FollowSuspend:
    def __await__(self):
        yield "follow pause"


def follow_start(value, kind):
    if kind == "generator":
        assert next(value) == 46
    elif kind == "coroutine":
        assert value.send(None) == "follow pause"
    else:
        try:
            value.__anext__().send(None)
        except StopIteration as result:
            assert result.value == 46
        else:
            raise AssertionError("async generator did not yield")


def follow_throw(value, kind, error):
    try:
        if kind == "async generator":
            value.athrow(error).send(None)
        else:
            value.throw(error)
    except ValueError as caught:
        assert caught is error
        return traceback_chain(caught)
    raise AssertionError("throw was swallowed")


follow_factories = follow_scope()
for follow_kind, follow_factory in zip(
    ("generator", "coroutine", "async generator"), follow_factories
):
    for follow_started in (False, True):
        for follow_observed in (False, True):
            follow_value = follow_factory(5)
            if follow_started:
                follow_start(follow_value, follow_kind)
            follow_error, follow_tail = follow_exception(follow_observed)
            follow_entries = follow_throw(follow_value, follow_kind, follow_error)
            follow_name = follow_factory.__name__
            follow_targets = [
                entry
                for entry in follow_entries
                if entry.tb_frame.f_code.co_name == follow_name
            ]
            assert len(follow_targets) == 1
            follow_locals = follow_targets[0].tb_frame.f_locals
            assert follow_locals["param"] == 5
            assert follow_locals["free"] == 41
            assert ("body" in follow_locals) == follow_started
            assert "removed" not in follow_locals
            if follow_started:
                assert follow_locals["body"] == 23
            assert follow_entries[-1].tb_frame.f_code.co_name == "follow_origin"
            if follow_tail is not None:
                assert any(entry is follow_tail for entry in follow_entries)
            follow_value = None
            assert follow_locals["param"] == 5
            print("follow-throw", follow_kind, follow_started, follow_observed, "ok")


def follow_reraise(explicit):
    try:
        follow_origin()
    except ValueError as error:
        before = error.__traceback__
        try:
            if explicit:
                raise error
            raise
        except ValueError as raised:
            after = raised.__traceback__
            if explicit:
                assert after is not before
                assert after.tb_next is before
            else:
                assert after is before


follow_reraise(False)
follow_reraise(True)
print("follow-reraise", "ok")

follow_reentries = []
follow_active = None


class FollowFinalizer:
    def __del__(self):
        frame = follow_active.gi_frame
        bindings = frame.f_locals
        follow_reentries.append(
            (bindings["param"], bindings["body"], bindings["marker"] is None)
        )


def follow_reenter(param):
    def capture():
        return param

    body = 23
    marker = FollowFinalizer()
    yield capture()
    body = 29
    marker = None
    yield body


follow_active = follow_reenter(5)
assert next(follow_active) == 5
assert next(follow_active) == 29
assert follow_reentries == [(5, 29, True)]
follow_error, follow_tail = follow_exception(True)
follow_entries = follow_throw(follow_active, "generator", follow_error)
follow_target = [
    entry
    for entry in follow_entries
    if entry.tb_frame.f_code.co_name == "follow_reenter"
][0]
assert follow_target.tb_frame.f_locals["body"] == 29
assert follow_target.tb_frame.f_locals["marker"] is None
follow_active = None
print("follow-finalizer-reentry", follow_reentries)
