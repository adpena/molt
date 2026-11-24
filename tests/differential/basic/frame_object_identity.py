"""One frame object per activation, for ordinary frames and fused generator frames."""

import gc
import sys


def same_frame():
    return sys._getframe() is sys._getframe()


def child():
    return sys._getframe(0)


def parent():
    first = child()
    second = child()
    return first.f_back is second.f_back, first.f_back is sys._getframe()


def frames(n):
    for i in range(n):
        yield sys._getframe() is sys._getframe()


def consumer():
    out = []
    for same in frames(2):
        out.append(same)
    return out


print(same_frame(), parent(), consumer())

def traced(frames):
    frames.append(sys._getframe())
    raise ValueError()


def traceback_identity():
    frames = []
    caller = sys._getframe()
    try:
        traced(frames)
    except ValueError as exc:
        tb = exc.__traceback__
        return tb.tb_frame is caller, tb.tb_next.tb_frame is frames[0], frames[0].f_back is caller


def suspended():
    yield sys._getframe()
    yield sys._getframe(), sys._getframe().f_back


def generator_identity():
    activation = suspended()
    created = activation.gi_frame
    first = next(activation)
    second, live_caller = next(activation)
    suspended_back = first.f_back
    activation.close()
    return (
        created is first is second,
        live_caller is sys._getframe(),
        suspended_back is None,
        activation.gi_frame is None,
    )


def escaped_frames(n):
    for index in range(n):
        yield sys._getframe()
    index = -1


def escaped_splice():
    seen = []
    for frame in escaped_frames(2):
        seen.append((frame, frame.f_back is None, frame.f_locals['index']))
    last = seen[-1][0]
    return (
        seen[0][0] is last,
        [(back, index) for _, back, index in seen],
        last.f_locals["index"],
        last.f_back is sys._getframe(),
    )


class Marker:
    def __del__(self):
        events.append("released")


def observe_and_drop():
    marker = Marker()
    sys._getframe()


events = []
print(traceback_identity(), generator_identity(), escaped_splice())
observe_and_drop()
print(events)


def leave_cycle():
    marker = Marker()
    frame = sys._getframe()
    return frame is sys._getframe()


print(leave_cycle())
gc.collect()
print(events)


def both_branch_bindings(flag):
    frame = sys._getframe()
    if flag:
        choice = 11
    else:
        choice = 22
    yield frame


def optional_branch_binding(flag):
    frame = sys._getframe()
    if flag:
        optional = None
    yield frame


def observed_branch_bindings():
    values = []
    for flag in (True, False):
        for frame in both_branch_bindings(flag):
            values.append(frame.f_locals["choice"])
        for frame in optional_branch_binding(flag):
            values.append(("optional" in frame.f_locals, frame.f_locals.get("optional")))
    return values


print(observed_branch_bindings())
