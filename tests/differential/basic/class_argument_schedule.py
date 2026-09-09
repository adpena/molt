"""Class bases and keyword expansions follow __build_class__ argument order."""

events = []


class Bases:
    def __iter__(self):
        events.append("bases.iter")
        return iter(())


def metaclass():
    events.append("metaclass")
    return type


class Example(*Bases(), metaclass=metaclass()):
    events.append("body")


print(events)
events.clear()


class Keywords:
    def keys(self):
        events.append("keywords.keys")
        return ["metaclass"]

    def __getitem__(self, key):
        events.append("keywords.get")
        return type


def late():
    events.append("late")
    return type


try:

    class Duplicate(*Bases(), **Keywords(), metaclass=late()):
        events.append("unreachable")
except TypeError:
    events.append("duplicate")

print(events)

events.clear()
prepared_flag = False


class PreparedNamespace(dict):
    def __getitem__(self, key):
        if key == "probe":
            events.append("namespace.get")
            return "mapping"
        return super().__getitem__(key)

    def __setitem__(self, key, value):
        if key == "probe":
            events.append("namespace.set")
            return
        return super().__setitem__(key, value)

    def __delitem__(self, key):
        if key == "probe":
            events.append("namespace.del")
            return
        return super().__delitem__(key)


class PreparedMeta(type):
    @classmethod
    def __prepare__(cls, name, bases):
        global prepared_flag
        prepared_flag = True
        return PreparedNamespace()


class Prepared(metaclass=PreparedMeta):
    before = prepared_flag
    probe = 7
    first = probe
    del probe
    second = probe


print("prepared-namespace", Prepared.before, Prepared.first, Prepared.second, events)


import sys


annotation_global = "global"


class AnnotationNamespace(dict):
    def __getitem__(self, key):
        if key == "probe":
            return "prepared"
        return super().__getitem__(key)


class AnnotationMeta(type):
    @classmethod
    def __prepare__(cls, name, bases):
        return AnnotationNamespace(
            annotation_global="namespace", T="stored-type", probe="stored"
        )


class AnnotationOwner(metaclass=AnnotationMeta):
    global annotation_global

    def helper[T](global_value: annotation_global, parameter: T, lookup: probe):
        pass


annotation_values = AnnotationOwner.helper.__annotations__
assert annotation_values["global_value"] == "global"
if sys.version_info >= (3, 14):
    assert annotation_values["parameter"] == "stored-type"
    assert annotation_values["lookup"] == "stored"
else:
    assert annotation_values["parameter"] is AnnotationOwner.helper.__type_params__[0]
    assert annotation_values["lookup"] == "prepared"
print("annotation-namespace-version-policy", True)


nonlocal_namespace_events = []


class NonlocalNamespace(dict):
    def __getitem__(self, key):
        if key == "cell":
            nonlocal_namespace_events.append("read")
            return "mapping"
        return super().__getitem__(key)

    def __setitem__(self, key, value):
        if key == "cell":
            nonlocal_namespace_events.append("write")
        return super().__setitem__(key, value)

    def __delitem__(self, key):
        if key == "cell":
            nonlocal_namespace_events.append("delete")
        return super().__delitem__(key)


class NonlocalMeta(type):
    @classmethod
    def __prepare__(cls, name, bases):
        return NonlocalNamespace()


def nonlocal_namespace_target():
    cell = "initial"

    class NonlocalOwner(metaclass=NonlocalMeta):
        nonlocal cell
        cell = "written"
        observed = cell
        del cell

    try:
        return cell
    except UnboundLocalError:
        return NonlocalOwner.observed


assert nonlocal_namespace_target() == "mapping"
assert nonlocal_namespace_events == ["read"]
print("nonlocal-namespace-read-write-policy", nonlocal_namespace_events)


key_callback_flag = False


class NamespaceKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        global key_callback_flag
        key_callback_flag = True
        return str.__eq__(self, other)


class InitiallyPlainOwner:
    global key_callback_flag
    locals()[NamespaceKey("probe")] = "stored"
    key_callback_flag = False
    observed = probe
    after = key_callback_flag


assert InitiallyPlainOwner.observed == "stored"
assert InitiallyPlainOwner.after is True
print("plain-namespace-retained-key-callback", InitiallyPlainOwner.after)
