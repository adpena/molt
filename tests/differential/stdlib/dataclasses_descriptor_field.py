"""Purpose: a dataclass field typed by a data descriptor keeps CPython semantics.

The default comes from the descriptor's ``__get__(None, cls)``, the descriptor
stays the class attribute, stores convert through ``__set__``, and only a
``field()`` object is replaced on the class.
"""

print("start")

from dataclasses import dataclass, field


def check(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, "raised", type(exc).__name__, exc)


class Converted:
    def __set_name__(self, owner, name):
        self.storage = "_" + name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return 100
        return getattr(obj, self.storage, 100)

    def __set__(self, obj, value):
        setattr(obj, self.storage, int(value))


@dataclass
class Record:
    q: Converted = Converted()


record = Record("5")
check("init", lambda: (record.q, type(record.q).__name__))
check("default", lambda: Record().q)
check("class-attr", lambda: type(Record.__dict__["q"]).__name__)


def store():
    record.q = "7"
    return (record.q, type(record.q).__name__)


check("store", store)


@dataclass
class WithField:
    items: list = field(default_factory=list)
    count: int = field(default=3)


def field_defaults():
    value = WithField()
    value.items.append(1)
    value.count += 1
    return (value.items, value.count, "items" in WithField.__dict__, WithField.count)


check("field", field_defaults)


class Defaults:
    level = 5


@dataclass
class FromBase(Defaults):
    level: int


check("inherited-default", lambda: (FromBase().level, "level" in FromBase.__dict__))
