from __future__ import annotations

import functools
import os
from collections.abc import Mapping
from pathlib import Path

from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import (
    AUTO_STDLIB_PROFILE,
    DEFAULT_STDLIB_PROFILE,
    MOLT_STDLIB_PROFILE_ENV,
)
from molt.cli import module_resolution as _module_resolution
from molt.cli.output import fail as _fail
from molt.cli.runtime_features import _runtime_builtin_features_for_profile
from molt import stdlib_intrinsic_policy as _stdlib_intrinsic_policy

_INTRINSIC_CALL_NAMES = _stdlib_intrinsic_policy.INTRINSIC_CALL_NAMES
_STDLIB_POLICY_GATE_STATUS = _stdlib_intrinsic_policy.STATUS_POLICY_GATE
_STDLIB_PROBE_INTRINSIC = _stdlib_intrinsic_policy.STDLIB_PROBE_INTRINSIC
_classify_stdlib_module_statuses = (
    _stdlib_intrinsic_policy.classify_stdlib_module_statuses
)
_is_fail_closed_import_policy_gate = (
    _stdlib_intrinsic_policy.is_fail_closed_import_policy_gate
)
_module_relative_import_base = _stdlib_intrinsic_policy.module_relative_import_base
_module_required_intrinsic_names = (
    _stdlib_intrinsic_policy.module_required_intrinsic_names
)
_same_package_intrinsic_import_closure = (
    _stdlib_intrinsic_policy.same_package_intrinsic_import_closure
)
_stdlib_module_intrinsic_status = (
    _stdlib_intrinsic_policy.stdlib_module_intrinsic_status
)
_stdlib_module_static_imports = _stdlib_intrinsic_policy.stdlib_module_static_imports


@functools.lru_cache(maxsize=8)
def _stdlib_allowlist_cached(project_root_text: str | None) -> frozenset[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md")
    if not spec_path.exists():
        if project_root_text:
            spec_path = (
                Path(project_root_text)
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
        else:
            spec_path = (
                _compiler_root()
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
    if not spec_path.exists():
        return frozenset(allowlist)
    for line in spec_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return frozenset(allowlist)


def _stdlib_allowlist() -> set[str]:
    project_root = os.environ.get("MOLT_PROJECT_ROOT")
    return set(_stdlib_allowlist_cached(project_root))


def _enforce_intrinsic_stdlib(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    json_output: bool,
) -> int | None:
    missing: list[str] = []
    probe_only: list[str] = []
    stdlib_root = stdlib_root.resolve()
    stdlib_modules: dict[str, Path] = {}
    for name, path in module_graph.items():
        if not path or not path.suffix == ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        stdlib_modules[name] = path
    statuses = _classify_stdlib_module_statuses(stdlib_modules)
    for name, status in statuses.items():
        if status == "python-only":
            missing.append(name)
        elif status == "probe-only":
            probe_only.append(name)
    if not missing:
        return None
    missing.sort()
    probe_only.sort()
    message = (
        "Intrinsic-only stdlib enforcement failed. These modules are Python-only "
        "and must be lowered to Rust intrinsics (or become thin intrinsic wrappers):\n"
        + "\n".join(f"  - {name}" for name in missing)
    )
    if probe_only:
        message += (
            "\n\nProbe-only modules in this build (thin wrappers + policy gate only):\n"
            + "\n".join(f"  - {name}" for name in probe_only)
        )
    return _fail(message, json_output, command="build")


def _profile_feature_gap_for_module(
    path: Path,
    enabled_features: frozenset[str],
) -> dict[str, list[str]]:
    """Map each excluded link-affecting feature to required intrinsics."""
    gap: dict[str, set[str]] = {}
    for symbol in _module_required_intrinsic_names(path):
        feature = link_affecting_feature_gate_for_symbol(symbol)
        if feature is None or feature in enabled_features:
            continue
        gap.setdefault(feature, set()).add(symbol)
    return {feature: sorted(symbols) for feature, symbols in gap.items()}


def _enforce_profile_feature_availability(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    stdlib_profile: str | None,
    target: str,
    json_output: bool,
) -> int | None:
    """Fail before link when the selected stdlib profile omits needed features."""
    is_wasm = target in {"wasm", "wasm-freestanding"} or target.startswith("wasm32")
    effective_triple = "wasm32-wasip1" if is_wasm else None
    enabled_features = frozenset(
        _runtime_builtin_features_for_profile(
            stdlib_profile,
            target_triple=effective_triple,
        )
    )
    profile_name = stdlib_profile or DEFAULT_STDLIB_PROFILE
    stdlib_root = stdlib_root.resolve()

    blocked: dict[str, dict[str, list[str]]] = {}
    for name, path in module_graph.items():
        if not path or path.suffix != ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        gap = _profile_feature_gap_for_module(path, enabled_features)
        for feature, symbols in gap.items():
            blocked.setdefault(feature, {})[name] = symbols
    if not blocked:
        return None

    lines: list[str] = []
    for feature in sorted(blocked):
        modules = blocked[feature]
        module_list = ", ".join(repr(m) for m in sorted(modules))
        plural = "module" if len(modules) == 1 else "modules"
        lines.append(f"  {feature}: required by {plural} {module_list}")
        for module_name in sorted(modules):
            sample = ", ".join(modules[module_name][:4])
            more = len(modules[module_name]) - 4
            if more > 0:
                sample += f", ... (+{more} more)"
            lines.append(f"      {module_name} -> {sample}")

    excluded_features = sorted(blocked)
    feature_phrase = (
        f"the {excluded_features[0]!r} runtime feature"
        if len(excluded_features) == 1
        else "runtime features " + ", ".join(repr(f) for f in excluded_features)
    )
    message = (
        f"Profile '{profile_name}' excludes {feature_phrase} that this program's "
        f"import graph requires.\n"
        f"These statically-imported stdlib modules need a feature profile "
        f"'{profile_name}' does not build, so their runtime intrinsics would be "
        f"undefined at link:\n"
        + "\n".join(lines)
        + "\n\nFeature selection is profile-driven, not import-driven: the "
        "native 'micro' profile omits heavy domains (ast, crypto, "
        "compression, ...) to keep small binaries small.\n"
        "Rebuild with the full stdlib profile, which includes these features:\n"
        "    --stdlib-profile full\n"
        "or set the environment knob the build reads as its canonical profile:\n"
        "    MOLT_STDLIB_PROFILE=full"
    )
    return _fail(message, json_output, command="build")


_CORE_STDLIB_MODULES_FULL = (
    "builtins",
    "sys",
    "types",
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_CORE_STDLIB_MODULES_MICRO = (
    "builtins",
    "sys",
)


def _core_stdlib_module_names_for_profile(
    stdlib_profile: str | None,
) -> tuple[str, ...]:
    profile = stdlib_profile or DEFAULT_STDLIB_PROFILE
    if profile in {AUTO_STDLIB_PROFILE, "micro", "edge", "standard", "server"}:
        return _CORE_STDLIB_MODULES_MICRO
    return _CORE_STDLIB_MODULES_FULL


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path], stdlib_root: Path
) -> None:
    """Add the profile's unconditional core stdlib modules to the graph.

    The profile is read from ``MOLT_STDLIB_PROFILE``, which `build()` exports
    from the value resolved by the single config authority
    (`config_resolution.resolve_stdlib_profile`). Falling back to the same
    `DEFAULT_STDLIB_PROFILE` constant that the staticlib selector uses keeps the
    closure and the linked staticlib from disagreeing.
    """
    core_modules = _core_stdlib_module_names_for_profile(
        os.environ.get(MOLT_STDLIB_PROFILE_ENV, DEFAULT_STDLIB_PROFILE)
    )
    for name in core_modules:
        path = _module_resolution._resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _looks_like_stdlib_module_name(module_name: str) -> bool:
    if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
        return True
    root = module_name.split(".", 1)[0]
    return root in {
        "__future__",
        "_collections_abc",
        "abc",
        "builtins",
        "collections",
        "dataclasses",
        "importlib",
        "os",
        "pathlib",
        "runpy",
        "signal",
        "sys",
        "test",
        "typing",
        "warnings",
        "zipfile",
        "zipimport",
    }


def _build_stdlib_like_module_flags(
    module_graph: Mapping[str, Path],
) -> dict[str, bool]:
    return {
        module_name: (
            _module_resolution._is_runtime_owned_module_path(module_path)
            or _looks_like_stdlib_module_name(module_name)
        )
        for module_name, module_path in sorted(module_graph.items())
    }
