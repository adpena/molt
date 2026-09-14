"""Read-only disk-capacity admission for build output paths."""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shutil
import stat


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
DISK_CAPACITY_DIAGNOSTIC_SCHEMA = "molt.disk-capacity.v1"

FreeSpaceMeasurement = Callable[[Path], int]


@dataclass(frozen=True, slots=True)
class DiskCapacityProbe:
    """One output path and the existing filesystem location measured for it."""

    requested_path: Path
    measured_path: Path | None
    free_bytes: int | None
    required_bytes: int
    error: str | None = None

    def as_dict(self) -> dict[str, object]:
        return {
            "requested_path": str(self.requested_path),
            "measured_path": (
                None if self.measured_path is None else str(self.measured_path)
            ),
            "free_bytes": self.free_bytes,
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


def _configuration_error(raw_value: object, reason: str) -> DiskCapacityError:
    diagnostic: dict[str, object] = {
        "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
        "status": "invalid-configuration",
        "environment_variable": DISK_GUARD_HIGH_WATER_ENV,
        "configured_value": raw_value,
        "error": reason,
    }
    return DiskCapacityError(
        f"invalid {DISK_GUARD_HIGH_WATER_ENV} value {raw_value!r}: {reason}. "
        f"Set it to a positive finite number of GiB, or unset it to use the "
        f"{DEFAULT_MINIMUM_HEADROOM_BYTES // _GIB} GiB default.",
        diagnostic,
    )


def minimum_headroom_bytes(env: Mapping[str, str] | None = None) -> int:
    """Return the canonical minimum free-space threshold in bytes.

    The environment override is deliberately strict and rounds fractional byte
    values upward. Invalid configuration rejects admission rather than silently
    falling back to the default.
    """

    environment = os.environ if env is None else env
    if DISK_GUARD_HIGH_WATER_ENV not in environment:
        return DEFAULT_MINIMUM_HEADROOM_BYTES

    raw_value = environment[DISK_GUARD_HIGH_WATER_ENV]
    if not isinstance(raw_value, str):
        raise _configuration_error(raw_value, "expected a string containing GiB")
    text = raw_value.strip()
    if not text or len(text) > _MAX_CONFIG_TEXT_LENGTH:
        raise _configuration_error(
            raw_value,
            "expected a positive finite decimal number of GiB",
        )
    match = _POSITIVE_DECIMAL.fullmatch(text)
    if match is None:
        raise _configuration_error(
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
        raise _configuration_error(
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
            raise _configuration_error(
                raw_value,
                f"value exceeds the supported {_MAX_SUPPORTED_BYTES}-byte range",
            )
        required_bytes = numerator * 10**decimal_shift

    if required_bytes < 1 or required_bytes > _MAX_SUPPORTED_BYTES:
        raise _configuration_error(
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


def _probe_failure(
    requested_path: Path,
    required_bytes: int,
    error: BaseException,
    *,
    measured_path: Path | None = None,
) -> DiskCapacityProbe:
    detail = f"{type(error).__name__}: {error}"
    return DiskCapacityProbe(
        requested_path=requested_path,
        measured_path=measured_path,
        free_bytes=None,
        required_bytes=required_bytes,
        error=detail,
    )


def _rejection_message(probes: Iterable[DiskCapacityProbe]) -> str:
    details: list[str] = []
    for probe in probes:
        if probe.error is not None:
            details.append(f"cannot measure {probe.requested_path}: {probe.error}")
        elif probe.free_bytes is not None and probe.free_bytes < probe.required_bytes:
            details.append(
                f"{probe.requested_path} has {probe.free_bytes} bytes free on "
                f"{probe.measured_path}, below the required "
                f"{probe.required_bytes} bytes"
            )
    joined = "; ".join(details)
    return (
        f"build capacity admission rejected: {joined}. Reclaim verified inactive "
        "artifacts or select an explicitly permitted build output root with "
        "enough free capacity before retrying."
    )


def require_build_capacity(
    paths: Iterable[Path],
    *,
    env: Mapping[str, str] | None = None,
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityReceipt:
    """Require sufficient free capacity for every requested build output.

    This function only resolves paths, reads filesystem metadata, and samples
    free space. It never creates output directories, reclaims storage, invokes
    another process, or honors a test/runtime bypass.
    """

    required_bytes = minimum_headroom_bytes(env)
    measure = (
        _default_measure_free_bytes
        if measure_free_bytes is None
        else measure_free_bytes
    )

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
            "error": "no build output paths were supplied",
        }
        raise DiskCapacityError(
            "build capacity admission requires at least one output path.",
            diagnostic,
        )

    probes = sorted(
        resolution_failures,
        key=lambda probe: _path_sort_key(probe.requested_path),
    )
    for resolved in sorted(resolved_paths.values(), key=_path_sort_key):
        try:
            measured_path = _nearest_existing_directory(resolved)
        except (OSError, ValueError) as exc:
            probes.append(_probe_failure(resolved, required_bytes, exc))
            continue
        try:
            free_bytes = measure(measured_path)
        except Exception as exc:
            probes.append(
                _probe_failure(
                    resolved,
                    required_bytes,
                    exc,
                    measured_path=measured_path,
                )
            )
            continue
        if (
            not isinstance(free_bytes, int)
            or isinstance(free_bytes, bool)
            or free_bytes < 0
        ):
            probes.append(
                _probe_failure(
                    resolved,
                    required_bytes,
                    ValueError("free-space measurement must be a non-negative integer"),
                    measured_path=measured_path,
                )
            )
            continue
        probes.append(
            DiskCapacityProbe(
                requested_path=resolved,
                measured_path=measured_path,
                free_bytes=free_bytes,
                required_bytes=required_bytes,
            )
        )

    probes.sort(key=lambda probe: _path_sort_key(probe.requested_path))
    rejected = tuple(
        probe
        for probe in probes
        if probe.error is not None
        or probe.free_bytes is None
        or probe.free_bytes < required_bytes
    )
    if rejected:
        diagnostic = {
            "schema": DISK_CAPACITY_DIAGNOSTIC_SCHEMA,
            "status": "rejected",
            "required_bytes": required_bytes,
            "probes": [probe.as_dict() for probe in probes],
        }
        raise DiskCapacityError(_rejection_message(rejected), diagnostic)

    return DiskCapacityReceipt(required_bytes=required_bytes, probes=tuple(probes))
