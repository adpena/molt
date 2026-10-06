"""Purpose: differential coverage for locals() mutation semantics.

Fused loops read a local once, before the loop, where Python reads it inside
the loop; after a callback rebinds that local through a frame proxy (3.13+),
the single read must see the new binding, and its cached type must not choose
the fused op's lane.
"""

import sys


def replace_callers_local(value):
    frame = sys._getframe(1)
    frame.f_locals["x"] = value


def callback_replaces_local():
    x = 1
    replace_callers_local("replaced")
    return x


def callback_replaces_callable():
    x = len
    replace_callers_local(lambda value: "replacement callable")
    return x(())


class ReenterOnRelease:
    def __del__(self):
        frame = sys._getframe(1)
        frame.f_locals["x"] = "replaced by finalizer"


def displaced_owner_reenters():
    x = 1
    replace_callers_local(ReenterOnRelease())
    x = 17
    return x


class Released:
    def __init__(self, label, log):
        self.label = label
        self.log = log

    def __del__(self):
        self.log.append(("released", self.label))


class ReenterWithHeapValue:
    def __init__(self, log):
        self.log = log

    def __del__(self):
        frame = sys._getframe(1)
        frame.f_locals["x"] = Released("installed by finalizer", self.log)


def displaced_heap_owner_reenters():
    # The store of "assigned" releases the finalizer-bearing binding, whose
    # finalizer rebinds x through the frame proxy: the read must load the
    # finalizer's object, never the store's own, possibly released, value.
    log = []
    x = Released("original", log)
    replace_callers_local(ReenterWithHeapValue(log))
    x = Released("assigned", log)
    log.append(("read", x.label))
    return log


def fused_sum_after_rebinding():
    x = [1, 2, 3]
    total = 0
    replace_callers_local([10, 20])
    for item in x:
        total += item
    return total


def fused_indexed_sum_after_rebinding():
    x = [1, 2, 3]
    total = 0
    replace_callers_local([10, 20])
    for i in range(len(x)):
        total += x[i]
    return total


def fused_accumulator_after_rebinding():
    x = 0.0
    replace_callers_local(100)
    for i in range(4):
        x += i
    return x


def fused_cell_accumulator_after_rebinding():
    x = 0.0

    def read():
        return x

    replace_callers_local(100)
    for i in range(4):
        x += i
    return x, read()


def counted_index_after_rebinding():
    x = 0
    seen = []
    while x < 5:
        seen.append(x)
        if x == 0:
            replace_callers_local(3)
        x += 1
    return seen


def fused_sum_iterable_after_rebinding():
    x = 4
    replace_callers_local(3)
    return sum(x for x in range(x))


def proxy_write_release_timing(captured, retain_frame):
    events = []
    saved = []

    class Probe:
        def __del__(self):
            events.append("released")

    def setter():
        frame = sys._getframe(1)
        if retain_frame:
            saved.append(frame)
        frame.f_locals["x"] = None
        events.append("assigned")

    def owner():
        x = Probe()
        setter()
        events.append("resumed")

    def cell_owner():
        x = Probe()

        def read():
            return x

        setter()
        events.append("resumed")
        return read()

    if captured:
        cell_owner()
    else:
        owner()
    events.append("owner exited")
    saved.clear()
    events.append("observations cleared")
    return events


def suspended_generator():
    x = 1
    yield None
    yield x


def repeated_proxy_writes(captured, clear_frame):
    events = []
    saved = []

    class Probe:
        def __init__(self, name):
            self.name = name

        def __del__(self):
            events.append(("released", self.name))

    def setter():
        frame = sys._getframe(1)
        saved.append(frame)
        proxy = frame.f_locals
        proxy["x"] = Probe("second")
        events.append("first write")
        proxy["x"] = proxy["x"]
        events.append("same object write")
        proxy["x"] = Probe("third")
        events.append("second write")

    def owner():
        x = Probe("first")
        setter()
        events.append(("resumed", x.name))

    def cell_owner():
        x = Probe("first")

        def read():
            return x

        setter()
        events.append(("resumed", read().name))

    if captured:
        cell_owner()
    else:
        owner()
    events.append("owner exited")
    if clear_frame:
        saved[0].clear()
        events.append("frame cleared")
    saved.clear()
    events.append("frame released")
    return events


class Pause:
    def __await__(self):
        yield None


async def suspended_coroutine():
    x = 1
    await Pause()
    return x


def suspension_mutations():
    generator = suspended_generator()
    next(generator)
    generator.gi_frame.f_locals["x"] = "generator replacement"
    print("generator frame", next(generator))
    generator.close()
    coroutine = suspended_coroutine()
    coroutine.send(None)
    coroutine.cr_frame.f_locals["x"] = "coroutine replacement"
    try:
        coroutine.send(None)
    except StopIteration as completed:
        print("coroutine frame", completed.value)


def main():
    def inner():
        x = 1
        locals()["x"] = 2
        locals()["y"] = 3
        return x, locals().get("y")

    print("inner", inner())
    print("frame callback", callback_replaces_local())
    print("frame callable", callback_replaces_callable())
    for captured in (False, True):
        for retain_frame in (False, True):
            print("proxy release", captured, retain_frame,
                  proxy_write_release_timing(captured, retain_frame))
    for captured in (False, True):
        for clear_frame in (False, True):
            print("repeated proxy writes", captured, clear_frame,
                  repeated_proxy_writes(captured, clear_frame))
    if sys.version_info >= (3, 13):
        print("frame reentry", displaced_owner_reenters())
        print("frame heap reentry", displaced_heap_owner_reenters())
    print("fused sum", fused_sum_after_rebinding())
    print("fused indexed sum", fused_indexed_sum_after_rebinding())
    print("fused accumulator", fused_accumulator_after_rebinding())
    print("fused cell accumulator", fused_cell_accumulator_after_rebinding())
    print("counted index", counted_index_after_rebinding())
    print("fused sum iterable", fused_sum_iterable_after_rebinding())
    suspension_mutations()


if __name__ == "__main__":
    main()
