"""The shared intrinsic registry installs the real resolver and capability checks."""

from __future__ import annotations

import builtins
import sys
from types import ModuleType

import pytest

from molt._intrinsic_symbols import INTRINSIC_SIGNATURES
from tests.stdlib_intrinsic_registry import (
    RESOLVER,
    capability_grants,
    bind_struct_sequence_module,
    intrinsic_registry,
    struct_sequence_intrinsics,
    struct_sequence_values,
)


def test_capability_grants_are_registered_runtime_intrinsics() -> None:
    names = capability_grants()
    assert names.keys() == {"molt_capabilities_has", "molt_capabilities_trusted"}
    assert names.keys() <= INTRINSIC_SIGNATURES.keys()
    assert names["molt_capabilities_trusted"]() is True
    assert names["molt_capabilities_has"]("fs.read") is True


def test_registry_resolves_through_the_real_resolver_and_restores() -> None:
    before = sys.modules.get("_intrinsics")
    behavior = {"molt_example_behavior": len}
    with intrinsic_registry(behavior) as resolver:
        assert resolver.__file__ == str(RESOLVER)
        assert sys.modules["_intrinsics"] is resolver
        assert resolver.require_intrinsic("molt_example_behavior") is len
        assert resolver.require_intrinsic("molt_capabilities_has")("fs.read") is True
        with pytest.raises(RuntimeError, match="intrinsic unavailable: molt_absent"):
            resolver.require_intrinsic("molt_absent")
    assert sys.modules.get("_intrinsics") is before
    assert not hasattr(builtins, "_molt_intrinsics_strict")


def test_registry_without_capabilities_holds_only_the_given_behavior() -> None:
    with intrinsic_registry({}, with_capabilities=False) as resolver:
        with pytest.raises(
            RuntimeError, match="intrinsic unavailable: molt_capabilities_has"
        ):
            resolver.require_intrinsic("molt_capabilities_has")


@pytest.mark.parametrize("exceptional", [False, True])
def test_explicit_struct_sequence_provider_uses_c_types_and_restores(exceptional):
    import gc
    import time
    import weakref

    module = ModuleType("native_record_consumer")
    providers = struct_sequence_intrinsics()
    view = bind_struct_sequence_module(module, providers)
    assert view is not None
    exec("import sys; from sys import _struct_sequence_new", module.__dict__)
    assert (
        module.sys is view and module._struct_sequence_new is view._struct_sequence_new
    )

    class Marker:
        pass

    marker = Marker()
    weak = weakref.ref(marker)
    sentinel = RuntimeError("loader failed after native publication")
    try:
        with intrinsic_registry(providers) as resolver:
            assert (
                resolver.require_intrinsic("molt_os_stat_result_type")()
                is __import__("os").stat_result
            )
            cls = view._struct_sequence_type(
                "struct_time",
                (
                    "tm_year",
                    "tm_mon",
                    "tm_mday",
                    "tm_hour",
                    "tm_min",
                    "tm_sec",
                    "tm_wday",
                    "tm_yday",
                    "tm_isdst",
                    "tm_zone",
                    "tm_gmtoff",
                ),
                module="time",
                visible=9,
                constructor=3,
                static_origin=False,
            )
            assert cls is time.struct_time and cls.__bases__ == (tuple,)
            value = view._struct_sequence_new(
                cls, (2026, 1, 2, 3, 4, 5, 4, 2, 0, marker, 3600)
            )
            assert (
                len(value) == 9 and value.tm_zone is marker and value.tm_gmtoff == 3600
            )
            del marker
            gc.collect()
            assert weak() is not None
            physical = struct_sequence_values(value)
            assert physical[9] is weak() and len(physical) == cls.n_fields
            del value
            gc.collect()
            assert weak() is not None
            del physical
            gc.collect()
            assert weak() is None
            with pytest.raises(
                RuntimeError, match="intrinsic unavailable: molt_absent"
            ):
                resolver.require_intrinsic("molt_absent")
            if exceptional:
                raise sentinel
    except RuntimeError as error:
        assert exceptional and error is sentinel
    assert module.sys is view
    assert module._struct_sequence_new is view._struct_sequence_new


def test_struct_sequence_providers_are_never_implicitly_granted():
    with intrinsic_registry() as resolver:
        for name in (
            "molt_structseq_type",
            "molt_structseq_new",
            "molt_os_stat_result_type",
        ):
            with pytest.raises(RuntimeError, match=f"intrinsic unavailable: {name}"):
                resolver.require_intrinsic(name)


def test_partial_struct_sequence_provider_restores_existing_context():
    module = ModuleType("partial_native_record_consumer")
    namespace = {"__import__": builtins.__import__}
    module.__dict__["__builtins__"] = namespace
    before = sys.modules.get("_intrinsics")
    behavior = {"molt_structseq_type": object()}
    with pytest.raises(ValueError, match="must be supplied together"):
        with intrinsic_registry(behavior):
            bind_struct_sequence_module(module, behavior)
    assert module.__dict__["__builtins__"] is namespace
    assert sys.modules.get("_intrinsics") is before


@pytest.mark.parametrize("module", ["posix", "nt"])
def test_uname_result_provider_keeps_native_target_identity_without_uname(
    monkeypatch, module
):
    import gc
    import os
    import weakref

    from tests import stdlib_intrinsic_registry as registry
    from tests.process_guard_common import ModuleView

    selected_os = ModuleView(os)
    if hasattr(selected_os, "uname"):
        del selected_os.uname
    monkeypatch.setattr(registry, "os", selected_os)
    assert not hasattr(registry.os, "uname")
    descriptors = []
    initialize = registry._UnameDescriptor.__init__

    def observe_descriptor(self, *args):
        initialize(self, *args)
        descriptors.append(weakref.ref(self))

    monkeypatch.setattr(registry._UnameDescriptor, "__init__", observe_descriptor)
    fields = ("sysname", "nodename", "release", "version", "machine")
    values = ("kernel", "node", "release", "version", "machine")
    providers = struct_sequence_intrinsics()
    consumer = ModuleType("uname_record_consumer")
    view = bind_struct_sequence_module(consumer, providers)
    assert view is not None
    with intrinsic_registry(providers):
        cls = view._struct_sequence_type(
            "uname_result", fields, module=module, static_origin=False
        )
        assert cls.__module__ == module and cls.__name__ == "uname_result"
        assert cls.__bases__ == (tuple,)
        assert (cls.n_fields, cls.n_sequence_fields, cls.n_unnamed_fields) == (5, 5, 0)
        assert (cls is os.uname_result) == (module == os.uname_result.__module__)
        assert type(cls.sysname) is type(os.uname_result.sysname)
        assert cls.__doc__ == os.uname_result.__doc__
        assert tuple(cls(values)) == values
        with pytest.raises(TypeError):
            cls(values[:-1])
        record = view._struct_sequence_new(cls, values)
        type_ref = weakref.ref(cls)
        with pytest.raises(AssertionError, match="unprovided native struct sequence"):
            view._struct_sequence_type(
                "uname_result", fields, module="foreign", static_origin=False
            )
    del cls, providers, view, consumer
    gc.collect()
    # The escaped instance retains its type and borrowed descriptor strings
    # after registry teardown. No provider closure may be its storage owner.
    assert type(record) is type_ref()
    if module != os.uname_result.__module__:
        assert len(descriptors) == 1 and descriptors[0]() is not None
    else:
        assert descriptors == []
    assert tuple(record) == values
    assert tuple(getattr(record, field) for field in fields) == values
    assert repr(record) == (
        f"{module}.uname_result(sysname='kernel', nodename='node', "
        "release='release', version='version', machine='machine')"
    )
    assert record.__reduce__()[0] is type(record)
    del record
    gc.collect()
    if module != os.uname_result.__module__:
        assert type_ref() is None
        assert descriptors[0]() is None


@pytest.mark.parametrize("exceptional", [False, True])
def test_struct_sequence_registry_never_publishes_helpers_on_host_modules(exceptional):
    import os
    import time
    from concurrent.futures import ThreadPoolExecutor

    missing = object()

    def observe_host():
        return (
            getattr(sys, "_struct_sequence_type", missing),
            getattr(sys, "_struct_sequence_new", missing),
            time.struct_time,
            os.stat_result,
            builtins.__import__,
            sys.modules["sys"],
        )

    before = observe_host()
    providers = struct_sequence_intrinsics()
    consumer = ModuleType("isolated_record_consumer")
    sentinel = RuntimeError("source load failed")
    with ThreadPoolExecutor(max_workers=1) as reader:
        try:
            with intrinsic_registry(providers):
                view = bind_struct_sequence_module(consumer, providers)
                exec(
                    "import sys; from sys import _struct_sequence_new; "
                    "import time; import os\n"
                    "def retained_import():\n    import sys\n    return sys\n",
                    consumer.__dict__,
                )
                assert consumer.sys is view and view is not sys
                assert consumer._struct_sequence_new is providers["molt_structseq_new"]
                assert consumer.time is time and consumer.os is os
                assert reader.submit(observe_host).result() == before
                if exceptional:
                    raise sentinel
        except RuntimeError as error:
            assert exceptional and error is sentinel
        assert consumer.retained_import() is view
        assert reader.submit(observe_host).result() == before
