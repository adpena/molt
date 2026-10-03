"""Purpose: differential coverage for frame binding lifetime, observation and release order."""

import sys

events = []


class Value:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(self.label)


class Holder:
    def method(self):
        # Declaration order differs from spelling order, and the method returns
        # by falling off its end.
        z_first = Value("first local")
        a_second = Value("second local")
        events.append(z_first.label + ":" + a_second.label)


def method_fallthrough_order():
    Holder().method()


def closure_cell_lifetime():
    captured = Value("captured")
    reader = lambda: captured.label  # noqa: E731
    events.append("read " + reader())
    # The frame keeps its own reference to the cell after the closure dies.
    del reader
    events.append("reader deleted")


def raising_frame(argument):
    z_local = Value("exception local")
    raise ValueError(argument.label + ":" + z_local.label)


def exception_exit_order():
    try:
        raising_frame(Value("exception parameter"))
    except ValueError:
        events.append("handler")
    events.append("handler left")


def inner_frame(argument):
    inner_local = Value("inner local")
    raise ValueError(argument.label + ":" + inner_local.label)


def outer_frame(argument):
    outer_local = Value("outer local")
    inner_frame(Value("inner parameter"))
    events.append("unreachable " + outer_local.label)


def nested_traceback_order():
    held = []
    try:
        outer_frame(Value("outer parameter"))
    except ValueError as error:
        held.append(error)
    events.append("handler left")
    held.clear()
    events.append("traceback released")


def inspected_frame(argument):
    z_local = argument + 1
    raise KeyError(z_local)


def cell_frame():
    captured = 7
    reader = lambda: captured  # noqa: E731
    raise KeyError(reader())


def traceback_frame_locals():
    try:
        inspected_frame(1)
    except KeyError as error:
        events.append(list(error.__traceback__.tb_next.tb_frame.f_locals.items()))
    try:
        cell_frame()
    except KeyError as error:
        events.append(list(error.__traceback__.tb_next.tb_frame.f_locals))


class CallerProbe:
    def __del__(self):
        # The exiting frame is no longer the executing one: its caller is.
        events.append(sys._getframe(1).f_code.co_name)


def implicit_return():
    probe = CallerProbe()  # noqa: F841


def explicit_return():
    probe = CallerProbe()  # noqa: F841
    return None


def finalizer_sees_the_caller():
    implicit_return()
    explicit_return()


def live_frame_locals():
    frame = sys._getframe()
    first = 1  # noqa: F841
    before = sorted(frame.f_locals)
    second = 2  # noqa: F841
    events.append(before)
    events.append(sorted(frame.f_locals))
    del frame


def rebinding_releases_at_the_store():
    value = Value("first binding")
    events.append("bound")
    value = Value("second binding")
    events.append("rebound")
    del value
    events.append("deleted")


def comprehension_restores_its_name():
    item = Value("outer item")
    labels = [item.label for item in (Value("inner item"),)]
    events.append(labels)
    events.append(item.label)


class Base:
    def who(self):
        return "base"


class Derived(Base):
    def who(self):
        self = Derived()
        return super().who()


def super_reads_the_rebound_argument():
    events.append(Derived().who())


def counter():
    total = 0
    for step in range(3):
        total += step
        yield total


def generator_frame_locals():
    generator = counter()
    next(generator)
    frame = generator.gi_frame
    events.append(sorted(frame.f_locals.items()))
    next(generator)
    events.append(sorted(frame.f_locals.items()))
    del frame


def holding_generator():
    held = Value("generator local")  # noqa: F841
    yield 1


def completed_generator_releases_its_frame():
    generator = holding_generator()
    next(generator)
    events.append("suspended")
    for _ in generator:
        pass
    events.append("completed")
    del generator


async def holding_coroutine():
    held = Value("coroutine local")  # noqa: F841
    return 1


def completed_coroutine_releases_its_frame():
    coroutine = holding_coroutine()
    try:
        coroutine.send(None)
    except StopIteration:
        events.append("completed")
    del coroutine


for case in (
    method_fallthrough_order,
    closure_cell_lifetime,
    exception_exit_order,
    nested_traceback_order,
    traceback_frame_locals,
    finalizer_sees_the_caller,
    live_frame_locals,
    rebinding_releases_at_the_store,
    comprehension_restores_its_name,
    super_reads_the_rebound_argument,
    generator_frame_locals,
    completed_generator_releases_its_frame,
    completed_coroutine_releases_its_frame,
):
    events.clear()
    case()
    print(case.__name__, events)
