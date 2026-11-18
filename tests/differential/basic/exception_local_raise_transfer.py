"""Purpose: a raise its own function catches keeps every observable effect of a raise."""

import sys


class Tracked(ValueError):
    freed = 0

    def __del__(self):
        Tracked.freed += 1


def finalized_once_each(n):
    caught = 0
    for i in range(n):
        try:
            if i % 2 == 0:
                raise Tracked(i)
        except Tracked as error:
            caught += int(str(error)) >= 0
    return caught


print("caught", finalized_once_each(10), "freed", Tracked.freed)


def chains_the_handled_exception():
    try:
        raise KeyError("outer")
    except KeyError:
        try:
            raise ValueError("inner")
        except ValueError as error:
            return type(error.__context__).__name__, error.__context__.args


print("context", chains_the_handled_exception())


def keeps_an_explicit_cause():
    try:
        raise ValueError("effect") from KeyError("cause")
    except ValueError as error:
        return (
            type(error.__cause__).__name__,
            error.__suppress_context__,
            error.__context__,
        )


print("cause", keeps_an_explicit_cause())


def unmatched_goes_to_the_outer_handler():
    try:
        try:
            raise OSError(2, "missing")
        except (ValueError, KeyError):
            return "wrong"
    except OSError as error:
        return "outer", error.errno


print("unmatched", unmatched_goes_to_the_outer_handler())


def traceback_names_the_raise_line():
    try:
        raise RuntimeError("where")
    except RuntimeError as error:
        tb = error.__traceback__
        return tb.tb_lineno - traceback_names_the_raise_line.__code__.co_firstlineno


print("traceback line offset", traceback_names_the_raise_line())


def handled_state_inside_the_handler():
    try:
        raise LookupError("seen")
    except LookupError as error:
        inside = sys.exception() is error
    return inside, sys.exception()


print("handled", handled_state_inside_the_handler())


saved = []


def escapes_through_a_list():
    for i in range(3):
        try:
            raise IndexError(i)
        except IndexError as error:
            saved.append(error)


escapes_through_a_list()
print("escaped", [e.args for e in saved], [e.__traceback__ is not None for e in saved])


def reraises_from_the_handler():
    try:
        raise TypeError("again")
    except TypeError:
        raise


try:
    reraises_from_the_handler()
except TypeError as error:
    print("reraised", error.args, error.__context__)


def finally_and_else(flag):
    log = []
    try:
        if flag:
            raise ValueError("f")
    except ValueError:
        log.append("except")
    else:
        log.append("else")
    finally:
        log.append("finally")
    return log


print("finally", finally_and_else(True), finally_and_else(False))


def handler_raises_a_new_error():
    try:
        raise ValueError("first")
    except ValueError:
        return int("not a number")


try:
    handler_raises_a_new_error()
except ValueError as error:
    print("during handling", error.args, type(error.__context__).__name__, error.__context__.args)


total = 0
for i in range(30):
    try:
        if i % 3 == 0:
            raise ValueError(i)
        total += i
    except ValueError as e:
        total += int(str(e))
print("total", total)
