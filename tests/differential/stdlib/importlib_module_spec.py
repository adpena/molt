"""Purpose: ModuleSpec type identity, construction, parent and mutation contract."""

import _frozen_importlib
import importlib.machinery as machinery
import importlib.util
import json

ModuleSpec = machinery.ModuleSpec

# Every importlib consumer shares the one public class.
print(isinstance(ModuleSpec, type), ModuleSpec.__name__, ModuleSpec.__qualname__)
print(_frozen_importlib.ModuleSpec is ModuleSpec)
print(type(json.__spec__) is ModuleSpec)
print(type(importlib.util.find_spec("json")) is ModuleSpec)
print(type(importlib.util.spec_from_loader("pkg.made", None)) is ModuleSpec)

spec = ModuleSpec("pkg.leaf", None)
print(type(spec) is ModuleSpec)
print(spec.name, spec.loader, spec.origin, spec.loader_state, spec.cached)
print(spec.submodule_search_locations, spec.has_location, repr(spec.parent))
print(repr(spec).startswith("ModuleSpec(name='pkg.leaf', loader=None"))
print(type(ModuleSpec.__dict__["parent"]).__name__)

keyword = ModuleSpec(name="top", loader=None, origin="built-in", is_package=False)
print(keyword.name, keyword.origin, repr(keyword.parent))
package = ModuleSpec("pkg.sub", None, is_package=True)
print(package.submodule_search_locations, package.parent)

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


class ObservedSpec(ModuleSpec):
    def __setattr__(self, name, value):
        if name == "name":
            value = "observed." + value
        object.__setattr__(self, name, value)


observed = ObservedSpec("leaf", None)
print(observed.name, observed.parent)
