"""Purpose: ModuleSpec type identity, construction, parent and mutation contract."""

import _frozen_importlib
import importlib.machinery as machinery
import importlib.util
import json

ModuleSpec = machinery.ModuleSpec

# Every importlib consumer shares the one public class.
print(isinstance(ModuleSpec, type), ModuleSpec.__name__, ModuleSpec.__qualname__)
print(ModuleSpec.__module__)
assert ModuleSpec.__module__ == "_frozen_importlib"
print(_frozen_importlib.ModuleSpec is ModuleSpec)
print(type(json.__spec__) is ModuleSpec)
print(type(importlib.util.find_spec("json")) is ModuleSpec)
print(type(importlib.util.spec_from_loader("pkg.made", None)) is ModuleSpec)

spec = ModuleSpec("pkg.leaf", None)
print(type(spec) is ModuleSpec)
print(spec.name, spec.loader, spec.origin, spec.loader_state, spec.cached)
print(spec.submodule_search_locations, spec.has_location, repr(spec.parent))
print(repr(spec))
assert repr(spec) == "ModuleSpec(name='pkg.leaf', loader=None)"
print(type(ModuleSpec.__dict__["parent"]).__name__)

keyword = ModuleSpec(name="top", loader=None, origin="built-in", is_package=False)
print(keyword.name, keyword.origin, repr(keyword.parent))
package = ModuleSpec("pkg.sub", None, is_package=True)
print(package.submodule_search_locations, package.parent)
print(repr(package))
assert repr(package) == (
    "ModuleSpec(name='pkg.sub', loader=None, submodule_search_locations=[])"
)

try:
    ModuleSpec()
except TypeError:
    print("TypeError")

# Instances and the class stay mutable; parent derives from current fields.
spec.name = "a.b.c"
print(spec.parent)
spec.submodule_search_locations = ["/somewhere"]
print(spec.parent)
spec.extra = 7
print(spec.extra)
try:
    spec.parent = "x"
except AttributeError:
    print("AttributeError")
ModuleSpec.marker = "class-level"
print(spec.marker)
del ModuleSpec.marker
print(hasattr(spec, "marker"))


class TaggedSpec(ModuleSpec):
    def __init__(self, name, tag):
        super().__init__(name, None, is_package=True)
        self.tag = tag


tagged = TaggedSpec("pkg.tagged", "t")
print(type(tagged).__name__, isinstance(tagged, ModuleSpec))
print(tagged.tag, tagged.parent, tagged.submodule_search_locations)
print(repr(tagged))
assert repr(tagged) == (
    "TaggedSpec(name='pkg.tagged', loader=None, submodule_search_locations=[])"
)


# The CPython repr reads optional attributes once for inclusion and again for
# rendering. Locations use default formatting, unlike the repr conversion of
# name, loader and origin; the subclass name is resolved only after the fields.
events = []


class ReprProbe:
    def __repr__(self):
        events.append("repr-origin")
        return "<origin>"


class FormatProbe:
    def __repr__(self):
        events.append("repr-locations")
        return "<wrong-repr>"

    def __format__(self, spec):
        events.append("format-locations:" + spec)
        return "<formatted>"


class TracedSpec(ModuleSpec):
    def __getattribute__(self, name):
        if name in {
            "name",
            "loader",
            "origin",
            "submodule_search_locations",
            "__class__",
        }:
            events.append(name)
        return object.__getattribute__(self, name)


traced = TracedSpec("order", None)
traced.origin = ReprProbe()
traced.submodule_search_locations = FormatProbe()
events.clear()
rendered = repr(traced)
print(rendered)
assert rendered == (
    "TracedSpec(name='order', loader=None, origin=<origin>, "
    "submodule_search_locations=<formatted>)"
)
assert events == [
    "name",
    "loader",
    "origin",
    "origin",
    "repr-origin",
    "submodule_search_locations",
    "submodule_search_locations",
    "format-locations:",
    "__class__",
]


class FailingRepr:
    def __repr__(self):
        raise ValueError("loader repr failed")


class FailingFormat:
    def __format__(self, spec):
        raise RuntimeError("locations format failed")


failing = ModuleSpec("fail", FailingRepr())
try:
    repr(failing)
except ValueError as exc:
    print(type(exc).__name__, str(exc))
    assert str(exc) == "loader repr failed"
else:
    raise AssertionError("loader repr failure was swallowed")
failing.loader = None
failing.submodule_search_locations = FailingFormat()
try:
    repr(failing)
except RuntimeError as exc:
    print(type(exc).__name__, str(exc))
    assert str(exc) == "locations format failed"
else:
    raise AssertionError("locations format failure was swallowed")


class ObservedSpec(ModuleSpec):
    def __setattr__(self, name, value):
        if name == "name":
            value = "observed." + value
        object.__setattr__(self, name, value)


observed = ObservedSpec("leaf", None)
print(observed.name, observed.parent)
