"""Purpose: differential coverage for types.new_class helper failure semantics."""

import types


class MetaA(type):
    pass


class MetaB(type):
    pass


class A(metaclass=MetaA):
    pass


class B(metaclass=MetaB):
    pass


for label, thunk in [
    ("prepare_conflict", lambda: types.prepare_class("Conflict", (A, B))),
    ("new_class_conflict", lambda: types.new_class("Conflict", (A, B))),
]:
    try:
        thunk()
    except Exception as exc:
        print(label, type(exc).__name__, "metaclass conflict" in str(exc))


class BadPrepareMeta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwds):
        return 1


try:
    types.new_class("BadPrepare", (), {"metaclass": BadPrepareMeta})
except Exception as exc:
    print("bad_prepare", type(exc).__name__)


class BadEntries:
    def __mro_entries__(self, bases):
        return [dict]


for label, thunk in [
    ("resolve_bad_entries", lambda: types.resolve_bases((BadEntries(),))),
    ("new_class_bad_entries", lambda: types.new_class("BadEntries", (BadEntries(),))),
]:
    try:
        thunk()
    except Exception as exc:
        print(label, type(exc).__name__, "__mro_entries__" in str(exc))


class NonCallablePrepareMeta(type):
    __prepare__ = 1


class RaisePrepareMeta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwds):
        raise RuntimeError("prepare boom")


class NonCallableEntries:
    __mro_entries__ = 1


for label, thunk in [
    (
        "prepare_noncallable",
        lambda: types.prepare_class(
            "NonCallablePrepare", (), {"metaclass": NonCallablePrepareMeta}
        ),
    ),
    (
        "new_class_prepare_raises",
        lambda: types.new_class("RaisePrepare", (), {"metaclass": RaisePrepareMeta}),
    ),
    ("resolve_noncall_entries", lambda: types.resolve_bases((NonCallableEntries(),))),
    (
        "new_class_noncall_entries",
        lambda: types.new_class("NonCallableEntries", (NonCallableEntries(),)),
    ),
]:
    try:
        thunk()
    except Exception as exc:
        print(label, type(exc).__name__)
