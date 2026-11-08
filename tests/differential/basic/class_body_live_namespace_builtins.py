"""Aliases, replacement builtins and callbacks reflect the executing class frame."""

import builtins
import sys

saved_locals = locals


def aliased():
    enclosing_only = 1
    alias = saved_locals

    class C:
        b = 2
        keys = sorted(alias())

    print(C.keys)
    return enclosing_only


print(aliased())


def replaced():
    return {"replacement": 81}


builtins.locals = replaced
try:
    class Replaced:
        view = locals()

    print(Replaced.view)
finally:
    builtins.locals = saved_locals


def helper():
    helper_only = 82
    view = saved_locals()
    print(view["helper_only"], "class_only" in view)


class Isolated:
    class_only = 83
    helper()
    view = saved_locals()


print(Isolated.view["class_only"], "helper_only" in Isolated.view)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        caller = sys._getframe(1)
        print(caller.f_code.co_name, caller.f_locals["outer_marker"])
        return {}


class Outer:
    outer_marker = 84

    class Inner(metaclass=Meta):
        inner_marker = 85


try:
    class Broken:
        raise RuntimeError("class body")
except RuntimeError as error:
    tb = error.__traceback__
    trace_names = [tb.tb_frame.f_code.co_name]
    while tb.tb_next is not None:
        tb = tb.tb_next
        trace_names.append(tb.tb_frame.f_code.co_name)
    print(tb.tb_frame.f_code.co_name, tb.tb_lineno)
    print(trace_names)

print("class_only" in saved_locals(), "outer_marker" in saved_locals())


original_builtins = globals()["__builtins__"]
replacement_builtins = dict(vars(builtins))
replacement_builtins["len"] = lambda value: 99


def changing_base():
    globals()["__builtins__"] = replacement_builtins
    return object


try:
    class Captured(changing_base()):
        observed = len([])
        captured = sys._getframe().f_builtins is vars(builtins)

    print(Captured.observed, Captured.captured, replacement_builtins["len"]([]))
finally:
    globals()["__builtins__"] = original_builtins


from enum import Enum


try:
    class ArithmeticFailure:
        invalid = 1 // 0
except ZeroDivisionError as error:
    tb = error.__traceback__
    while tb.tb_next is not None:
        tb = tb.tb_next
    print(tb.tb_frame.f_code.co_name, tb.tb_lineno, tb.tb_frame.f_code.co_flags & 3)


def nested_failure():
    class FailedOuter:
        class FailedInner:
            raise RuntimeError("nested class body")


try:
    nested_failure()
except RuntimeError as error:
    tb = error.__traceback__
    trace_names = []
    while tb is not None:
        trace_names.append(tb.tb_frame.f_code.co_name)
        tb = tb.tb_next
    print(trace_names)


class Choice(Enum):
    _ignore_ = "namespace temporary"
    namespace = vars()
    temporary = 86
    namespace["FIRST"] = temporary


print(Choice.FIRST.value, list(Choice.__members__))


class ListedIgnore(Enum):
    _ignore_ = ["temporary"]
    temporary = 1
    temporary = 2
    MEMBER = temporary


print(ListedIgnore.MEMBER.value, list(ListedIgnore.__members__))
print(hasattr(ListedIgnore, "temporary"), hasattr(ListedIgnore, "_ignore_"))
try:
    class LateIgnore(Enum):
        MEMBER = 1
        _ignore_ = ["MEMBER"]
except ValueError as error:
    print(type(error).__name__, str(error))


shared_namespace = {}


class SharedMeta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        shared_namespace.clear()
        return shared_namespace

    def __new__(mcls, name, bases, namespace):
        return super().__new__(mcls, name, bases, dict(namespace))


class SharedFirst(metaclass=SharedMeta):
    frame = sys._getframe()


finished_frame = SharedFirst.frame


class SharedSecond(metaclass=SharedMeta):
    finished_frame.clear()
    active = sys._getframe()
    refused = False
    try:
        active.clear()
    except RuntimeError:
        refused = True


print(SharedSecond.refused)


try:

    class LookupFailure:
        global absent_class_global
        marker = absent_class_global
except NameError as error:
    traceback = error.__traceback__
    while traceback.tb_next is not None:
        traceback = traceback.tb_next
    print(traceback.tb_frame.f_code.co_name, traceback.tb_lineno)
