"""Replacing each builtin among the scalar call lowerings reaches every read of
it (HF-776).

The frontend lowers a proved read of these builtins by name: `type`, `object`,
`id`, `ord`, `chr`, `repr`, `callable`, `abs`.
For each one in turn, a module binding, a `builtins` member write and a
`__builtins__` mapping select another object, so every read below must reach
the replacement, as in CPython. The mapping form comes last: a function takes
its builtins from the mapping when it is created.
"""

import builtins

KINDS = ("object",)
NAMES = (
    "type",
    "object",
    "id",
    "ord",
    "chr",
    "repr",
    "callable",
    "abs",
)
ORIGINALS = {name: getattr(builtins, name) for name in NAMES}


class Fake:
    """A replacement whose value and call results print deterministically."""

    def __init__(self, name):
        self.name = name

    def __call__(self, *args, **kwargs):
        return "called-" + self.name

    def __repr__(self):
        return "fake-" + self.name


def show(value):
    # Reads no replaceable builtin: the program replaces them.
    return value.__class__.__name__ if value.__class__.__name__ in KINDS else value


SENTINEL = []
SENTINEL_ID = id(SENTINEL)


def use_type():
    return type(1)


print("type original", show(use_type()), show(type(1)))
type = Fake("type-module")
print("type module binding", show(use_type()), show(type(1)))
del type
print("type module binding deleted", show(use_type()), show(type(1)))
builtins.type = Fake("type-member")
print("type member", show(use_type()), show(type(1)))
builtins.type = ORIGINALS["type"]
print("type member restored", show(use_type()), show(type(1)))


def use_object():
    return object()


print("object original", show(use_object()), show(object()))
object = Fake("object-module")
print("object module binding", show(use_object()), show(object()))
del object
print("object module binding deleted", show(use_object()), show(object()))
builtins.object = Fake("object-member")
print("object member", show(use_object()), show(object()))
builtins.object = ORIGINALS["object"]
print("object member restored", show(use_object()), show(object()))


def use_id():
    return id(SENTINEL) == SENTINEL_ID


print("id original", show(use_id()), show(id(SENTINEL) == SENTINEL_ID))
id = Fake("id-module")
print("id module binding", show(use_id()), show(id(SENTINEL) == SENTINEL_ID))
del id
print("id module binding deleted", show(use_id()), show(id(SENTINEL) == SENTINEL_ID))
builtins.id = Fake("id-member")
print("id member", show(use_id()), show(id(SENTINEL) == SENTINEL_ID))
builtins.id = ORIGINALS["id"]
print("id member restored", show(use_id()), show(id(SENTINEL) == SENTINEL_ID))


def use_ord():
    return ord('a')


print("ord original", show(use_ord()), show(ord('a')))
ord = Fake("ord-module")
print("ord module binding", show(use_ord()), show(ord('a')))
del ord
print("ord module binding deleted", show(use_ord()), show(ord('a')))
builtins.ord = Fake("ord-member")
print("ord member", show(use_ord()), show(ord('a')))
builtins.ord = ORIGINALS["ord"]
print("ord member restored", show(use_ord()), show(ord('a')))


def use_chr():
    return chr(65)


print("chr original", show(use_chr()), show(chr(65)))
chr = Fake("chr-module")
print("chr module binding", show(use_chr()), show(chr(65)))
del chr
print("chr module binding deleted", show(use_chr()), show(chr(65)))
builtins.chr = Fake("chr-member")
print("chr member", show(use_chr()), show(chr(65)))
builtins.chr = ORIGINALS["chr"]
print("chr member restored", show(use_chr()), show(chr(65)))


def use_repr():
    return repr('a')


print("repr original", show(use_repr()), show(repr('a')))
repr = Fake("repr-module")
print("repr module binding", show(use_repr()), show(repr('a')))
del repr
print("repr module binding deleted", show(use_repr()), show(repr('a')))
builtins.repr = Fake("repr-member")
print("repr member", show(use_repr()), show(repr('a')))
builtins.repr = ORIGINALS["repr"]
print("repr member restored", show(use_repr()), show(repr('a')))


def use_callable():
    return callable(SENTINEL)


print("callable original", show(use_callable()), show(callable(SENTINEL)))
callable = Fake("callable-module")
print("callable module binding", show(use_callable()), show(callable(SENTINEL)))
del callable
print("callable module binding deleted", show(use_callable()), show(callable(SENTINEL)))
builtins.callable = Fake("callable-member")
print("callable member", show(use_callable()), show(callable(SENTINEL)))
builtins.callable = ORIGINALS["callable"]
print("callable member restored", show(use_callable()), show(callable(SENTINEL)))


def use_abs():
    return abs(-3)


print("abs original", show(use_abs()), show(abs(-3)))
abs = Fake("abs-module")
print("abs module binding", show(use_abs()), show(abs(-3)))
del abs
print("abs module binding deleted", show(use_abs()), show(abs(-3)))
builtins.abs = Fake("abs-member")
print("abs member", show(use_abs()), show(abs(-3)))
builtins.abs = ORIGINALS["abs"]
print("abs member restored", show(use_abs()), show(abs(-3)))

custom = dict(builtins.__dict__)
custom["type"] = Fake("type-mapping")
custom["object"] = Fake("object-mapping")
custom["id"] = Fake("id-mapping")
custom["ord"] = Fake("ord-mapping")
custom["chr"] = Fake("chr-mapping")
custom["repr"] = Fake("repr-mapping")
custom["callable"] = Fake("callable-mapping")
custom["abs"] = Fake("abs-mapping")
__builtins__ = custom


def after_type():
    return type(1)


print("type mapping", show(use_type()), show(after_type()))


def after_object():
    return object()


print("object mapping", show(use_object()), show(after_object()))


def after_id():
    return id(SENTINEL) == SENTINEL_ID


print("id mapping", show(use_id()), show(after_id()))


def after_ord():
    return ord('a')


print("ord mapping", show(use_ord()), show(after_ord()))


def after_chr():
    return chr(65)


print("chr mapping", show(use_chr()), show(after_chr()))


def after_repr():
    return repr('a')


print("repr mapping", show(use_repr()), show(after_repr()))


def after_callable():
    return callable(SENTINEL)


print("callable mapping", show(use_callable()), show(after_callable()))


def after_abs():
    return abs(-3)


print("abs mapping", show(use_abs()), show(after_abs()))
