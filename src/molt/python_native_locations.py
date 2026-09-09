"""Loaded native-image custody shared by location and content probes.

This module does not import the content scanner. On macOS its single loader
snapshot reads the selected thin image headers needed to bind universal files
to the exact slices already chosen by dyld.
"""

from __future__ import annotations

import os
import re
import struct
import sys
import unicodedata
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, cast

from molt.python_identity_common import PythonEnvironmentIdentityError


_MACOS_DYLD_CACHE_CONTRACT_PREFIX = "macos-dyld-cache-image:"


@dataclass(frozen=True, slots=True)
class LoadedNativeModuleSnapshot:
    """One loader census shared by path, contract, and content custody."""

    paths: tuple[Path, ...]
    aliases: Mapping[str, Path]
    contracts: tuple[str, ...]
    macho_identities: Mapping[Path, tuple[int, int]]

    def __post_init__(self) -> None:
        object.__setattr__(self, "aliases", MappingProxyType(dict(self.aliases)))
        object.__setattr__(
            self,
            "macho_identities",
            MappingProxyType(dict(self.macho_identities)),
        )


def _macos_dyld_cache_contract(path: str) -> str | None:
    normalized = unicodedata.normalize("NFC", path)
    components = normalized.split("/")
    if (
        not normalized.startswith("/")
        or normalized.startswith("//")
        or len(components) < 2
        or any(component in {"", ".", ".."} for component in components[1:])
        or any(character in normalized for character in ("\\", "\0", ":"))
    ):
        return None
    return _MACOS_DYLD_CACHE_CONTRACT_PREFIX + normalized


def _native_contract_valid(contract: object, operating_system: str) -> bool:
    if operating_system == "macos":
        return (
            isinstance(contract, str)
            and contract.startswith(_MACOS_DYLD_CACHE_CONTRACT_PREFIX)
            and _macos_dyld_cache_contract(
                contract.removeprefix(_MACOS_DYLD_CACHE_CONTRACT_PREFIX)
            )
            == contract
        )
    patterns = {
        "windows": r"windows-api-set:(?:api|ext)-ms-[a-z0-9-]+-l[0-9]+-[0-9]+-[0-9]+\.dll",
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


def _loaded_native_module_snapshot(
    operating_system: str,
) -> LoadedNativeModuleSnapshot:
    """Capture one loader-bound image snapshot for all native custody fields."""

    import ctypes

    paths: list[Path] = []
    raw_macho_identities: dict[str, tuple[int, int]] = {}
    macos_shared_cache_contains: Any | None = None
    if operating_system == "windows":
        if os.name != "nt":
            raise PythonEnvironmentIdentityError(
                "cannot enumerate Windows runtime dependencies on a non-Windows host"
            )
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        get_process = kernel32.GetCurrentProcess
        get_process.argtypes = ()
        get_process.restype = wintypes.HANDLE
        process = get_process()
        enum_modules = psapi.EnumProcessModulesEx
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
        get_name = psapi.GetModuleFileNameExW
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
        if sys.platform != "darwin":
            raise PythonEnvironmentIdentityError(
                "cannot enumerate macOS runtime dependencies on a non-macOS host"
            )
        process = ctypes.CDLL(None)
        image_count = process._dyld_image_count
        image_count.argtypes = ()
        image_count.restype = ctypes.c_uint32
        image_name = process._dyld_get_image_name
        image_name.argtypes = (ctypes.c_uint32,)
        image_name.restype = ctypes.c_char_p
        image_header = process._dyld_get_image_header
        image_header.argtypes = (ctypes.c_uint32,)
        image_header.restype = ctypes.c_void_p
        macos_shared_cache_contains = getattr(
            process, "_dyld_shared_cache_contains_path", None
        )
        if macos_shared_cache_contains is not None:
            macos_shared_cache_contains.argtypes = (ctypes.c_char_p,)
            macos_shared_cache_contains.restype = ctypes.c_bool
        magics = {
            b"\xce\xfa\xed\xfe": "<",
            b"\xcf\xfa\xed\xfe": "<",
            b"\xfe\xed\xfa\xce": ">",
            b"\xfe\xed\xfa\xcf": ">",
        }
        for index in range(image_count()):
            raw_name = image_name(index)
            address = image_header(index)
            if not raw_name or not address:
                raise PythonEnvironmentIdentityError(
                    "cannot identify an enumerated macOS native module header"
                )
            raw = Path(os.fsdecode(raw_name))
            fixed = ctypes.string_at(address, 12)
            try:
                endian = magics[fixed[:4]]
            except KeyError as exc:
                raise PythonEnvironmentIdentityError(
                    f"loaded macOS image has invalid thin Mach-O magic: {raw}"
                ) from exc
            identity = struct.unpack_from(endian + "II", fixed, 4)
            key = str(raw)
            prior = raw_macho_identities.get(key)
            if prior is not None and prior != identity:
                raise PythonEnvironmentIdentityError(
                    f"loaded macOS image has conflicting dyld identities: {raw}"
                )
            raw_macho_identities[key] = identity
            paths.append(raw)
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
    macho_identities: dict[Path, tuple[int, int]] = {}
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
                if macos_shared_cache_contains is not None:
                    if macos_shared_cache_contains(os.fsencode(raw)):
                        contract = _macos_dyld_cache_contract(raw.as_posix())
                        if contract is None:
                            raise PythonEnvironmentIdentityError(
                                "loaded macOS dyld-cache image path is not canonical: "
                                f"{raw}"
                            )
                        contracts.add(contract)
                        continue
            raise PythonEnvironmentIdentityError(
                f"loaded native runtime dependency has no file identity: {raw}"
            ) from exc
        if operating_system == "macos":
            identity = raw_macho_identities.get(str(raw))
            prior_identity = macho_identities.get(resolved)
            if identity is None:
                if prior_identity is None:
                    raise PythonEnvironmentIdentityError(
                        f"loaded macOS file-backed image has no dyld identity: {raw}"
                    )
            elif prior_identity is not None and prior_identity != identity:
                raise PythonEnvironmentIdentityError(
                    f"loaded macOS image has conflicting dyld identities: {resolved}"
                )
            else:
                macho_identities[resolved] = identity
        key = os.path.normcase(str(resolved))
        canonical[key] = resolved
        # dyld identifies loaded images by their install/resolved path, and can
        # legitimately load distinct framework images with the same basename.
        # PE and ELF dependency tables bind by case-folded/import basename.
        alias = (
            os.path.normcase(str(resolved))
            if operating_system == "macos"
            else _loader_name(raw.name, operating_system)
        )
        prior = aliases.get(alias)
        if prior is not None and not resolved.samefile(prior):
            raise PythonEnvironmentIdentityError(
                f"loaded native modules have an ambiguous loader name: {raw.name}"
            )
        aliases[alias] = resolved
    ordered = tuple(
        sorted(canonical.values(), key=lambda path: os.path.normcase(str(path)))
    )
    return LoadedNativeModuleSnapshot(
        paths=ordered,
        aliases=aliases,
        contracts=tuple(sorted(contracts)),
        macho_identities=macho_identities,
    )


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
    return _loaded_native_module_snapshot(operating_system).paths
