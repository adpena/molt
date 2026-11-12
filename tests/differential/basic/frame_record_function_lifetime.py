"""Purpose: a running frame keeps its own function and code alive and reports the code it entered with."""

import sys


def victim():
    global victim
    # Drop the only binding of this function while it runs.
    victim = None
    frame = sys._getframe(0)
    print("running", frame.f_code.co_name, frame.f_lineno > 0)
    try:
        raise KeyError("after unbind")
    except KeyError as exc:
        print("traceback", exc.__traceback__.tb_frame.f_code.co_name)
    return frame.f_code.co_name


def replacement():
    return "replacement body"


def swapped():
    swapped.__code__ = replacement.__code__
    frame = sys._getframe(0)
    try:
        raise ValueError("after swap")
    except ValueError as exc:
        tb_name = exc.__traceback__.tb_frame.f_code.co_name
    return frame.f_code.co_name, tb_name


def recursive(depth):
    if depth == 0:
        return sys._getframe(0).f_back.f_code.co_name
    return recursive(depth - 1)


if __name__ == "__main__":
    print("returned", victim())
    print("unbound", victim)
    print("swapped", swapped())
    print("next call", swapped())
    print("recursive", recursive(3))
