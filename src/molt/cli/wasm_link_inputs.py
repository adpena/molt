"""Shared WASM compiler headers, provider archives and link-input policy.

Admission and final link construction consume the same resolved inputs.
Read-only toolchain readiness and linker validation remain in wasm_toolchain.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import functools
import os
from pathlib import Path

from molt.default_paths import executable_environment_value, expand_user_path

from molt.cli.command_runtime import _run_completed_command
from molt.toolchain_identity import (
    find_executable,
    stable_executable_probe,
)
from molt.rust_toolchain import resolve_rustup_proxy
from molt.source_root import compiler_source_root
from molt.llvm_toolchain import selected_wasi_c_abi_plan, selected_wasi_sdk_installation
from molt.wasi_sdk_identity import WasiCAbiProjection
from molt.wasi_sysroot import normalize_wasi_sysroot


def resolve_wasi_c_abi_plan(
    *, env: Mapping[str, str] | None = None
) -> WasiCAbiProjection:
    """Require the complete manifest-selected SDK C ABI before execution."""
    return selected_wasi_c_abi_plan(
        compiler_source_root(), environ=os.environ if env is None else env
    )


def resolve_wasi_sysroot(*, env: Mapping[str, str] | None = None) -> Path | None:
    """Read-only readiness: absence is distinct from a malformed installation."""
    environment = os.environ if env is None else env
    installation = selected_wasi_sdk_installation(
        compiler_source_root(), environ=environment
    )
    if installation is None:
        return None
    for name in ("MOLT_WASI_SYSROOT", "WASI_SYSROOT"):
        if raw := environment.get(name):
            if (
                normalize_wasi_sysroot(expand_user_path(raw, environment=environment))
                != installation.sysroot
            ):
                raise ValueError(f"{name} differs from the selected WASI SDK")
    return installation.sysroot


def wasi_libcxx_include_dir(
    sysroot: str | Path | None,
    *,
    target_triple: str | None = None,
    exceptions: bool = True,
) -> Path | None:
    """Select exactly the requested SDK C++ variant; never substitute its ABI."""
    if sysroot is None:
        return None
    target = target_triple or "wasm32-wasip1"
    if target != "wasm32-wasip1":
        raise ValueError(f"unsupported SDK C++ target: {target}")
    candidate = (
        Path(sysroot)
        / "include"
        / target
        / ("eh" if exceptions else "noeh")
        / "c++"
        / "v1"
    )
    return candidate if (candidate / "atomic").is_file() else None


def rust_target_libdir(
    target_triple: str,
    *,
    environment: Mapping[str, str] | None = None,
    root: Path | None = None,
) -> Path | None:
    selected = dict(os.environ if environment is None else environment)
    cwd = (compiler_source_root() if root is None else root).resolve()
    rustc = find_executable(
        executable_environment_value(selected, "RUSTC", "rustc"),
        cwd=cwd,
        environment=selected,
    )
    if rustc is None:
        return None
    rustc = resolve_rustup_proxy(rustc, role="rustc", root=cwd, env=selected)
    with stable_executable_probe(rustc, label="Rust target-library selection") as (
        entrypoint,
        identity,
    ):
        return _rust_target_libdir_cached(
            target_triple,
            str(entrypoint),
            identity.sha256,
            str(cwd),
            tuple(sorted(selected.items())),
        )


@functools.lru_cache(maxsize=8)
def _rust_target_libdir_cached(
    target_triple: str,
    rustc: str,
    generation: str,
    cwd: str,
    environment: tuple[tuple[str, str], ...],
) -> Path | None:
    del generation  # The selected compiler generation participates in cache identity.
    try:
        result = _run_completed_command(
            [rustc, "--print", "target-libdir", "--target", target_triple],
            capture_output=True,
            timeout=30,
            env=dict(environment),
            cwd=Path(cwd),
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    path_text = result.stdout.strip()
    if not path_text:
        return None
    if len(path_text.splitlines()) != 1 or not Path(path_text).is_absolute():
        raise ValueError("selected rustc target-libdir must be one absolute path")
    return Path(path_text).resolve(strict=False)


def clear_rust_target_libdir_cache() -> None:
    _rust_target_libdir_cached.cache_clear()


def wasm_wasi_libc_archive(
    target_triple: str = "wasm32-wasip1",
    *,
    environment: Mapping[str, str] | None = None,
) -> Path:
    if target_triple not in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        raise ValueError(f"unsupported SDK libc provider target: {target_triple}")
    return resolve_wasi_c_abi_plan(env=environment).path("libc")


def wasm_compiler_builtins_archive(
    target_triple: str = "wasm32-wasip1",
    *,
    target_libdir: Path | None = None,
    environment: Mapping[str, str] | None = None,
) -> Path | None:
    if target_libdir is None:
        target_libdir = rust_target_libdir(target_triple, environment=environment)
    if target_libdir is None:
        return None
    candidates = sorted(target_libdir.glob("libcompiler_builtins-*.rlib"))
    unversioned = target_libdir / "libcompiler_builtins.rlib"
    if unversioned.exists():
        candidates.append(unversioned)
    if len(candidates) > 1:
        raise ValueError(
            f"selected Rust target has ambiguous compiler-builtins archives: {target_libdir}"
        )
    return candidates[0] if candidates else None


def wasm_cxx_runtime_archives(
    target_triple: str = "wasm32-wasip1",
    *,
    exceptions: bool = True,
    environment: Mapping[str, str] | None = None,
    plan: WasiCAbiProjection | None = None,
) -> tuple[Path, ...]:
    if target_triple != "wasm32-wasip1":
        raise ValueError(f"unsupported SDK C++ target: {target_triple}")
    plan = plan or resolve_wasi_c_abi_plan(env=environment)
    library_root = (
        plan.sysroot / "lib" / target_triple / ("eh" if exceptions else "noeh")
    )
    archives = (library_root / "libc++.a", library_root / "libc++abi.a")
    if exceptions:
        archives += (library_root / "libunwind.a",)
    for path in archives:
        if not path.is_file() or not path.resolve(strict=True).is_relative_to(plan.sdk):
            raise ValueError(f"selected WASI SDK C++ variant is incomplete: {path}")
    return archives


def admit_wasi_provider_inputs(paths: Sequence[Path]) -> WasiCAbiProjection | None:
    """Bind recognizable C-runtime archives to the selected complete SDK.

    Rust compiler_builtins rlibs are deliberately outside this C ABI family.
    Source-extension receipts retain the admitted bytes independently.
    """
    selected = [
        path
        for path in paths
        if path.name
        in {
            "libc.a",
            "libc-printscan-long-double.a",
            "libclang_rt.builtins.a",
            "libclang_rt.builtins-wasm32.a",
            "libc++.a",
            "libc++abi.a",
            "libunwind.a",
        }
    ]
    if not selected:
        return None
    plan = resolve_wasi_c_abi_plan()
    permitted = {
        plan.path(role).resolve(strict=True)
        for role in ("libc", "long_double", "compiler_rt")
    }
    variants: set[str] = set()
    for path in selected:
        actual = path.resolve(strict=True)
        if actual in permitted:
            continue
        if path.name in {"libc++.a", "libc++abi.a", "libunwind.a"}:
            variant = path.parent.name
            expected = plan.sysroot / "lib" / "wasm32-wasip1" / variant / path.name
            if (
                variant in {"eh", "noeh"}
                and actual == expected.resolve(strict=True)
                and actual.is_relative_to(plan.sdk)
            ):
                variants.add(variant)
                continue
        raise ValueError(f"C-runtime input differs from selected WASI SDK: {path}")
    if len(variants) > 1:
        raise ValueError("WASI C++ input mixes exception variants")
    return plan


@dataclass(frozen=True)
class LongDoubleLinkPolicy:
    """One SDK's mandatory formatter and binary128 support, in link order."""

    printscan: Path
    builtins: Path


def resolve_long_double_link_policy(
    *, env: Mapping[str, str] | None = None
) -> LongDoubleLinkPolicy:
    plan = resolve_wasi_c_abi_plan(env=env)
    return LongDoubleLinkPolicy(plan.path("long_double"), plan.path("compiler_rt"))


def long_double_whole_archive_link_argv(
    policy: LongDoubleLinkPolicy,
    *,
    whole_archive: Sequence[str],
    trailing: Sequence[str],
) -> list[str]:
    """Raw wasm-ld dialect: real formatters precede lazy libc and builtins."""
    wa = [*map(str, whole_archive), str(policy.printscan)]
    tr = list(map(str, trailing))
    if str(policy.builtins) not in tr:
        tr.append(str(policy.builtins))
    return ["--whole-archive", *wa, "--no-whole-archive", *tr]
