"""Read-only disk-capacity admission for build outputs and run scratch."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from functools import cache
import json
import math
import os
from pathlib import Path
from molt.custody_layout import StorageClass, storage_rule
import re
import shutil
import stat
import sys
from typing import TYPE_CHECKING, Literal

if TYPE_CHECKING:
    import ctypes


_GIB = 1024**3
_MAX_SUPPORTED_BYTES = (1 << 63) - 1
_MAX_CONFIG_TEXT_LENGTH = 256
_POSITIVE_DECIMAL = re.compile(
    r"\+?(?:(?P<integer>[0-9]+)(?:\.(?P<fraction>[0-9]*))?"
    r"|\.(?P<leading_fraction>[0-9]+))"
    r"(?:[eE](?P<exponent>[+-]?[0-9]+))?\Z"
)

DEFAULT_MINIMUM_HEADROOM_BYTES = 25 * _GIB
DISK_GUARD_HIGH_WATER_ENV = "MOLT_DISK_GUARD_HIGH_WATER_GB"
# A guarded run starts only when its scratch volume has this much free space.
DEFAULT_SCRATCH_BUDGET_BYTES = 4 * _GIB
SCRATCH_BUDGET_ENV = "MOLT_SCRATCH_BUDGET_GB"
DISK_CAPACITY_DIAGNOSTIC_SCHEMA = "molt.disk-capacity.v2"

FreeSpaceMeasurement = Callable[[Path], int]
CapacityMeasurement = Literal["filesystem", "supplied", "darwin-important-usage"]


@dataclass(frozen=True, slots=True)
class DiskCapacityProbe:
    """One output path and the existing filesystem location measured for it."""

    requested_path: Path
    measured_path: Path | None
    free_bytes: int | None
    required_bytes: int
    error: str | None = None
    important_usage_bytes: int | None = None
    measurement: CapacityMeasurement = "filesystem"

    @property
    def available_bytes(self) -> int | None:
        return (
            self.important_usage_bytes
            if self.measurement == "darwin-important-usage"
            else self.free_bytes
        )

    @property
    def ready(self) -> bool:
        return (
            self.error is None
            and self.available_bytes is not None
            and self.available_bytes >= self.required_bytes
        )

    def as_dict(self) -> dict[str, object]:
        return {
            "requested_path": str(self.requested_path),
            "measured_path": (
                None if self.measured_path is None else str(self.measured_path)
            ),
            "free_bytes": self.free_bytes,
            "important_usage_bytes": self.important_usage_bytes,
            "available_bytes": self.available_bytes,
            "measurement": self.measurement,
            "required_bytes": self.required_bytes,
            "error": self.error,
        }


@dataclass(frozen=True, slots=True)
class DiskCapacityReceipt:
    """Successful admission receipt for all requested build output paths."""

    required_bytes: int
    probes: tuple[DiskCapacityProbe, ...]

    def as_dict(self) -> dict[str, object]:
        return {
            "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
            "status": "admitted",
            "required_bytes": self.required_bytes,
            "probes": [probe.as_dict() for probe in self.probes],
        }


class DiskCapacityError(ValueError):
    """Capacity admission rejection with a machine-readable diagnostic."""

    def __init__(self, message: str, diagnostic: Mapping[str, object]) -> None:
        self.diagnostic = dict(diagnostic)
        serialized = json.dumps(
            self.diagnostic,
            sort_keys=True,
            separators=(",", ":"),
        )
        super().__init__(f"{message} diagnostic={serialized}")


def _setting_error(
    name: str, default_bytes: int, raw_value: object, reason: str
) -> DiskCapacityError:
    diagnostic: dict[str, object] = {
        "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
        "status": "invalid-configuration",
        "environment_variable": name,
        "configured_value": raw_value,
        "error": reason,
    }
    return DiskCapacityError(
        f"invalid {name} value {raw_value!r}: {reason}. "
        f"Set it to a positive finite number of GiB, or unset it to use the "
        f"{default_bytes / _GIB:g} GiB default.",
        diagnostic,
    )


def capacity_bytes_from_gib(value: float) -> int:
    """Convert a caller's non-negative GiB budget to bounded whole bytes."""
    byte_value = value * _GIB
    if (
        not math.isfinite(byte_value)
        or byte_value < 0
        or byte_value > _MAX_SUPPORTED_BYTES
    ):
        raise ValueError(
            f"capacity must be finite and non-negative, at most {_MAX_SUPPORTED_BYTES} bytes"
        )
    return math.ceil(byte_value)


def minimum_headroom_bytes(env: Mapping[str, str] | None = None) -> int:
    """Return the canonical minimum free-space threshold for builds in bytes."""

    return _gib_setting_bytes(
        env, DISK_GUARD_HIGH_WATER_ENV, DEFAULT_MINIMUM_HEADROOM_BYTES
    )


def scratch_budget_bytes(env: Mapping[str, str] | None = None) -> int:
    """Return the free space a guarded run's scratch volume needs, in bytes."""

    return _gib_setting_bytes(env, SCRATCH_BUDGET_ENV, DEFAULT_SCRATCH_BUDGET_BYTES)


def _gib_setting_bytes(
    env: Mapping[str, str] | None, name: str, default_bytes: int
) -> int:
    """Parse one GiB setting into bytes.

    The parser is deliberately strict and rounds fractional byte values
    upward. Invalid configuration rejects admission rather than silently
    falling back to the default.
    """

    environment = os.environ if env is None else env
    if name not in environment:
        return default_bytes

    raw_value = environment[name]
    if not isinstance(raw_value, str):
        raise _setting_error(
            name, default_bytes, raw_value, "expected a string containing GiB"
        )
    text = raw_value.strip()
    if not text or len(text) > _MAX_CONFIG_TEXT_LENGTH:
        raise _setting_error(
            name,
            default_bytes,
            raw_value,
            "expected a positive finite decimal number of GiB",
        )
    match = _POSITIVE_DECIMAL.fullmatch(text)
    if match is None:
        raise _setting_error(
            name,
            default_bytes,
            raw_value,
            "expected a positive finite decimal number of GiB",
        )

    integer = match.group("integer")
    if integer is None:
        fractional = match.group("leading_fraction") or ""
        digits = fractional
    else:
        fractional = match.group("fraction") or ""
        digits = integer + fractional
    significant_digits = digits.lstrip("0")
    if not significant_digits:
        raise _setting_error(
            name,
            default_bytes,
            raw_value,
            "expected a positive finite decimal number of GiB",
        )
    coefficient = int(significant_digits)
    exponent = int(match.group("exponent") or "0")
    decimal_scale = len(fractional) - exponent
    numerator = coefficient * _GIB

    if decimal_scale >= len(str(numerator)):
        return 1
    if decimal_scale >= 0:
        denominator = 10**decimal_scale
        quotient, remainder = divmod(numerator, denominator)
        required_bytes = quotient + bool(remainder)
    else:
        decimal_shift = -decimal_scale
        maximum_digits = len(str(_MAX_SUPPORTED_BYTES))
        if len(str(numerator)) + decimal_shift > maximum_digits:
            raise _setting_error(
                name,
                default_bytes,
                raw_value,
                f"value exceeds the supported {_MAX_SUPPORTED_BYTES}-byte range",
            )
        required_bytes = numerator * 10**decimal_shift

    if required_bytes < 1 or required_bytes > _MAX_SUPPORTED_BYTES:
        raise _setting_error(
            name,
            default_bytes,
            raw_value,
            f"value exceeds the supported {_MAX_SUPPORTED_BYTES}-byte range",
        )
    return required_bytes


def _path_sort_key(path: Path) -> str:
    return os.path.normcase(os.fspath(path))


def _nearest_existing_directory(path: Path) -> Path:
    candidate = path
    while True:
        try:
            metadata = candidate.stat()
        except (FileNotFoundError, NotADirectoryError):
            parent = candidate.parent
            if parent == candidate:
                raise FileNotFoundError(
                    f"no existing filesystem ancestor for {path}"
                ) from None
            candidate = parent
            continue
        if stat.S_ISDIR(metadata.st_mode):
            return candidate
        parent = candidate.parent
        if parent == candidate:
            raise NotADirectoryError(f"no existing directory ancestor for {path}")
        candidate = parent


def _default_measure_free_bytes(path: Path) -> int:
    return shutil.disk_usage(path).free


@cache
def _darwin_capacity_api() -> tuple[ctypes.CDLL, ctypes.c_void_p]:
    """Retain only immutable ABI bindings, never an observed capacity."""
    import ctypes

    pointer = ctypes.c_void_p
    cf = ctypes.CDLL(
        "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"
    )
    create = cf.CFURLCreateFromFileSystemRepresentation
    create.argtypes = [pointer, ctypes.c_char_p, ctypes.c_long, ctypes.c_ubyte]
    create.restype = pointer
    copy = cf.CFURLCopyResourcePropertyForKey
    copy.argtypes = [pointer, pointer, ctypes.POINTER(pointer), ctypes.POINTER(pointer)]
    copy.restype = ctypes.c_ubyte
    get_type = cf.CFGetTypeID
    get_type.argtypes = [pointer]
    get_type.restype = ctypes.c_ulong
    number_type = cf.CFNumberGetTypeID
    number_type.argtypes = []
    number_type.restype = ctypes.c_ulong
    number_value = cf.CFNumberGetValue
    number_value.argtypes = [pointer, ctypes.c_long, pointer]
    number_value.restype = ctypes.c_ubyte
    error_code = cf.CFErrorGetCode
    error_code.argtypes = [pointer]
    error_code.restype = ctypes.c_long
    release = cf.CFRelease
    release.argtypes = [pointer]
    release.restype = None
    key = pointer.in_dll(cf, "kCFURLVolumeAvailableCapacityForImportantUsageKey")
    if not key.value:
        raise OSError("Darwin important-usage capacity key is unavailable")
    return cf, key


def _darwin_important_usage_bytes(path: Path) -> int:
    """Read Apple's allocation estimate; never purge or fall back after failure.

    CFURL.h defines this as capacity for explicitly requested, replaceable
    resources, including space the OS expects to purge. A fresh CFURL avoids
    retaining resource-value answers across admission calls.
    """
    import ctypes

    cf, key = _darwin_capacity_api()
    encoded = os.fsencode(path)
    url = cf.CFURLCreateFromFileSystemRepresentation(None, encoded, len(encoded), 1)
    if not url:
        raise OSError("Darwin capacity could not create the volume URL")
    value, error = ctypes.c_void_p(), ctypes.c_void_p()
    try:
        if not cf.CFURLCopyResourcePropertyForKey(
            url, key, ctypes.byref(value), ctypes.byref(error)
        ):
            code = cf.CFErrorGetCode(error) if error.value else None
            raise OSError(
                f"Darwin important-usage capacity query failed: CFError {code}"
            )
        if not value.value or cf.CFGetTypeID(value) != cf.CFNumberGetTypeID():
            raise ValueError("Darwin important-usage capacity is not a CFNumber")
        result = ctypes.c_int64()
        # kCFNumberSInt64Type is 4; GetValue must report an exact conversion.
        if not cf.CFNumberGetValue(value, 4, ctypes.byref(result)):
            raise ValueError("Darwin important-usage capacity is not an exact int64")
        if result.value < 0:
            raise ValueError("Darwin important-usage capacity is negative")
        return result.value
    finally:
        if value.value:
            cf.CFRelease(value)
        if error.value:
            cf.CFRelease(error)
        cf.CFRelease(url)


def _probe_failure(
    requested_path: Path,
    required_bytes: int,
    error: BaseException,
    *,
    measured_path: Path | None = None,
    free_bytes: int | None = None,
    measurement: CapacityMeasurement = "filesystem",
) -> DiskCapacityProbe:
    detail = f"{type(error).__name__}: {error}"
    return DiskCapacityProbe(
        requested_path=requested_path,
        measured_path=measured_path,
        free_bytes=free_bytes,
        required_bytes=required_bytes,
        error=detail,
        measurement=measurement,
    )


def _rejection_message(
    purpose: str, remedy: str, probes: Iterable[DiskCapacityProbe]
) -> str:
    details: list[str] = []
    for probe in probes:
        if probe.error is not None:
            details.append(f"cannot measure {probe.requested_path}: {probe.error}")
        elif (
            probe.available_bytes is not None
            and probe.available_bytes < probe.required_bytes
        ):
            details.append(
                f"{probe.requested_path} has {probe.available_bytes} bytes available "
                f"by {probe.measurement} ({probe.free_bytes} bytes raw free) on "
                f"{probe.measured_path}, below the required "
                f"{probe.required_bytes} bytes"
            )
    joined = "; ".join(details)
    return f"{purpose} capacity admission rejected: {joined}. {remedy}"


_BUILD_REMEDY = (
    "Reclaim verified inactive artifacts or select an explicitly permitted "
    "build output root with enough free capacity before retrying."
)
_SCRATCH_REMEDY = (
    "Free space on that volume, select other scratch storage with "
    "MOLT_SCRATCH_STORAGE, or set MOLT_SCRATCH_BUDGET_GB to the scratch this "
    "run needs."
)


def require_build_capacity(
    paths: Iterable[Path],
    *,
    env: Mapping[str, str] | None = None,
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityReceipt:
    """Require sufficient free capacity for every requested build output."""

    return _require_capacity(
        paths,
        required_bytes=minimum_headroom_bytes(env),
        purpose="build",
        remedy=_BUILD_REMEDY,
        measure_free_bytes=measure_free_bytes,
    )


def require_scratch_capacity(
    paths: Iterable[Path],
    *,
    env: Mapping[str, str] | None = None,
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityReceipt:
    """Require the scratch budget free on every scratch volume before a run.

    Insufficient observed capacity rejects the run before it starts. Admission
    reserves no space; concurrent allocations and estimate changes can still
    cause ENOSPC during the run.
    """

    return _require_capacity(
        paths,
        required_bytes=scratch_budget_bytes(env),
        purpose="scratch",
        remedy=_SCRATCH_REMEDY,
        measure_free_bytes=measure_free_bytes,
    )


def require_storage_capacity(
    paths: Iterable[Path],
    *,
    kind: StorageClass,
    env: Mapping[str, str] | None = None,
    additional_bytes: int = 0,
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityReceipt:
    """Admit a typed state writer on its actual destination volume.

    Reuse the existing capacity floor; evidence compaction additionally needs
    space for the complete source before it may release any source bytes.
    This is an observation, not a reservation against concurrent allocators.
    """
    storage_rule(kind)  # Closed identity admission before any output mutation.
    if additional_bytes < 0:
        raise ValueError("storage admission requires nonnegative payload bytes")
    return _require_capacity(
        paths,
        required_bytes=minimum_headroom_bytes(env) + additional_bytes,
        purpose=f"storage:{kind}",
        remedy=_BUILD_REMEDY,
        measure_free_bytes=measure_free_bytes,
    )


def probe_capacity(
    path: Path,
    *,
    required_bytes: int,
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityProbe:
    """Observe capacity for one caller-owned budget without creating any paths.

    Raw free capacity proves admission cheaply. Only insufficient Darwin raw
    capacity needs the OS estimate for important, replaceable resources. A
    failed estimate never falls back to a different measurement. An explicit
    supplied measurement owns the complete answer (including in tests).
    """
    if (
        not isinstance(required_bytes, int)
        or isinstance(required_bytes, bool)
        or not 0 <= required_bytes <= _MAX_SUPPORTED_BYTES
    ):
        raise ValueError("required capacity must be a non-negative int64")
    raw_path = Path(path)
    try:
        resolved = raw_path.expanduser().resolve(strict=False)
    except (OSError, RuntimeError, ValueError) as exc:
        return _probe_failure(raw_path, required_bytes, exc)
    return _measure_capacity(resolved, required_bytes, measure_free_bytes)


def _measure_capacity(
    resolved: Path,
    required_bytes: int,
    measure_free_bytes: FreeSpaceMeasurement | None,
) -> DiskCapacityProbe:
    try:
        measured_path = _nearest_existing_directory(resolved)
    except (OSError, ValueError) as exc:
        return _probe_failure(resolved, required_bytes, exc)
    measurement: CapacityMeasurement = (
        "filesystem" if measure_free_bytes is None else "supplied"
    )
    measure = (
        _default_measure_free_bytes
        if measure_free_bytes is None
        else measure_free_bytes
    )
    try:
        free_bytes = measure(measured_path)
        if (
            not isinstance(free_bytes, int)
            or isinstance(free_bytes, bool)
            or free_bytes < 0
        ):
            raise ValueError("free-space measurement must be a non-negative integer")
    except Exception as exc:
        return _probe_failure(
            resolved,
            required_bytes,
            exc,
            measured_path=measured_path,
            measurement=measurement,
        )
    important_usage_bytes = None
    if (
        measure_free_bytes is None
        and sys.platform == "darwin"
        and free_bytes < required_bytes
    ):
        measurement = "darwin-important-usage"
        try:
            important_usage_bytes = _darwin_important_usage_bytes(measured_path)
            if (
                not isinstance(important_usage_bytes, int)
                or isinstance(important_usage_bytes, bool)
                or not 0 <= important_usage_bytes <= _MAX_SUPPORTED_BYTES
            ):
                raise ValueError(
                    "Darwin important-usage capacity must be a non-negative int64"
                )
        except Exception as exc:
            return _probe_failure(
                resolved,
                required_bytes,
                exc,
                measured_path=measured_path,
                free_bytes=free_bytes,
                measurement=measurement,
            )
    return DiskCapacityProbe(
        requested_path=resolved,
        measured_path=measured_path,
        free_bytes=free_bytes,
        required_bytes=required_bytes,
        important_usage_bytes=important_usage_bytes,
        measurement=measurement,
    )


def _require_capacity(
    paths: Iterable[Path],
    *,
    required_bytes: int,
    purpose: str,
    remedy: str,
    measure_free_bytes: FreeSpaceMeasurement | None,
) -> DiskCapacityReceipt:
    """Require ``required_bytes`` free on the filesystem of every path.

    This function only resolves paths, reads filesystem metadata, and samples
    free space. It never creates directories, reclaims storage, invokes
    another process, or honors a test/runtime bypass.
    """

    resolved_paths: dict[str, Path] = {}
    resolution_failures: list[DiskCapacityProbe] = []
    for requested in paths:
        raw_path = Path(requested)
        try:
            resolved = raw_path.expanduser().resolve(strict=False)
        except (OSError, RuntimeError, ValueError) as exc:
            resolution_failures.append(_probe_failure(raw_path, required_bytes, exc))
            continue
        resolved_paths.setdefault(_path_sort_key(resolved), resolved)

    if not resolved_paths and not resolution_failures:
        diagnostic: dict[str, object] = {
            "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
            "status": "rejected",
            "required_bytes": required_bytes,
            "probes": [],
            "error": f"no {purpose} paths were supplied",
        }
        raise DiskCapacityError(
            f"{purpose} capacity admission requires at least one path.",
            diagnostic,
        )

    probes = sorted(
        resolution_failures,
        key=lambda probe: _path_sort_key(probe.requested_path),
    )
    probes.extend(
        _measure_capacity(resolved, required_bytes, measure_free_bytes)
        for resolved in sorted(resolved_paths.values(), key=_path_sort_key)
    )

    probes.sort(key=lambda probe: _path_sort_key(probe.requested_path))
    rejected = tuple(probe for probe in probes if not probe.ready)
    if rejected:
        diagnostic = {
            "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
            "status": "rejected",
            "required_bytes": required_bytes,
            "probes": [probe.as_dict() for probe in probes],
        }
        raise DiskCapacityError(
            _rejection_message(purpose, remedy, rejected), diagnostic
        )

    return DiskCapacityReceipt(required_bytes=required_bytes, probes=tuple(probes))
