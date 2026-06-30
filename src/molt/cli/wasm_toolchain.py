from __future__ import annotations

import functools
import os
import shutil
from pathlib import Path

from molt.cli.command_runtime import _run_completed_command


_WASI_ERRNO_RELATIVE_PATHS = (
    Path("include") / "errno.h",
    Path("include") / "wasm32-wasip1" / "errno.h",
    Path("include") / "wasm32-wasi" / "errno.h",
)


def normalize_wasi_sysroot(path: str | Path | None) -> Path | None:
    if path is None:
        return None
    candidate = Path(path).expanduser()
    roots = [candidate]
    if candidate.name == "include":
        roots.append(candidate.parent)
    if candidate.parent.name == "include" and candidate.name.startswith("wasm32-"):
        roots.append(candidate.parent.parent)
    for root in roots:
        if any((root / relative).exists() for relative in _WASI_ERRNO_RELATIVE_PATHS):
            return root.resolve(strict=False)
    return None


def _wasi_sdk_sysroot_candidates(raw: str | None) -> list[Path]:
    if not raw:
        return []
    sdk_root = Path(raw).expanduser()
    return [
        sdk_root,
        sdk_root / "share" / "wasi-sysroot",
        sdk_root / "wasi-sysroot",
    ]


@functools.lru_cache(maxsize=64)
def _resolve_wasi_sysroot_cached(
    molt_wasi_sysroot: str | None,
    wasi_sysroot: str | None,
    wasi_sdk_path: str | None,
    molt_target_root: str | None,
) -> Path | None:
    candidates: list[Path] = []
    for raw in (molt_wasi_sysroot, wasi_sysroot):
        if raw:
            candidates.append(Path(raw).expanduser())
    candidates.extend(_wasi_sdk_sysroot_candidates(wasi_sdk_path))
    if molt_target_root:
        target_root = Path(molt_target_root).expanduser()
        target_toolchains = target_root / "toolchains"
        candidates.extend(
            [
                target_root / "toolchains" / "wasi-sysroot",
                target_root / "toolchains" / "wasi-sdk" / "share" / "wasi-sysroot",
                target_root / "toolchains" / "wasi-sdk" / "wasi-sysroot",
                target_root / "wasi-sysroot",
                target_root / "wasi-sdk" / "share" / "wasi-sysroot",
                target_root / "wasi-sdk" / "wasi-sysroot",
            ]
        )
        if target_toolchains.exists():
            candidates.extend(sorted(target_toolchains.glob("wasi-sysroot-*")))
    if os.name == "nt":
        program_files = os.environ.get("ProgramFiles")
        local_app_data = os.environ.get("LOCALAPPDATA")
        for root in (program_files, local_app_data):
            if root:
                candidates.extend(
                    _wasi_sdk_sysroot_candidates(str(Path(root) / "wasi-sdk"))
                )
    else:
        candidates.extend(
            [
                Path("/opt/homebrew/opt/wasi-libc/share/wasi-sysroot"),
                Path("/usr/local/opt/wasi-libc/share/wasi-sysroot"),
                Path("/opt/wasi-sdk/share/wasi-sysroot"),
                Path("/opt/wasi-sdk/wasi-sysroot"),
                Path("/usr/share/wasi-sysroot"),
                Path("/usr/local/share/wasi-sysroot"),
            ]
        )
    seen: set[Path] = set()
    for candidate in candidates:
        normalized = candidate.resolve(strict=False)
        if normalized in seen:
            continue
        seen.add(normalized)
        resolved = normalize_wasi_sysroot(normalized)
        if resolved is not None:
            return resolved
    return None


def resolve_wasi_sysroot() -> Path | None:
    return _resolve_wasi_sysroot_cached(
        os.environ.get("MOLT_WASI_SYSROOT"),
        os.environ.get("WASI_SYSROOT"),
        os.environ.get("WASI_SDK_PATH"),
        os.environ.get("MOLT_TARGET_ROOT"),
    )


@functools.lru_cache(maxsize=8)
def rust_target_libdir(target_triple: str) -> Path | None:
    rustc = shutil.which("rustc")
    if rustc is None:
        return None
    try:
        result = _run_completed_command(
            [rustc, "--print", "target-libdir", "--target", target_triple],
            capture_output=True,
            timeout=30,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    path_text = result.stdout.strip()
    if not path_text:
        return None
    return Path(path_text)


def wasm_wasi_libc_archive(target_triple: str = "wasm32-wasip1") -> Path | None:
    target_libdir = rust_target_libdir(target_triple)
    if target_libdir is None:
        return None
    libc_archive = target_libdir / "self-contained" / "libc.a"
    if not libc_archive.exists():
        return None
    return libc_archive


def wasm_compiler_builtins_archive(target_triple: str = "wasm32-wasip1") -> Path | None:
    target_libdir = rust_target_libdir(target_triple)
    if target_libdir is None:
        return None
    candidates = sorted(target_libdir.glob("libcompiler_builtins-*.rlib"))
    if candidates:
        return candidates[0]
    unversioned = target_libdir / "libcompiler_builtins.rlib"
    if unversioned.exists():
        return unversioned
    return None
