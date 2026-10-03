"""Purpose: differential coverage for sys._getframe fields."""

import sys
import inspect

frame = sys._getframe()
print(isinstance(frame.f_globals, dict))
print(isinstance(frame.f_locals, dict))
print(frame.f_globals.get("__name__") == __name__)


def inner():
    f = sys._getframe()
    print(f.f_back is not None)
    print(isinstance(f.f_back.f_globals, dict))


inner()


class Index:
    def __index__(self):
        return 0


class BadIndex:
    def __index__(self):
        return 0.0


class FailingIndex:
    def __index__(self):
        raise LookupError("depth callback failed")


def frame_depth_contract():
    alias = sys._getframe
    for depth in (0, -1, -(2**31), False, Index()):
        current = alias(depth)
        print("current", current.f_code.co_name)
        print(current.f_globals is globals(), "alias" in current.f_locals)
    parent = alias(1)
    print("parent", parent.f_code.co_name)
    print("inspect", inspect.currentframe().f_code.co_name)
    for depth in (
        2**31 - 1,
        2**31,
        -(2**31) - 1,
        2**100,
        0.0,
        None,
        BadIndex(),
        FailingIndex(),
    ):
        try:
            alias(depth)
        except (TypeError, ValueError, OverflowError, LookupError) as error:
            print(type(error).__name__, str(error))
        else:
            raise AssertionError("invalid depth accepted")
    try:
        alias(depth=0)
    except TypeError:
        print("positional-only")
    else:
        raise AssertionError("keyword depth accepted")


frame_depth_contract()
