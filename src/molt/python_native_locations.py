"""Path-only loaded native-image custody shared by location and content probes.

This module does not import the content scanner or parse native images.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from typing import Any, cast

from molt.python_identity_common import PythonEnvironmentIdentityError


def _native_contract_valid(contract: object, operating_system: str) -> bool:
    patterns = {
        "windows": r"windows-api-set:(?:api|ext)-ms-[a-z0-9-]+-l[0-9]+-[0-9]+-[0-9]+\.dll",
        "macos": r"macos-dyld-cache-image:[^/\\\x00:]+",
        "linux": r"linux-loader-image:linux-(?:vdso|gate)\.so\.1",
    }
    pattern = patterns.get(operating_system)
    return (
        isinstance(contract, str)
        and pattern is not None
        and re.fullmatch(pattern, contract) is not None
    )


def _loader_name(name: str, operating_system: str) -> str:
    return name.casefold() if operating_system == "windows" else name


def _loaded_native_module_paths(
    operating_system: str,
) -> tuple[tuple[Path, ...], dict[str, Path], tuple[str, ...]]:
    """Return the current process' loader-bound native image paths."""

    import ctypes

    paths: list[Path] = []
    if operating_system == "windows":
        from ctypes import wintypes

        get_process = ctypes.windll.kernel32.GetCurrentProcess  # type: ignore[attr-defined]
        get_process.argtypes = ()
        get_process.restype = wintypes.HANDLE
        process = get_process()
        enum_modules = ctypes.windll.psapi.EnumProcessModulesEx  # type: ignore[attr-defined]
        enum_modules.argtypes = (
            wintypes.HANDLE,
            ctypes.POINTER(wintypes.HMODULE),
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            wintypes.DWORD,
        )
        enum_modules.restype = wintypes.BOOL
        capacity = 256
        while True:
            modules = (wintypes.HMODULE * capacity)()
            needed = wintypes.DWORD()
            if not enum_modules(
                process, modules, ctypes.sizeof(modules), ctypes.byref(needed), 3
            ):
                raise PythonEnvironmentIdentityError(
                    "cannot enumerate loaded Windows runtime dependencies"
                )
            count = needed.value // ctypes.sizeof(wintypes.HMODULE)
            if count <= capacity:
                break
            capacity = count
        get_name = ctypes.windll.psapi.GetModuleFileNameExW  # type: ignore[attr-defined]
        get_name.argtypes = (
            wintypes.HANDLE,
            wintypes.HMODULE,
            wintypes.LPWSTR,
            wintypes.DWORD,
        )
        get_name.restype = wintypes.DWORD
        for module in modules[:count]:
            buffer = ctypes.create_unicode_buffer(32768)
            length = get_name(process, module, buffer, len(buffer))
            if not length or length >= len(buffer):
                raise PythonEnvironmentIdentityError(
                    "cannot identify an enumerated Windows native module"
                )
            paths.append(Path(buffer.value))
    elif operating_system == "macos":
        process = ctypes.CDLL(None)
        image_count = process._dyld_image_count
        image_count.argtypes = ()
        image_count.restype = ctypes.c_uint32
        image_name = process._dyld_get_image_name
        image_name.argtypes = (ctypes.c_uint32,)
        image_name.restype = ctypes.c_char_p
        for index in range(image_count()):
            raw = image_name(index)
            if not raw:
                raise PythonEnvironmentIdentityError(
                    "cannot identify an enumerated macOS native module"
                )
            paths.append(Path(os.fsdecode(raw)))
    elif operating_system == "linux":

        class _DlPhdrInfo(ctypes.Structure):
            _fields_ = [
                ("address", ctypes.c_void_p),
                ("name", ctypes.c_char_p),
                ("phdr", ctypes.c_void_p),
                ("phnum", ctypes.c_ushort),
            ]

        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.POINTER(_DlPhdrInfo), ctypes.c_size_t, ctypes.c_void_p
        )
        callback_errors: list[str] = []

        @callback_type
        def collect(info: object, _size: int, _data: object) -> int:
            # ctypes discards exceptions escaping callbacks. Preserve a typed
            # failure and stop iteration instead of publishing a partial list.
            try:
                if _size < ctypes.sizeof(_DlPhdrInfo):
                    raise ValueError("truncated dl_phdr_info")
                typed = cast(Any, info).contents
                if typed.name:
                    paths.append(Path(os.fsdecode(typed.name)))
            except (OSError, TypeError, ValueError) as exc:
                callback_errors.append(str(exc))
                return 1
            return 0

        iterator = ctypes.CDLL(None).dl_iterate_phdr
        iterator.argtypes = (callback_type, ctypes.c_void_p)
        iterator.restype = ctypes.c_int
        if iterator(collect, None) != 0 or callback_errors:
            raise PythonEnvironmentIdentityError(
                "cannot enumerate loaded Linux runtime dependencies"
                + (f": {callback_errors[0]}" if callback_errors else "")
            )
    else:  # guarded by _platform_identity
        raise PythonEnvironmentIdentityError(
            f"native dependency enumeration is unsupported on {operating_system}"
        )
    base = Path(getattr(sys, "_base_executable", None) or sys.executable)
    paths.append(base)
    canonical: dict[str, Path] = {}
    aliases: dict[str, Path] = {}
    contracts: set[str] = set()
    for raw in paths:
        try:
            resolved = raw.resolve(strict=True)
        except OSError as exc:
            if operating_system == "linux" and str(raw) in {
                "linux-vdso.so.1",
                "linux-gate.so.1",
            }:
                contracts.add(f"linux-loader-image:{raw.name}")
                continue
            if operating_system == "macos" and raw.is_absolute():
                contains = getattr(
                    ctypes.CDLL(None), "_dyld_shared_cache_contains_path", None
                )
                if contains is not None:
                    contains.argtypes = (ctypes.c_char_p,)
                    contains.restype = ctypes.c_bool
                    if contains(os.fsencode(raw)):
                        contracts.add(f"macos-dyld-cache-image:{raw.name}")
                        continue
            raise PythonEnvironmentIdentityError(
                f"loaded native runtime dependency has no file identity: {raw}"
            ) from exc
        key = os.path.normcase(str(resolved))
        canonical[key] = resolved
        alias = _loader_name(raw.name, operating_system)
        prior = aliases.get(alias)
        if prior is not None and not resolved.samefile(prior):
            raise PythonEnvironmentIdentityError(
                f"loaded native modules have an ambiguous loader name: {raw.name}"
            )
        aliases[alias] = resolved
    ordered = tuple(
        sorted(canonical.values(), key=lambda path: os.path.normcase(str(path)))
    )
    return ordered, aliases, tuple(sorted(contracts))


def loaded_native_module_paths(
    *, operating_system: str | None = None
) -> tuple[Path, ...]:
    """Prearm the current file census, not future loads or optional bindings.

    The content probe recaptures and fences its own census. A location receipt
    cannot authorize a file first loaded after this snapshot.
    """
    if operating_system is None:
        operating_system = {
            "win32": "windows",
            "darwin": "macos",
            "linux": "linux",
        }.get(sys.platform)
    if operating_system is None:
        raise PythonEnvironmentIdentityError(
            f"native dependency enumeration is unsupported on {sys.platform}"
        )
    paths, _aliases, _contracts = _loaded_native_module_paths(operating_system)
    return paths
