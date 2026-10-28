"""Purpose: ModuleSpec type identity, construction, parent and mutation contract."""

import _frozen_importlib
import _frozen_importlib_external as external
import importlib.machinery as machinery
import importlib.util
import json
import sys

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


def assert_default_cache(label: str, cached: str, source_stem: str) -> None:
    # The implementation tag is intentionally different between runtimes.
    # Assert the whole path independently before emitting a portable witness.
    expected = (
        "cache_root/__pycache__/"
        + source_stem
        + "."
        + sys.implementation.cache_tag
        + ".pyc"
    )
    assert cached.replace("\\", "/") == expected
    print(label, True)


def run_spec_cache_contract() -> None:
    spec = importlib.machinery.ModuleSpec(
        "cache_contract", None, origin="cache_root/alpha.py"
    )
    print("cache-default", spec.has_location, spec.cached)
    events = []

    class Location:
        def __bool__(self):
            events.append("truth")
            return True

    spec.has_location = Location()
    print("cache-location", spec.has_location, events)
    first = spec.cached
    assert_default_cache("cache-derived", first, "alpha")
    spec.origin = "cache_root/beta.py"
    print("cache-retained", spec.cached == first)
    spec.cached = None
    assert_default_cache("cache-reset", spec.cached, "beta")
    spec.cached = 0
    print("cache-explicit", spec.cached)
    marker = ValueError("cache-location-truth")

    class BadLocation:
        def __bool__(self):
            events.append("truth-error")
            raise marker

    try:
        spec.has_location = BadLocation()
    except ValueError as error:
        print("cache-truth-error", error is marker, spec.has_location, events)
    else:
        raise AssertionError("location assignment must propagate truth failure")

    bytecode = importlib.machinery.ModuleSpec(
        "cache_bytecode", None, origin="cache_root/alpha.pyc"
    )
    bytecode.has_location = True
    print("cache-bytecode", bytecode.cached is bytecode.origin)

    class ObservedOrigin(str):
        def endswith(self, suffix):
            events.append(("suffix", suffix))
            raise marker

    observed = importlib.machinery.ModuleSpec(
        "cache_observed", None, origin=ObservedOrigin("cache_root/observed.py")
    )
    observed.has_location = True
    try:
        observed.cached
    except ValueError as error:
        print("cache-origin-error", error is marker, events[-1])
    else:
        raise AssertionError("cache derivation must observe origin.endswith")


# The host's development bytecode-cache directory is not a guest test input.
# Select the default public policy explicitly for these derivation controls.
previous_cache_prefix = sys.pycache_prefix
sys.pycache_prefix = None
assert sys.flags.optimize == 0
run_spec_cache_contract()
repeated = ModuleSpec("repeated_separators", None, origin="cache_root///repeated.py")
repeated.has_location = True
assert_default_cache("cache-repeated-separators", repeated.cached, "repeated")

# Suffixes are shared mutable policy objects; rebinding a facade attribute
# differs from changing the defining namespace used by cached derivation.
source_suffixes = external.SOURCE_SUFFIXES
bytecode_suffixes = external.BYTECODE_SUFFIXES
print(
    "suffix-owners",
    source_suffixes is machinery.SOURCE_SUFFIXES,
    bytecode_suffixes is machinery.BYTECODE_SUFFIXES,
    external.DEBUG_BYTECODE_SUFFIXES is bytecode_suffixes,
    external.OPTIMIZED_BYTECODE_SUFFIXES is bytecode_suffixes,
)
source_suffixes.append(".molt_cache_probe")
bytecode_suffixes.append(".molt_bytecode_probe")
try:
    source = ModuleSpec("source_probe", None, origin="cache_root/probe.molt_cache_probe")
    source.has_location = True
    assert_default_cache("suffix-mutation", source.cached, "probe")
    bytecode = ModuleSpec(
        "bytecode_probe", None, origin="cache_root/probe.molt_bytecode_probe"
    )
    bytecode.has_location = True
    print("bytecode-suffix-mutation", bytecode.cached is bytecode.origin)
finally:
    source_suffixes.pop()
    bytecode_suffixes.pop()

machinery.SOURCE_SUFFIXES = [".machinery_only"]
try:
    unchanged = ModuleSpec("unchanged_policy", None, origin="cache_root/unchanged.py")
    unchanged.has_location = True
    assert_default_cache("machinery-rebinding", unchanged.cached, "unchanged")
finally:
    machinery.SOURCE_SUFFIXES = source_suffixes

external.SOURCE_SUFFIXES = [".external_policy"]
try:
    changed = ModuleSpec("changed_policy", None, origin="cache_root/changed.external_policy")
    changed.has_location = True
    former = ModuleSpec("former_policy", None, origin="cache_root/former.py")
    former.has_location = True
    assert_default_cache("external-rebinding", changed.cached, "changed")
    print("external-former-suffix", former.cached)
finally:
    external.SOURCE_SUFFIXES = source_suffixes

previous_tag = sys.implementation.cache_tag
try:
    pending = ModuleSpec("pending_tag", None, origin="cache_root/pending.py")
    pending.has_location = True
    sys.implementation.cache_tag = "contract-tag"
    assert_default_cache("cache-live-tag", pending.cached, "pending")
    sys.implementation.cache_tag = None
    unavailable = ModuleSpec("no_tag", None, origin="cache_root/no_tag.py")
    unavailable.has_location = True
    print("cache-unavailable", unavailable.cached)
    try:
        importlib.util.cache_from_source("cache_root/no_tag.py")
    except NotImplementedError:
        print("cache-public-unavailable", True)
    else:
        raise AssertionError("the public cache API must reject an unavailable tag")
finally:
    sys.implementation.cache_tag = previous_tag

previous_suffix = bytecode_suffixes[0]
try:
    bytecode_suffixes[0] = ".policy_cache"
    changed = ModuleSpec("changed_bytecode", None, origin="cache_root/changed.py")
    changed.has_location = True
    assert changed.cached.replace("\\", "/") == (
        "cache_root/__pycache__/changed." + sys.implementation.cache_tag + ".policy_cache"
    )
    print("cache-live-bytecode-suffix", True)
finally:
    bytecode_suffixes[0] = previous_suffix

sys.pycache_prefix = previous_cache_prefix


# Metadata preparation does not require executing a new runtime source body.
# This exercises the ZIP producer on both targets without host source access.
import tempfile
import zipfile
import zipimport

with tempfile.TemporaryDirectory(prefix="molt_zip_spec_") as root:
    archive = root + "/metadata.zip"
    with zipfile.ZipFile(archive, "w") as writer:
        writer.writestr("cache_zip_member.py", "")
    finder = zipimport.zipimporter(archive)
    zip_spec = finder.find_spec("cache_zip_member")
    zip_module = importlib.util.module_from_spec(zip_spec)
    print(
        "zip-cache-metadata",
        zip_spec.has_location,
        zip_spec.cached.endswith(".pyc"),
        zip_module.__cached__ == zip_spec.cached,
        zip_module.__file__ == zip_spec.origin,
    )
