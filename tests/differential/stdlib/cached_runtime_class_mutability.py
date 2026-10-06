"""Class namespace policy stays independent of each instance's physical slots."""

import functools
import importlib.machinery
import operator
import types


def mutation(label, cls):
    name = "cached_class_policy_witness"
    try:
        setattr(cls, name, 73)
    except TypeError:
        print(label, "immutable", hasattr(cls, name))
    else:
        print(label, "mutable", getattr(cls, name))
        delattr(cls, name)
        print(label, "removed", hasattr(cls, name))


@functools.lru_cache(maxsize=2)
def cached(value):
    return value + 1


partial = functools.partial(int)
comparison_key = functools.cmp_to_key(lambda left, right: left - right)(1)
native = [
    ("mappingproxy", types.MappingProxyType),
    ("method", types.MethodType),
    ("namespace", types.SimpleNamespace),
    ("cell", types.CellType),
    ("partial", type(partial)),
    ("key", type(comparison_key)),
    ("lru", type(cached)),
    ("itemgetter", type(operator.itemgetter(0))),
    ("attrgetter", type(operator.attrgetter("value"))),
    ("methodcaller", type(operator.methodcaller("lower"))),
]
for label, cls in native:
    mutation(label, cls)

for label, cls in [
    ("dynamic", types.DynamicClassAttribute),
    ("spec", importlib.machinery.ModuleSpec),
    ("builtin-loader", importlib.machinery.BuiltinImporter),
    ("frozen-loader", importlib.machinery.FrozenImporter),
    ("cache-info", type(cached.cache_info())),
]:
    mutation(label, cls)

# An immutable class still admits mutation of the instances that own a dict.
namespace = types.SimpleNamespace()
namespace.value = 4
partial.value = 5
cached.value = 6
print("instance-values", namespace.value, partial.value, cached.value)
print("native-call", partial("17"), cached(9), operator.itemgetter(1)((2, 3)))


# Annotation eligibility is heap origin, independent of mutation permission.
import io
import sys

for label, cls in native + [
    ("exception-group", ExceptionGroup),
    ("io-base", io.IOBase),
    ("file-io", io.FileIO),
    ("bytes-io", io.BytesIO),
]:
    try:
        annotations = cls.__annotations__
    except AttributeError:
        print(label, "annotations", "absent")
    else:
        print(label, "annotations", isinstance(annotations, dict), cls.__annotations__ is annotations)
    if sys.version_info >= (3, 14):
        try:
            evaluator = cls.__annotate__
        except AttributeError:
            print(label, "annotate", "absent")
        else:
            print(label, "annotate", evaluator is None)
    try:
        child = type("PolicyChild", (cls,), {})
    except TypeError:
        print(label, "basetype", False)
    else:
        print(label, "basetype", True)
        child.policy_child_attribute = 1
        print(label, "child-mutable", child.policy_child_attribute)

for label, cls in [("exception-group", ExceptionGroup), ("io-base", io.IOBase)]:
    mutation(label, cls)

# Public ABC mutation must never enter the immutable physical _io hierarchy.
import _io
import asyncio

io.IOBase.public_namespace_witness = 9
print("io-isolation", hasattr(io.FileIO, "public_namespace_witness"),
      hasattr(io.BytesIO, "public_namespace_witness"),
      hasattr(io.StringIO, "public_namespace_witness"))
del io.IOBase.public_namespace_witness
for concrete, abstract in [(io.BytesIO, io.BufferedIOBase),
                           (io.StringIO, io.TextIOBase)]:
    stream = concrete()
    print("io-virtual", isinstance(stream, abstract), isinstance(stream, io.IOBase),
          issubclass(concrete, abstract), abstract in concrete.__mro__)
    try:
        stream.fileno()
    except io.UnsupportedOperation:
        print("io-exception", _io.UnsupportedOperation is io.UnsupportedOperation)
    stream.close()
mutation("native-io-base", _io._IOBase)

import errno

_unsupported_name = io.UnsupportedOperation.__name__
for _name in ("OSError", "IOError", "EnvironmentError"):
    io.UnsupportedOperation.__name__ = _name
    _error = io.UnsupportedOperation(errno.ENOENT, "missing")
    print("io-promotion", type(_error) is io.UnsupportedOperation,
          isinstance(_error, FileNotFoundError))
io.UnsupportedOperation.__name__ = _unsupported_name

# Mutable schema classes keep their identity after a metadata rename.
print("cancel-annotations", asyncio.CancelledError.__annotations__)
mutation("cancel", asyncio.CancelledError)
_cancel_name = asyncio.CancelledError.__name__
asyncio.CancelledError.__name__ = "RenamedCancellation"
try:
    raise asyncio.CancelledError("cancelled")
except asyncio.CancelledError as error:
    print("cancel-identity", type(error) is asyncio.CancelledError, str(error))
asyncio.CancelledError.__name__ = _cancel_name

# The class and instance use the same mutable namespace for native methods.
_derive = ExceptionGroup.derive
_derive_owned = "derive" in vars(ExceptionGroup)


def patched_derive(self, exceptions):
    return "patched"


ExceptionGroup.derive = patched_derive
print("group-namespace", ExceptionGroup.derive is patched_derive,
      vars(ExceptionGroup)["derive"] is patched_derive,
      ExceptionGroup("group", [ValueError()]).derive([]))
if _derive_owned:
    ExceptionGroup.derive = _derive
else:
    del ExceptionGroup.derive

_hash = ExceptionGroup.__hash__
_hash_owned = "__hash__" in vars(ExceptionGroup)
ExceptionGroup.__hash__ = lambda self: 17
print("group-hash", hash(ExceptionGroup("hash", [ValueError()])))
if _hash_owned:
    ExceptionGroup.__hash__ = _hash
else:
    del ExceptionGroup.__hash__

_group = ExceptionGroup("eq", [ValueError()])
_original_hash = hash(_group)
ExceptionGroup.__eq__ = lambda self, other: True
print("group-live-equality", hash(_group) == _original_hash)
del ExceptionGroup.__eq__


class DefinedEquality:
    def __eq__(self, other):
        return True


print("construction-hash", DefinedEquality.__hash__ is None)
try:
    hash(DefinedEquality())
except TypeError:
    print("construction-unhashable")


class LateEquality:
    pass


_late = LateEquality()
_late_hash = hash(_late)
LateEquality.__eq__ = lambda self, other: True
print("late-equality", hash(_late) == _late_hash)
