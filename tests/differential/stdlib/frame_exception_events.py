"""CPython error versus exception_unwind: order, multiplicity and locations."""

import sys


def chain(error):
    rows = []
    tb = error.__traceback__
    while tb is not None:
        code = tb.tb_frame.f_code
        position = list(code.co_positions())[tb.tb_lasti // 2]
        rows.append((code.co_name, tb.tb_lineno, position))
        tb = tb.tb_next
    return rows


def relay(error):
    frame = sys._getframe()
    assert frame.f_code.co_name == "relay"
    raise error


def repeated():
    error = ValueError("same object")
    try:
        relay(error)
    except ValueError:
        pass
    try:
        relay(error)
    except ValueError as caught:
        print("repeated", chain(caught))


repeated()


def cleared():
    try:
        raise ValueError("clear")
    except ValueError as error:
        error.__traceback__ = None
        raise


try:
    cleared()
except ValueError as error:
    print("cleared bare", chain(error))


def no_active():
    raise


try:
    no_active()
except RuntimeError as error:
    print("no active", chain(error))


saved = []


class Missing:
    @property
    def value(self):
        error = AttributeError("retained")
        saved.append(error)
        raise error


print("default", getattr(Missing(), "value", 17))
print("suppressed getter", chain(saved[0]))


def runtime_failure():
    frame = sys._getframe()
    assert frame.f_code.co_name == "runtime_failure"
    return 1 // 0


try:
    runtime_failure()
except ZeroDivisionError as error:
    print("operation", chain(error))


def suspended():
    try:
        yield 1
    except ValueError as error:
        print("injected", chain(error))
        raise


generator = suspended()
print("yield", next(generator))
try:
    generator.throw(ValueError("injected"))
except ValueError as error:
    print("injected propagated", chain(error))
