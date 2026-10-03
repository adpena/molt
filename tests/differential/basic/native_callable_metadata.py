"""Native callable roles, public metadata, and descriptor binding against CPython.

Native functions do not become descriptors merely because a class stores them.
Method descriptors bind only compatible receivers; their private binder state
must not leak into Python's function metadata surface.
"""

import builtins
import inspect
import types
import io
import _io


def outcome(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__)
    else:
        print(label, "ok")


def module_roundtrip(label, value):
    original = getattr(value, "__module__", None)
    value.__module__ = "native-probe"
    try:
        print("module-write", label, value.__module__)
        del value.__module__
        print("module-delete", label, value.__module__ is None)
    finally:
        value.__module__ = original


values = []
append = values.append
length = values.__len__
fromkeys = dict.__dict__["fromkeys"]
roles = (
    ("function", len, types.BuiltinFunctionType),
    ("bound-native", append, types.BuiltinMethodType),
    ("method-descriptor", list.append, types.MethodDescriptorType),
    ("wrapper-descriptor", list.__len__, types.WrapperDescriptorType),
    ("bound-wrapper", length, types.MethodWrapperType),
    ("classmethod-descriptor", fromkeys, types.ClassMethodDescriptorType),
)
attributes = (
    "__dict__", "__defaults__", "__kwdefaults__", "__annotations__",
    "__code__", "__globals__", "__closure__", "__func__", "__self__",
    "__get__", "__objclass__", "__module__",
)
for label, value, expected_type in roles:
    print("role", label, type(value) is expected_type, type(value).__name__)
    print("metadata", label, tuple(hasattr(value, name) for name in attributes))
    print("name", label, value.__name__, value.__qualname__)
    outcome(label + "-defaults-write", lambda: setattr(value, "__defaults__", (9,)))
    outcome(label + "-annotations-write", lambda: setattr(value, "__annotations__", {}))
    outcome(label + "-defaults-delete", lambda: delattr(value, "__defaults__"))
    outcome(label + "-module-roundtrip", lambda: module_roundtrip(label, value))

print("bound-self", append.__self__ is values, length.__self__ is values)
append.__module__ = "bound-probe"
print("bound-module-owner", append.__module__, values.append.__module__ is None,
      not hasattr(list.append, "__module__"))
del append.__module__
print("descriptor-owner", list.append.__objclass__ is list,
      list.__len__.__objclass__ is list, fromkeys.__objclass__ is dict)
print("unbound", list.append.__get__(None, list) is list.append,
      list.__len__.__get__(None, list) is list.__len__)
list.append.__get__(values, list)(7)
print("descriptor-call", values, list.__len__.__get__(values, list)())
outcome("method-wrong-receiver", lambda: list.append.__get__({}))
outcome("wrapper-wrong-receiver", lambda: list.__len__.__get__({}))
outcome("classmethod-wrong-owner", lambda: fromkeys.__get__(None, list))


class ChildList(list):
    pass


child = ChildList()
list.append.__get__(child, ChildList)(11)
print("subclass-receiver", list(child), list.__len__.__get__(child, ChildList)())


class ChildDict(dict):
    pass


constructor = fromkeys.__get__(None, ChildDict)
created = constructor(("x", "y"), 17)
print("classmethod-bind", constructor.__self__ is ChildDict,
      type(created) is ChildDict, dict(created))


class StoredNative:
    ordinary = len
    bound = append
    explicit_static = staticmethod(len)
    explicit_class = classmethod(len)

    def __len__(self):
        return 999


stored = StoredNative()
print("native-nonbinding", StoredNative.ordinary is len, stored.ordinary is len,
      stored.ordinary([1, 2, 3]), stored.explicit_static([1, 2]))
print("bound-nonbinding", stored.bound is append, stored.bound.__self__ is values)
stored.bound(8)
print("bound-call", values)
outcome("explicit-classmethod-arity", lambda: stored.explicit_class([]))
print("static-factory", type(str.maketrans) is types.BuiltinFunctionType,
      str.maketrans("a", "b"))
print("native-new", type(list.__new__) is types.BuiltinFunctionType,
      type(list.__new__(ChildList)) is ChildList)


def managed(self, value=23):
    return value


managed.extra = "shared"
managed.__annotations__ = {"value": int}


class StoredManaged:
    method = managed


managed_receiver = StoredManaged()
bound = managed_receiver.method
print("managed-binding", type(bound) is types.MethodType,
      bound.__func__ is managed, bound.__self__ is managed_receiver, bound())
print("managed-metadata", bound.__defaults__, bound.extra,
      bound.__annotations__["value"] is int, bound.__dict__ is managed.__dict__)
outcome("bound-managed-write", lambda: setattr(bound, "extra", "changed"))
print("managed-preserved", managed.extra)

# Forwarded metadata must bind once, and method identity must not be inferred
# from a coincidentally named __func__ or __self__ attribute.
managed_signature = inspect.signature(managed_receiver.method)
print("managed-signature", tuple(managed_signature.parameters),
      managed_signature.parameters["value"].default)
managed.__text_signature__ = "(self, visible=5, /)"
print("managed-text-signature", str(inspect.signature(managed_receiver.method)))
del managed.__text_signature__
for parameters in (
    [],
    [inspect.Parameter("only", inspect.Parameter.KEYWORD_ONLY)],
    [inspect.Parameter("keywords", inspect.Parameter.VAR_KEYWORD)],
):
    managed.__signature__ = inspect.Signature(parameters)
    outcome("invalid-managed-signature", lambda: inspect.signature(managed_receiver.method))
managed.__signature__ = inspect.Signature([
    inspect.Parameter("receiver", inspect.Parameter.POSITIONAL_ONLY),
    inspect.Parameter("visible", inspect.Parameter.KEYWORD_ONLY, default=9),
])
print("managed-override", str(inspect.signature(managed_receiver.method)))
managed.__signature__ = inspect.Signature([
    inspect.Parameter("arguments", inspect.Parameter.VAR_POSITIONAL),
])
print("managed-variadic", str(inspect.signature(managed_receiver.method)))
del managed.__signature__


class MethodKinds:
    @classmethod
    def class_bound(receiver, value=3):
        return value

    @staticmethod
    def static(value=4):
        return value


for method_owner in (MethodKinds, MethodKinds()):
    print("managed-method-kinds", str(inspect.signature(method_owner.class_bound)),
          str(inspect.signature(method_owner.static)))


class FunctionShaped:
    __func__ = managed
    __self__ = object()

    def __call__(self, actual=31):
        return actual


print("function-shaped", str(inspect.signature(FunctionShaped())))


class SignatureFailure:
    @property
    def __signature__(self):
        raise RuntimeError("original signature lookup failure")

    def __call__(self, *args):
        return args


class CallableFailure:
    __call__ = SignatureFailure()


outcome("callable-signature-failure", lambda: inspect.signature(CallableFailure()))

# Construction metadata is complete before Python-defined builtins become
# readonly native callables. Clinic's implicit receiver is visible in the raw
# text but removed from a bound callable's public inspect signature.
for name in ("compile", "input", "breakpoint", "eval", "exec", "pow"):
    value = getattr(builtins, name)
    try:
        signature = str(inspect.signature(value))
    except ValueError:
        signature = "ValueError"
    print("builtin-signature", name, repr(value.__text_signature__), signature,
          value.__self__ is builtins)
    outcome("builtin-signature-readonly-" + name,
            lambda: setattr(value, "__text_signature__", "()"))

for name, value in (("append-descriptor", list.append), ("append-bound", [].append),
                    ("wrapper-descriptor", object.__str__),
                    ("wrapper-bound", object().__str__), ("classmethod", dict.fromkeys)):
    print("descriptor-signature", name, str(inspect.signature(value)))

print("open-provider", builtins.open is io.open is _io.open,
      builtins.open.__self__ is _io, builtins.open.__module__)
print("open-signature", str(inspect.signature(io.open)))
outcome("open-invalid-mode", lambda: io.open("unopened-path", "rw"))

for value in (list, [], ChildList, ChildList()):
    names = dir(value)
    print("enumeration", all(name in names for name in ("append", "extend", "__len__")),
          len(names) == len(set(names)))
