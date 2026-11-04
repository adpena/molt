"""Executing, retained, suspended and traceback locations share the code table."""

import sys


def site(frame):
    offset = frame.f_lasti
    position = None if offset < 0 else list(frame.f_code.co_positions())[offset // 2]
    return frame.f_code.co_name, frame.f_lineno, position


def returned():
    frame = sys._getframe()
    print("live", site(frame))
    return frame


held = returned()
print("returned", site(held))


def loop():
    frame = sys._getframe()
    for value in range(3):
        print("loop", value, site(frame))
    return frame


print("loop returned", site(loop()))


def recursive(depth):
    frame = sys._getframe()
    print("recursive", depth, site(frame))
    if depth:
        recursive(depth - 1)
    else:
        print("caller", site(frame.f_back))


recursive(2)


def suspended():
    frame = sys._getframe()
    yield site(frame)
    print("resumed", site(frame))
    yield 17


generator = suspended()
retained = generator.gi_frame
print("created", site(retained))
print("yield value", next(generator))
print("suspended", site(retained))
returned()  # Reuse ordinary frame storage between suspensions.
print("yield constant", next(generator))
print("suspended again", site(retained))
generator.close()
print("closed", generator.gi_frame is None, site(retained))


def traceback_sites(trace):
    rows = []
    while trace is not None:
        positions = list(trace.tb_frame.f_code.co_positions())
        rows.append((trace.tb_frame.f_code.co_name, trace.tb_lineno, positions[trace.tb_lasti // 2]))
        trace = trace.tb_next
    return rows


def caught():
    frame = sys._getframe()
    try:
        1 / 0
    except ZeroDivisionError as error:
        trace = error.__traceback__
    print("continued", site(frame))
    print("caught trace", traceback_sites(trace))


caught()


def propagating():
    try:
        2 / 0
    finally:
        print("finally", site(sys._getframe()))


try:
    propagating()
except ZeroDivisionError as error:
    print("propagated trace", traceback_sites(error.__traceback__))
