"""The intrinsic registry for tests that run Molt stdlib sources on CPython.

A Molt stdlib module asks ``_intrinsics.require_intrinsic`` for its runtime
hooks, and the host interpreter has no runtime to answer. A test therefore
installs a registry: the real resolver (``src/molt/stdlib/_intrinsics.py``) in
strict mode, reading ``builtins._molt_intrinsics``. The registry always holds
the anchors the intrinsic manifest declares (every ``molt_*_runtime_ready``
predicate, ``molt_stdlib_probe`` and the capability checks), so a module that
starts requiring another anchor needs no test edit. A test passes only the
behavior it exercises.

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
import re
import sys
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "runtime" / "molt-runtime" / "src" / "intrinsics" / "manifest.pyi"
RESOLVER = ROOT / "src" / "molt" / "stdlib" / "_intrinsics.py"

# A readiness anchor is a nullary predicate a module calls once at import to
# prove its runtime support exists.
_READINESS_ANCHOR = re.compile(
    r"^def (molt_\w+_runtime_ready|molt_stdlib_probe)\(\) -> bool: \.\.\.$",
    re.MULTILINE,
)
_CAPABILITY_ANCHORS = ("molt_capabilities_has", "molt_capabilities_trusted")
_REGISTRY_BUILTINS = ("_molt_intrinsics", "_molt_intrinsics_strict")


def _grant(*_args: object) -> bool:
    return True


def anchors() -> dict[str, Callable[..., bool]]:
    """Every anchor intrinsic the manifest declares, each answering True."""

    names = _READINESS_ANCHOR.findall(MANIFEST.read_text(encoding="utf-8"))
    if not names:
        raise RuntimeError(f"{MANIFEST} declares no readiness anchors")
    return {name: _grant for name in (*names, *_CAPABILITY_ANCHORS)}


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
    with_anchors: bool = True,
) -> ModuleType:
    """Install the resolver and a strict registry for the rest of this process.

    ``with_anchors=False`` leaves out the anchors, for a test that proves what
    a module does when one is missing. Returns the installed resolver module.
    """

    resolver = _load_resolver()
    sys.modules["_intrinsics"] = resolver
    builtins._molt_intrinsics = {**(anchors() if with_anchors else {}), **behavior}  # type: ignore[attr-defined]
    builtins._molt_intrinsics_strict = True  # type: ignore[attr-defined]
    return resolver


@contextmanager
def intrinsic_registry(
    behavior: Mapping[str, object] = {},  # noqa: B006 - read only
    *,
    with_anchors: bool = True,
) -> Iterator[ModuleType]:
    """:func:`install_registry` for one in-process block, then restore."""

    missing = object()
    saved_module = sys.modules.get("_intrinsics", missing)
    saved = {name: getattr(builtins, name, missing) for name in _REGISTRY_BUILTINS}
    try:
        yield install_registry(behavior, with_anchors=with_anchors)
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
