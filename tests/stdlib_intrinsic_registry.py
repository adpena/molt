"""The intrinsic registry for tests that run Molt stdlib sources on CPython.

A Molt stdlib module asks ``_intrinsics.require_intrinsic`` for its runtime
hooks, and the host interpreter has no runtime to answer. A test therefore
installs a registry: the real resolver (``src/molt/stdlib/_intrinsics.py``) in
strict mode, reading ``builtins._molt_intrinsics``. The registry always grants
the capability checks (``molt_capabilities_has``, ``molt_capabilities_trusted``);
a test passes only the behavior it exercises. There are no readiness anchors:
spec 0016 admits no intrinsic that exists only to make a module count.

A probe that runs in a child interpreter calls :func:`install_registry` once;
an in-process test uses :func:`intrinsic_registry`, which restores what it
replaced.
"""

from __future__ import annotations

import builtins
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
import importlib.util
from pathlib import Path
import sys
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
RESOLVER = ROOT / "src" / "molt" / "stdlib" / "_intrinsics.py"

_CAPABILITY_CHECKS = ("molt_capabilities_has", "molt_capabilities_trusted")
_REGISTRY_BUILTINS = ("_molt_intrinsics", "_molt_intrinsics_strict")


def _grant(*_args: object) -> bool:
    return True


def capability_grants() -> dict[str, Callable[..., bool]]:
    """The capability checks, each granting every capability."""

    return {name: _grant for name in _CAPABILITY_CHECKS}


def _load_resolver() -> ModuleType:
    spec = importlib.util.spec_from_file_location("_intrinsics", RESOLVER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load the intrinsic resolver from {RESOLVER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def install_registry(
    behavior: Mapping[str, object] = {},  # noqa: B006 - read only
    *,
    with_capabilities: bool = True,
) -> ModuleType:
    """Install the resolver and a strict registry for the rest of this process.

    ``with_capabilities=False`` leaves out the capability checks, for a test that
    proves what a module does without them. Returns the installed resolver module.
    """

    resolver = _load_resolver()
    sys.modules["_intrinsics"] = resolver
    builtins._molt_intrinsics = {
        **(capability_grants() if with_capabilities else {}),
        **behavior,
    }  # type: ignore[attr-defined]
    builtins._molt_intrinsics_strict = True  # type: ignore[attr-defined]
    return resolver


@contextmanager
def intrinsic_registry(
    behavior: Mapping[str, object] = {},  # noqa: B006 - read only
    *,
    with_capabilities: bool = True,
) -> Iterator[ModuleType]:
    """:func:`install_registry` for one in-process block, then restore."""

    missing = object()
    saved_module = sys.modules.get("_intrinsics", missing)
    saved = {name: getattr(builtins, name, missing) for name in _REGISTRY_BUILTINS}
    try:
        yield install_registry(behavior, with_capabilities=with_capabilities)
    finally:
        if saved_module is missing:
            sys.modules.pop("_intrinsics", None)
        else:
            sys.modules["_intrinsics"] = saved_module  # type: ignore[assignment]
        for name, value in saved.items():
            if value is missing:
                if hasattr(builtins, name):
                    delattr(builtins, name)
            else:
                setattr(builtins, name, value)
