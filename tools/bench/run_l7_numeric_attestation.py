#!/usr/bin/env python3
"""Build and execute the native L7 numeric performance attestations.

The runner owns source and build provenance. Measured children never invoke
git, rustc, or another provenance helper. Every child instead echoes a
runner-generated nonce and the exact artifact fingerprint supplied in its
environment. Timing is explicitly loop-inclusive; allocation and hook
observers are executed in separate untimed passes.

A schema-valid, semantically valid bundle without a compatible baseline is
evidence-only and can never claim PASS.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import secrets
import statistics
import sys
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402
import perf_calibration  # noqa: E402
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

SCHEMA_PATH = (
    Path(__file__).resolve().parent / "results" / "l7_numeric_attestation.schema.json"
)
DEFAULT_OUTPUT = (
    REPO_ROOT / "logs" / "benchmarks" / "l7_numeric_attestation" / "latest.json"
)
CAPSULE_ACTIVE_DIR = REPO_ROOT / "tmp" / "memory_guard" / "active"
CAPSULE_ARCHIVE_DIR = (
    REPO_ROOT / "logs" / "benchmarks" / "l7_numeric_attestation" / "custody"
)
BUNDLE_SCHEMA_VERSION = 5
CHILD_SCHEMA_VERSION = 3
BUNDLE_KIND = "l7_numeric_performance_attestation_bundle"
SAMPLE_COUNT = 9
TIMING_SCOPE = "loop_inclusive; allocation and hook observers are untimed"
AFFINITY_SCOPE = "current_benchmark_thread"
AUTO_AFFINITY_POLICY = "third_allowed_logical_cpu_avoids_primary_housekeeping"
EXPLICIT_AFFINITY_POLICY = "explicit_cli_mask"
BASE_METRICS = (
    "ns_per_op",
    "allocations_per_op",
    "allocated_bytes_per_op",
    "peak_live_bytes",
)

ABI_CASES = tuple(
    (
        f"decimal.{digits}",
        "abi_boundary_control_decimal",
        {
            "digits": digits,
            "base": 10,
            "digit": "9",
            "measurement": (
                "scanner and counted hook boundary only; no runtime BigInt payload"
            ),
        },
    )
    for digits in (25, 37, 256, 4096, 4300)
) + tuple(
    (
        f"bytes.{width}",
        "abi_boundary_control_bytes",
        {
            "bytes": width,
            "little_endian": True,
            "signed": False,
            "operations": ["from_bytes", "to_bytes", "num_bits"],
            "measurement": (
                "counted hook boundary control only; no runtime BigInt payload"
            ),
        },
    )
    for width in (8, 17, 256, 4096)
) + (
    (
        "bridge.canonical_scalar_decode",
        "bridge_c_header",
        {
            "representation": "canonical_PyLongObject",
            "operation": "compiled_overlay_PyLong_AsLong",
            "legacy_raw_pointer": False,
        },
    ),
    (
        "bridge.managed_proxy_lookup",
        "bridge",
        {
            "representation": "managed_non_scalar",
            "operation": "pyobj_to_handle",
            "expected_proxy_churn": 0,
        },
    ),
    (
        "bridge.singleton_lookup",
        "bridge",
        {
            "representation": "canonical_true_singleton",
            "operation": "pyobj_to_handle",
            "expected_proxy_churn": 0,
        },
    ),
    (
        "bridge.cold_proxy_cycle",
        "bridge",
        {
            "representation": "unique_heap_handle",
            "operation": "handle_to_pyobj+release",
            "working_set": 4096,
        },
    ),
    (
        "bridge.c_header_foreign_decode",
        "bridge_c_header",
        {
            "representation": "foreign_PyLongObject",
            "operation": "compiled_overlay_PyLong_AsLong",
            "scope": "native_test_probe",
        },
    ),
    (
        "bridge.c_header_canonical_scalar_chain",
        "bridge_c_header",
        {
            "representation": "canonical_scalar_objects",
            "operation": "constructor+Py_TYPE+exact+PyNumber_Add+decref",
            "legacy_raw_pointer": False,
        },
    ),
) + tuple(
    (
        f"{format_name}.{class_name}",
        "float_pack",
        {
            "format": format_name,
            "class": class_name,
            "value_bits": value_bits,
            "expect_error": expect_error,
        },
    )
    for format_name, class_name, value_bits, expect_error in (
        ("f16", "normal", "3ff8000000000000", False),
        ("f16", "subnormal", "3e70000000000000", False),
        ("f16", "tie", "3ff0020000000000", False),
        ("f16", "error", "40effe0000000000", True),
        ("f32", "normal", "3ff8000000000000", False),
        ("f32", "subnormal", "36a0000000000000", False),
        ("f32", "tie", "3ff0000010000000", False),
        ("f32", "error", "7fefffffffffffff", True),
    )
) + (
    (
        "complex.sum",
        "complex",
        {
            "operation": "_Py_c_sum",
            "left": [1.25, -2.5],
            "right": [-0.75, 4.0],
        },
    ),
    (
        "complex.pow",
        "complex",
        {
            "operation": "_Py_c_pow",
            "base": [1.25, -2.5],
            "exponent": [-0.75, 4.0],
        },
    ),
)
RUNTIME_CASES = tuple(
    (
        f"runtime.decimal.{digits}",
        "runtime_bigint",
        {
            "digits": digits,
            "base": 10,
            "value_class": "power_of_two" if digits == 4096 else "dense_nines",
            "real_runtime_hook": "int_from_digits",
        },
    )
    for digits in (25, 37, 256, 4096, 4300)
) + tuple(
    (
        f"runtime.bytes.{width}",
        "runtime_bigint",
        {
            "bytes": width,
            "little_endian": True,
            "signed": False,
            "operations": ["int_from_bytes", "int_to_bytes", "int_num_bits"],
            "real_runtime_hooks": True,
        },
    )
    for width in (1, 2, 4, 8, 17, 256, 4096)
)

COMPONENTS = {
    "abi_boundary": {
        "package": "molt-lang-cpython-abi",
        "test": "l7_numeric_perf_attestation",
        "function": "l7_numeric_performance_attestation",
        "prefix": "L7_NUMERIC_ATTESTATION=",
        "kind": "l7_numeric_performance_attestation",
        "allocator_scope": "rust_global_allocator",
        "invariant_metric": "hook_calls_per_op",
        "features": ["l7-test-probe"],
        "timing_instrumentation": (
            "test binary uses a counting global allocator; observer atomics are "
            "disabled during timed loops but allocator-side disabled checks remain"
        ),
        "cases": ABI_CASES,
    },
    "runtime_bigint": {
        "package": "molt-runtime",
        "test": "l7_numeric_runtime_perf_attestation",
        "function": "l7_numeric_runtime_performance_attestation",
        "prefix": "L7_NUMERIC_RUNTIME_ATTESTATION=",
        "kind": "l7_numeric_runtime_performance_attestation",
        "allocator_scope": "test_feature_counting_wrapper_over_production_mimalloc",
        "invariant_metric": "numeric_hook_calls_per_op",
        "features": ["l7-attestation-probe"],
        "timing_instrumentation": (
            "test-feature probe leaves one relaxed TRACK atomic load in each "
            "allocation and numeric hook during timed loops; increments are disabled"
        ),
        "cases": RUNTIME_CASES,
    },
}

_ENVIRONMENT_PREFIXES = (
    "CARGO_",
    "RUST",
    "MOLT_",
    "MIMALLOC_",
)
_ENVIRONMENT_KEYS = {
    "AR",
    "CC",
    "CFLAGS",
    "CXX",
    "CXXFLAGS",
    "LDFLAGS",
    "PATH",
    "PYTHON",
}
_DYNAMIC_ENVIRONMENT_KEYS = {
    "MOLT_L7_BUILD_FINGERPRINT",
    "MOLT_L7_GIT_COMMIT",
    "MOLT_L7_GIT_DIRTY",
    "MOLT_L7_AFFINITY_MASK",
    "MOLT_L7_RUN_NONCE",
    "MOLT_L7_RUSTC",
}


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _canonical_bytes(value: Any) -> bytes:
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
        allow_nan=False,
    ).encode("utf-8")


def _sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _summary(values: list[float]) -> dict[str, Any]:
    if not values or not all(math.isfinite(value) and value >= 0.0 for value in values):
        raise ValueError("summary requires finite nonnegative samples")
    mean = statistics.fmean(values)
    stdev = statistics.stdev(values) if len(values) > 1 else 0.0
    median = statistics.median(values)
    mad = statistics.median(abs(value - median) for value in values)
    cv = (stdev / mean) if mean else 0.0
    return {
        "median": median,
        "mean": mean,
        "cv": cv,
        "robust_cv": (1.4826 * mad / median) if median else cv,
        "min": min(values),
        "max": max(values),
        "samples": values,
    }


def _normalize_affinity_mask(value: str) -> str:
    try:
        mask = int(value, 0)
    except ValueError as exc:
        raise ValueError("affinity mask must be a hexadecimal or decimal integer") from exc
    if mask <= 0 or mask & (mask - 1):
        raise ValueError("affinity mask must select exactly one logical CPU")
    if mask > sys.maxsize:
        raise ValueError("affinity mask exceeds the native pointer width")
    logical_cpus = os.cpu_count()
    if logical_cpus is not None and mask.bit_length() > logical_cpus:
        raise ValueError(
            f"affinity mask selects CPU {mask.bit_length() - 1}, but only "
            f"{logical_cpus} logical CPUs are visible"
        )
    return f"0x{mask:x}"


def _allowed_affinity_mask() -> int:
    """Return the native-pointer-width CPUs this process may execute on."""
    pointer_bits = sys.maxsize.bit_length() + 1
    pointer_mask = (1 << pointer_bits) - 1
    if sys.platform == "win32":
        import ctypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        get_current_process = kernel32.GetCurrentProcess
        get_current_process.argtypes = []
        get_current_process.restype = ctypes.c_void_p
        get_process_affinity_mask = kernel32.GetProcessAffinityMask
        get_process_affinity_mask.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_size_t),
        ]
        get_process_affinity_mask.restype = ctypes.c_int
        process_mask = ctypes.c_size_t()
        system_mask = ctypes.c_size_t()
        if not get_process_affinity_mask(
            get_current_process(),
            ctypes.byref(process_mask),
            ctypes.byref(system_mask),
        ):
            error = ctypes.get_last_error()
            raise OSError(error, "GetProcessAffinityMask failed")
        allowed = int(process_mask.value)
    elif hasattr(os, "sched_getaffinity"):
        allowed = sum(
            1 << cpu for cpu in os.sched_getaffinity(0) if 0 <= cpu < pointer_bits
        )
    else:
        logical_cpus = os.cpu_count() or 1
        allowed = (1 << min(logical_cpus, pointer_bits)) - 1
    allowed &= pointer_mask
    if allowed == 0:
        raise ValueError("no native-pointer-width logical CPU is available")
    return allowed


def _resolve_execution_control(affinity_request: str) -> dict[str, Any]:
    allowed_mask = _allowed_affinity_mask()
    allowed_cpus = [
        cpu for cpu in range(allowed_mask.bit_length()) if allowed_mask & (1 << cpu)
    ]
    if affinity_request.strip().lower() == "auto":
        selected_cpu = allowed_cpus[min(2, len(allowed_cpus) - 1)]
        affinity_mask = 1 << selected_cpu
        selection = "auto"
        selection_policy = AUTO_AFFINITY_POLICY
    else:
        normalized = _normalize_affinity_mask(affinity_request)
        affinity_mask = int(normalized, 16)
        if affinity_mask & allowed_mask != affinity_mask:
            raise ValueError(
                f"affinity mask {normalized} is unavailable to this process; "
                f"allowed mask is {allowed_mask:#x}"
            )
        selected_cpu = affinity_mask.bit_length() - 1
        selection = "explicit"
        selection_policy = EXPLICIT_AFFINITY_POLICY
    return {
        "affinity_mask": f"0x{affinity_mask:x}",
        "allowed_affinity_mask": f"0x{allowed_mask:x}",
        "logical_cpu": selected_cpu,
        "selection": selection,
        "selection_policy": selection_policy,
        "scope": AFFINITY_SCOPE,
    }


def _child_execution_control(execution_control: dict[str, Any]) -> dict[str, str]:
    return {
        "affinity_mask": execution_control["affinity_mask"],
        "scope": execution_control["scope"],
    }


def _execution_control_errors(execution_control: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    affinity_mask = int(execution_control["affinity_mask"], 16)
    allowed_mask = int(execution_control["allowed_affinity_mask"], 16)
    if affinity_mask <= 0 or affinity_mask & (affinity_mask - 1):
        errors.append("runner execution control must select exactly one logical CPU")
    if affinity_mask & allowed_mask != affinity_mask:
        errors.append("runner affinity mask is absent from its recorded allowed mask")
    if execution_control["logical_cpu"] != affinity_mask.bit_length() - 1:
        errors.append("runner logical CPU does not match its affinity mask")
    selection = execution_control["selection"]
    expected_policy = (
        AUTO_AFFINITY_POLICY if selection == "auto" else EXPLICIT_AFFINITY_POLICY
    )
    if execution_control["selection_policy"] != expected_policy:
        errors.append("runner affinity selection policy does not match its mode")
    if selection == "auto":
        allowed_cpus = [
            cpu
            for cpu in range(allowed_mask.bit_length())
            if allowed_mask & (1 << cpu)
        ]
        expected_cpu = allowed_cpus[min(2, len(allowed_cpus) - 1)]
        if affinity_mask != 1 << expected_cpu:
            errors.append("runner automatic affinity does not follow its recorded policy")
    return errors


def _schema_type_matches(value: Any, expected: str) -> bool:
    if expected == "object":
        return isinstance(value, dict)
    if expected == "array":
        return isinstance(value, list)
    if expected == "string":
        return isinstance(value, str)
    if expected == "boolean":
        return isinstance(value, bool)
    if expected == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if expected == "number":
        return (
            isinstance(value, (int, float))
            and not isinstance(value, bool)
            and math.isfinite(float(value))
        )
    if expected == "null":
        return value is None
    raise ValueError(f"unsupported schema type {expected!r}")


def _resolve_schema_ref(root: dict[str, Any], reference: str) -> dict[str, Any]:
    if not reference.startswith("#/"):
        raise ValueError(f"only local schema references are supported: {reference}")
    current: Any = root
    for raw in reference[2:].split("/"):
        key = raw.replace("~1", "/").replace("~0", "~")
        current = current[key]
    if not isinstance(current, dict):
        raise ValueError(f"schema reference does not resolve to an object: {reference}")
    return current


def _schema_errors(
    value: Any,
    schema: dict[str, Any],
    *,
    root: dict[str, Any] | None = None,
    path: str = "$",
) -> list[str]:
    root = schema if root is None else root
    if "$ref" in schema:
        return _schema_errors(
            value,
            _resolve_schema_ref(root, schema["$ref"]),
            root=root,
            path=path,
        )

    errors: list[str] = []
    expected_type = schema.get("type")
    if expected_type is not None:
        candidates = (
            expected_type if isinstance(expected_type, list) else [expected_type]
        )
        if not any(_schema_type_matches(value, candidate) for candidate in candidates):
            return [f"{path}: expected type {expected_type!r}"]
    if "const" in schema and value != schema["const"]:
        errors.append(f"{path}: expected constant {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{path}: value is outside the allowed enum")

    if isinstance(value, dict):
        properties = schema.get("properties", {})
        for key in schema.get("required", []):
            if key not in value:
                errors.append(f"{path}: missing required property {key!r}")
        additional = schema.get("additionalProperties", True)
        for key, child in value.items():
            child_path = f"{path}.{key}"
            if key in properties:
                errors.extend(
                    _schema_errors(child, properties[key], root=root, path=child_path)
                )
            elif additional is False:
                errors.append(f"{path}: unknown property {key!r}")
            elif isinstance(additional, dict):
                errors.extend(
                    _schema_errors(child, additional, root=root, path=child_path)
                )
    elif isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            errors.append(f"{path}: too few items")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            errors.append(f"{path}: too many items")
        item_schema = schema.get("items")
        if isinstance(item_schema, dict):
            for index, child in enumerate(value):
                errors.extend(
                    _schema_errors(
                        child,
                        item_schema,
                        root=root,
                        path=f"{path}[{index}]",
                    )
                )
    elif isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            errors.append(f"{path}: string is too short")
        if "pattern" in schema and re.search(schema["pattern"], value) is None:
            errors.append(f"{path}: string does not match the required pattern")
    elif isinstance(value, (int, float)) and not isinstance(value, bool):
        number = float(value)
        if "minimum" in schema and number < float(schema["minimum"]):
            errors.append(f"{path}: value is below the minimum")
        if "maximum" in schema and number > float(schema["maximum"]):
            errors.append(f"{path}: value is above the maximum")
    return errors


def _load_json_strict(path: Path) -> dict[str, Any]:
    def reject_constant(token: str) -> Any:
        raise ValueError(f"non-finite JSON constant {token!r} is forbidden")

    value = json.loads(path.read_text(encoding="utf-8"), parse_constant=reject_constant)
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def _load_schema() -> dict[str, Any]:
    return _load_json_strict(SCHEMA_PATH)


def _parent_command(args: list[str]) -> bytes:
    env = dict(os.environ)
    env["GIT_OPTIONAL_LOCKS"] = "0"
    completed = _COMMANDS.run(
        args,
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", "replace").strip()
        raise RuntimeError(f"parent provenance command failed: {args!r}: {stderr}")
    return completed.stdout


def _source_snapshot() -> dict[str, Any]:
    commit = _parent_command(["git", "rev-parse", "HEAD"]).decode().strip()
    status = _parent_command(
        ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all"]
    )
    diff = _parent_command(["git", "diff", "--binary", "--no-ext-diff", "HEAD", "--"])
    untracked = _parent_command(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"]
    )
    untracked_digest = hashlib.sha256()
    for raw_path in sorted(part for part in untracked.split(b"\0") if part):
        path = REPO_ROOT / os.fsdecode(raw_path)
        resolved = path.resolve()
        try:
            resolved.relative_to(REPO_ROOT.resolve())
        except ValueError as exc:
            raise RuntimeError(f"untracked path escaped repository: {path}") from exc
        untracked_digest.update(raw_path)
        untracked_digest.update(b"\0")
        untracked_digest.update(bytes.fromhex(_sha256_file(resolved)))
    snapshot = {
        "git_commit": commit,
        "git_dirty": bool(status),
        "status_sha256": _sha256_bytes(status),
        "worktree_diff_sha256": _sha256_bytes(diff),
        "untracked_sha256": untracked_digest.hexdigest(),
    }
    snapshot["fingerprint"] = _sha256_bytes(_canonical_bytes(snapshot))
    return snapshot


def _captured_environment(env: dict[str, str]) -> dict[str, str]:
    captured: dict[str, str] = {}
    for key, value in sorted(env.items()):
        if key in _DYNAMIC_ENVIRONMENT_KEYS:
            continue
        if key in _ENVIRONMENT_KEYS or key.startswith(_ENVIRONMENT_PREFIXES):
            captured[key] = _sha256_bytes(value.encode("utf-8", "surrogatepass"))
    return captured


def _build_fingerprints(
    configuration: dict[str, Any],
    *,
    source_fingerprint: str,
    cargo_lock_sha256: str,
    executable_sha256: str,
) -> tuple[str, str]:
    configuration_fingerprint = _sha256_bytes(_canonical_bytes(configuration))
    artifact_payload = {
        "configuration_fingerprint": configuration_fingerprint,
        "source_fingerprint": source_fingerprint,
        "cargo_lock_sha256": cargo_lock_sha256,
        "executable_sha256": executable_sha256,
    }
    return configuration_fingerprint, _sha256_bytes(_canonical_bytes(artifact_payload))


def _validate_policy(
    *,
    runs: int,
    timeout: float,
    max_robust_cv: float,
    max_raw_cv: float,
    max_time_regression: float,
    max_allocation_regression: float,
    max_allocated_bytes_regression: float,
    max_peak_live_regression: float,
    max_rss_regression: float,
    max_measured_rss_bytes: int | None,
) -> None:
    if isinstance(runs, bool) or not isinstance(runs, int) or not 7 <= runs <= 9:
        raise ValueError("--runs must be an integer between 7 and 9")
    if not math.isfinite(timeout) or timeout <= 0.0:
        raise ValueError("--timeout must be finite and positive")
    policies = {
        "--max-robust-cv": max_robust_cv,
        "--max-raw-cv": max_raw_cv,
        "--max-time-regression": max_time_regression,
        "--max-allocation-regression": max_allocation_regression,
        "--max-allocated-bytes-regression": max_allocated_bytes_regression,
        "--max-peak-live-regression": max_peak_live_regression,
        "--max-rss-regression": max_rss_regression,
    }
    for name, value in policies.items():
        if not math.isfinite(value) or not 0.0 <= value <= 1.0:
            raise ValueError(f"{name} must be finite and between 0 and 1")
    if max_measured_rss_bytes is not None and (
        isinstance(max_measured_rss_bytes, bool)
        or not isinstance(max_measured_rss_bytes, int)
        or max_measured_rss_bytes <= 0
    ):
        raise ValueError("--max-measured-rss-bytes must be a positive integer")


def _build_test_executable(
    config: dict[str, Any],
    *,
    timeout: float,
    source: dict[str, Any],
    rustc: str,
    cargo_lock_sha256: str,
) -> tuple[Path, dict[str, Any]]:
    command = [
        "cargo",
        "test",
        "--locked",
        "-p",
        config["package"],
        "--release",
        "--test",
        config["test"],
        "--no-run",
        "--message-format=json",
    ]
    if config["features"]:
        command.extend(["--features", ",".join(config["features"])])
    build_env = dict(os.environ)
    build_env["CARGO_TERM_COLOR"] = "never"
    completed = harness_memory_guard.guarded_completed_process(
        command,
        prefix="MOLT_BENCH",
        cwd=REPO_ROOT,
        env=build_env,
        capture_output=True,
        text=True,
        timeout=timeout,
        limits=harness_memory_guard.limits_from_env("MOLT_BENCH", build_env),
        progress_label=f"L7 numeric attestation build ({config['package']})",
    )
    executable: Path | None = None
    for line in completed.stdout.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = event.get("target") or {}
        if (
            event.get("reason") == "compiler-artifact"
            and target.get("name") == config["test"]
            and event.get("executable")
        ):
            executable = Path(event["executable"]).resolve()
    if completed.returncode != 0 or executable is None or not executable.is_file():
        sys.stderr.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise RuntimeError("release attestation test executable did not build")

    executable_sha256 = _sha256_file(executable)
    configuration = {
        "package": config["package"],
        "test": config["test"],
        "profile": "release",
        "features": list(config["features"]),
        "rustc": rustc,
        "timing_instrumentation": config["timing_instrumentation"],
        "environment": _captured_environment(build_env),
    }
    configuration_fingerprint, artifact_fingerprint = _build_fingerprints(
        configuration,
        source_fingerprint=source["fingerprint"],
        cargo_lock_sha256=cargo_lock_sha256,
        executable_sha256=executable_sha256,
    )
    return executable, {
        "command": command,
        "stderr_sha256": _sha256_bytes(completed.stderr.encode("utf-8")),
        "executable_sha256": executable_sha256,
        "configuration": configuration,
        "configuration_fingerprint": configuration_fingerprint,
        "artifact_fingerprint": artifact_fingerprint,
    }


def _parse_attestation(
    stdout: str,
    *,
    config: dict[str, Any],
    schema: dict[str, Any],
) -> dict[str, Any]:
    prefix = config["prefix"]
    lines = [line for line in stdout.splitlines() if prefix in line]
    if len(lines) != 1:
        raise RuntimeError(
            f"expected one {prefix.rstrip('=')} record, found {len(lines)}"
        )
    result = json.loads(
        lines[0].split(prefix, 1)[1],
        parse_constant=lambda token: (_ for _ in ()).throw(
            ValueError(f"non-finite JSON constant {token!r} is forbidden")
        ),
    )
    errors = _schema_errors(
        result,
        schema["$defs"]["childAttestation"],
        root=schema,
        path="$child",
    )
    if errors:
        raise RuntimeError(
            "child attestation violates schema: " + "; ".join(errors[:8])
        )
    if result["kind"] != config["kind"]:
        raise RuntimeError("child attestation kind does not match component")
    if result["allocator_scope"] != config["allocator_scope"]:
        raise RuntimeError("child allocator scope does not match component")
    return result


def _quiescence_ok(value: dict[str, Any]) -> bool:
    return value.get("certified") is True and value.get("competing_builds") == 0


def _write_json_atomic(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(payload, sort_keys=True, indent=2, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def _capsule_paths(nonce: str, component: str, run: int) -> tuple[Path, Path]:
    name = f"l7-{nonce}-{component}-run-{run:02d}.json"
    return CAPSULE_ACTIVE_DIR / name, CAPSULE_ARCHIVE_DIR / name


def _run_component(
    name: str,
    config: dict[str, Any],
    *,
    runs: int,
    timeout: float,
    schema: dict[str, Any],
    source: dict[str, Any],
    rustc: str,
    cargo_lock_sha256: str,
    run_nonce: str,
    max_measured_rss_bytes: int | None,
    affinity_mask: str,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    executable, build = _build_test_executable(
        config,
        timeout=timeout,
        source=source,
        rustc=rustc,
        cargo_lock_sha256=cargo_lock_sha256,
    )
    argv = [
        str(executable),
        config["function"],
        "--exact",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    child_env = {
        "MOLT_L7_GIT_COMMIT": source["git_commit"],
        "MOLT_L7_GIT_DIRTY": "true" if source["git_dirty"] else "false",
        "MOLT_L7_RUSTC": rustc,
        "MOLT_L7_BUILD_FINGERPRINT": build["artifact_fingerprint"],
        "MOLT_L7_AFFINITY_MASK": affinity_mask,
        "MOLT_L7_RUN_NONCE": run_nonce,
    }
    run_rows: list[dict[str, Any]] = []
    attestations: list[dict[str, Any]] = []
    for index in range(1, runs + 1):
        before = asdict(perf_calibration.measure_quiescence())
        active_capsule, archived_capsule = _capsule_paths(run_nonce, name, index)
        capsule = {
            "kind": "l7_numeric_attestation_death_capsule",
            "command": argv,
            "cwd": str(REPO_ROOT),
            "guard_pid": os.getpid(),
            "child_pid": None,
            "component": name,
            "run": index,
            "run_nonce": run_nonce,
            "status": "starting",
            "started_at_utc": _utc_now(),
            "evidence_path": str(archived_capsule),
            "quiescence_before": before,
        }
        _write_json_atomic(active_capsule, capsule)
        try:
            measured = perf_calibration.run_and_measure(
                argv,
                timeout=timeout,
                cwd=str(REPO_ROOT),
                env=child_env,
            )
            after = asdict(perf_calibration.measure_quiescence())
            capsule.update(
                {
                    "status": (
                        "completed"
                        if measured.returncode == 0 and not measured.timed_out
                        else "failed"
                    ),
                    "completed_at_utc": _utc_now(),
                    "returncode": measured.returncode,
                    "timed_out": measured.timed_out,
                    "peak_rss_bytes": measured.peak_rss_bytes,
                    "quiescence_after": after,
                }
            )
        except BaseException as exc:
            capsule.update(
                {
                    "status": "runner_error",
                    "completed_at_utc": _utc_now(),
                    "error": f"{type(exc).__name__}: {exc}",
                }
            )
            _write_json_atomic(active_capsule, capsule)
            archived_capsule.parent.mkdir(parents=True, exist_ok=True)
            active_capsule.replace(archived_capsule)
            raise
        _write_json_atomic(active_capsule, capsule)
        archived_capsule.parent.mkdir(parents=True, exist_ok=True)
        active_capsule.replace(archived_capsule)

        if measured.returncode != 0 or measured.timed_out:
            sys.stderr.write(measured.stdout)
            sys.stderr.write(measured.stderr)
            raise RuntimeError(
                f"direct attestation run {index}/{runs} failed: "
                f"rc={measured.returncode} timeout={measured.timed_out}"
            )
        if measured.peak_rss_bytes is None:
            raise RuntimeError("peak RSS was unavailable for a direct run")
        if (
            max_measured_rss_bytes is not None
            and measured.peak_rss_bytes > max_measured_rss_bytes
        ):
            raise RuntimeError(
                f"{name} run {index} peak RSS {measured.peak_rss_bytes} exceeded "
                f"ceiling {max_measured_rss_bytes}"
            )
        attestation = _parse_attestation(
            measured.stdout,
            config=config,
            schema=schema,
        )
        expected_source = {
            "git_commit": source["git_commit"],
            "git_dirty": source["git_dirty"],
            "rustc": rustc,
            "build_fingerprint": build["artifact_fingerprint"],
            "run_nonce": run_nonce,
        }
        if attestation["source"] != expected_source:
            raise RuntimeError(f"{name} run {index} did not echo parent provenance")
        expected_execution = {
            "affinity_mask": affinity_mask,
            "scope": AFFINITY_SCOPE,
        }
        if attestation["execution_control"] != expected_execution:
            raise RuntimeError(f"{name} run {index} did not enforce execution control")
        canonical = _canonical_bytes(attestation)
        attestations.append(attestation)
        run_rows.append(
            {
                "run": index,
                "elapsed_ms": measured.elapsed_s * 1000.0,
                "peak_rss_bytes": measured.peak_rss_bytes,
                "attestation_sha256": _sha256_bytes(canonical),
                "quiescence_before": before,
                "quiescence_after": after,
                "capsule_path": str(archived_capsule.relative_to(REPO_ROOT)),
            }
        )
    elapsed = [float(row["elapsed_ms"]) for row in run_rows]
    rss = [float(row["peak_rss_bytes"]) for row in run_rows]
    return (
        {
            "component": name,
            "measurement": "whole child harness RSS; provenance commands excluded",
            "coverage": {
                "timing_scope": TIMING_SCOPE,
                "timing_instrumentation": config["timing_instrumentation"],
                "claim": (
                    "relative comparison against an identical configuration "
                    "fingerprint; not absolute uninstrumented production timing"
                ),
            },
            "elapsed_ms": _summary(elapsed),
            "peak_rss_bytes": _summary(rss),
            "runs": run_rows,
            "runner": {
                "direct_executable": str(executable),
                "argv": argv,
                "runs": runs,
                "build": build,
            },
        },
        attestations,
    )


def _number(value: Any, *, context: str, errors: list[str]) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        errors.append(f"{context}: expected a number")
        return None
    number = float(value)
    if not math.isfinite(number) or number < 0.0:
        errors.append(f"{context}: expected a finite nonnegative number")
        return None
    return number


def _summary_matches(
    reported: dict[str, Any],
    recomputed: dict[str, Any],
    *,
    context: str,
    errors: list[str],
) -> None:
    for metric in ("median", "cv", "robust_cv"):
        actual = _number(
            reported.get(metric), context=f"{context}.{metric}", errors=errors
        )
        if actual is None:
            continue
        expected = float(recomputed[metric])
        if not math.isclose(actual, expected, rel_tol=1e-6, abs_tol=5e-6):
            errors.append(
                f"{context}.{metric}: reported {actual} != recomputed {expected}"
            )


def _validate_dispersion(
    summary: dict[str, Any],
    *,
    context: str,
    max_robust_cv: float,
    max_raw_cv: float,
    errors: list[str],
) -> None:
    robust_cv = float(summary["robust_cv"])
    raw_cv = float(summary["cv"])
    if robust_cv > max_robust_cv:
        errors.append(
            f"{context}: robust CV {robust_cv:.4f}>{max_robust_cv:.4f}"
        )
    if raw_cv > max_raw_cv:
        errors.append(f"{context}: raw CV {raw_cv:.4f}>{max_raw_cv:.4f}")


def _recompute_case(
    case: dict[str, Any],
    *,
    config: dict[str, Any],
    context: str,
    max_robust_cv: float,
    max_raw_cv: float,
    errors: list[str],
) -> dict[str, dict[str, Any]]:
    metrics = BASE_METRICS + (config["invariant_metric"],)
    iterations = case["iterations_per_sample"]
    observer_iterations = case["observer_iterations_per_sample"]
    calibration_target_ns = case["calibration_target_ns"]
    minimum_sample_ns = case["minimum_sample_ns"]
    if case["sample_count"] != SAMPLE_COUNT or len(case["samples"]) != SAMPLE_COUNT:
        errors.append(f"{context}: sample count must be exactly {SAMPLE_COUNT}")
    if (
        iterations <= 0
        or observer_iterations <= 0
        or calibration_target_ns <= 0
        or minimum_sample_ns <= 0
    ):
        errors.append(f"{context}: iteration and calibration counts must be positive")
    if calibration_target_ns < minimum_sample_ns * 10:
        errors.append(f"{context}: calibration target lacks 10x minimum headroom")
    if case["timing_scope"] != TIMING_SCOPE:
        errors.append(f"{context}: timing scope drift")

    values: dict[str, list[float]] = {metric: [] for metric in metrics}
    for sample_index, sample in enumerate(case["samples"], 1):
        if set(sample) != set(metrics):
            errors.append(f"{context} sample {sample_index}: metric set drift")
            continue
        for metric in metrics:
            number = _number(
                sample[metric],
                context=f"{context} sample {sample_index}.{metric}",
                errors=errors,
            )
            if number is not None:
                values[metric].append(number)
        ns = (
            values["ns_per_op"][-1]
            if len(values["ns_per_op"]) == sample_index
            else None
        )
        if ns is not None and ns * iterations < minimum_sample_ns:
            errors.append(
                f"{context} sample {sample_index}: sample duration "
                f"{ns * iterations:.0f}ns<{minimum_sample_ns}ns minimum"
            )

    recomputed: dict[str, dict[str, Any]] = {}
    if set(case["summary"]) != set(metrics):
        errors.append(f"{context}: summary metric set drift")
    for metric in metrics:
        if len(values[metric]) != len(case["samples"]):
            continue
        result = _summary(values[metric])
        recomputed[metric] = result
        reported = case["summary"].get(metric)
        if isinstance(reported, dict):
            _summary_matches(
                reported,
                result,
                context=f"{context}.summary.{metric}",
                errors=errors,
            )
        if metric == config["invariant_metric"]:
            if any(value != values[metric][0] for value in values[metric][1:]):
                errors.append(f"{context}: {metric} is not exact within the process")
        else:
            _validate_dispersion(
                result,
                context=f"{context}: {metric}",
                max_robust_cv=max_robust_cv,
                max_raw_cv=max_raw_cv,
                errors=errors,
            )
    return recomputed


def _aggregate_bundle(
    bundle: dict[str, Any],
    max_robust_cv: float,
    max_raw_cv: float,
) -> tuple[dict[str, Any], list[str]]:
    errors: list[str] = []
    errors.extend(
        _execution_control_errors(bundle["runner"]["execution_control"])
    )
    aggregated: dict[str, Any] = {}
    source = bundle["source"]
    for label in ("start", "end"):
        snapshot = source[label]
        snapshot_payload = {
            key: value for key, value in snapshot.items() if key != "fingerprint"
        }
        if snapshot["fingerprint"] != _sha256_bytes(_canonical_bytes(snapshot_payload)):
            errors.append(f"source {label} fingerprint digest mismatch")
    if source["start"] != source["end"]:
        errors.append("source fingerprint changed between start and end")
    if source["start"]["git_dirty"] or source["end"]["git_dirty"]:
        errors.append("source tree is dirty")
    if set(bundle["attestations"]) != set(COMPONENTS):
        errors.append("component attestation set mismatch")
        return aggregated, errors
    if set(bundle["process"]) != set(COMPONENTS):
        errors.append("component process set mismatch")
        return aggregated, errors

    for component, config in COMPONENTS.items():
        runs = bundle["attestations"][component]
        process = bundle["process"][component]
        process_runs = process["runs"]
        if len(runs) != bundle["runner"]["runs_per_component"]:
            errors.append(f"{component}: attestation run count drift")
            continue
        if len(process_runs) != len(runs):
            errors.append(f"{component}: process/attestation run count mismatch")
            continue
        expected_manifest = list(config["cases"])
        case_runs: dict[str, list[tuple[dict[str, Any], dict[str, Any]]]] = {
            name: [] for name, _family, _input in expected_manifest
        }
        build = process["runner"]["build"]
        if process["component"] != component:
            errors.append(f"{component}: process component identity drift")
        if process["coverage"]["timing_scope"] != TIMING_SCOPE:
            errors.append(f"{component}: process timing scope drift")
        if (
            process["coverage"]["timing_instrumentation"]
            != config["timing_instrumentation"]
        ):
            errors.append(f"{component}: timing instrumentation declaration drift")
        configuration = build["configuration"]
        expected_configuration = {
            "package": config["package"],
            "test": config["test"],
            "profile": "release",
            "features": list(config["features"]),
            "rustc": source["rustc"],
            "timing_instrumentation": config["timing_instrumentation"],
            "environment": configuration["environment"],
        }
        if configuration != expected_configuration:
            errors.append(f"{component}: build configuration declaration drift")
        expected_configuration_fingerprint, expected_artifact_fingerprint = (
            _build_fingerprints(
                configuration,
                source_fingerprint=source["start"]["fingerprint"],
                cargo_lock_sha256=source["cargo_lock_sha256"],
                executable_sha256=build["executable_sha256"],
            )
        )
        if build["configuration_fingerprint"] != expected_configuration_fingerprint:
            errors.append(f"{component}: build configuration digest mismatch")
        if build["artifact_fingerprint"] != expected_artifact_fingerprint:
            errors.append(f"{component}: build artifact digest mismatch")
        expected_child_source = {
            "git_commit": source["start"]["git_commit"],
            "git_dirty": source["start"]["git_dirty"],
            "rustc": source["rustc"],
            "build_fingerprint": build["artifact_fingerprint"],
            "run_nonce": source["run_nonce"],
        }
        expected_execution = _child_execution_control(
            bundle["runner"]["execution_control"]
        )

        elapsed = []
        rss = []
        for run_index, (attestation, process_run) in enumerate(
            zip(runs, process_runs, strict=True), 1
        ):
            context = f"{component} run {run_index}"
            if process_run["run"] != run_index:
                errors.append(f"{context}: run index drift")
            if not _quiescence_ok(process_run["quiescence_before"]):
                errors.append(f"{context}: pre-run quiescence not certified")
            if not _quiescence_ok(process_run["quiescence_after"]):
                errors.append(f"{context}: post-run quiescence not certified")
            elapsed_value = _number(
                process_run["elapsed_ms"],
                context=f"{context}.elapsed_ms",
                errors=errors,
            )
            rss_value = _number(
                process_run["peak_rss_bytes"],
                context=f"{context}.peak_rss_bytes",
                errors=errors,
            )
            if elapsed_value is not None:
                elapsed.append(elapsed_value)
            if rss_value is not None:
                rss.append(rss_value)
            if process_run["attestation_sha256"] != _sha256_bytes(
                _canonical_bytes(attestation)
            ):
                errors.append(f"{context}: attestation digest mismatch")
            if attestation["schema_version"] != CHILD_SCHEMA_VERSION:
                errors.append(f"{context}: child schema version mismatch")
            if attestation["kind"] != config["kind"]:
                errors.append(f"{context}: child kind mismatch")
            if attestation["allocator_scope"] != config["allocator_scope"]:
                errors.append(f"{context}: allocator scope mismatch")
            if attestation["profile"] != "release":
                errors.append(f"{context}: non-release profile")
            if attestation["sample_count"] != SAMPLE_COUNT:
                errors.append(f"{context}: child sample count drift")
            if attestation["source"] != expected_child_source:
                errors.append(f"{context}: parent provenance echo mismatch")
            if attestation["execution_control"] != expected_execution:
                errors.append(f"{context}: execution control drift")

            manifest = [
                (case["name"], case["family"], case["input"])
                for case in attestation["cases"]
            ]
            if manifest != expected_manifest:
                errors.append(f"{context}: ordered case manifest drift")
                continue
            for case in attestation["cases"]:
                case_context = f"{context}/{case['name']}"
                summaries = _recompute_case(
                    case,
                    config=config,
                    context=case_context,
                    max_robust_cv=max_robust_cv,
                    max_raw_cv=max_raw_cv,
                    errors=errors,
                )
                case_runs[case["name"]].append((case, summaries))

        if len(elapsed) == len(process_runs):
            _summary_matches(
                process["elapsed_ms"],
                _summary(elapsed),
                context=f"{component}.process.elapsed_ms",
                errors=errors,
            )
        if len(rss) == len(process_runs):
            _summary_matches(
                process["peak_rss_bytes"],
                _summary(rss),
                context=f"{component}.process.peak_rss_bytes",
                errors=errors,
            )
        for metric_name in ("elapsed_ms", "peak_rss_bytes"):
            _validate_dispersion(
                process[metric_name],
                context=f"{component}: process {metric_name}",
                max_robust_cv=max_robust_cv,
                max_raw_cv=max_raw_cv,
                errors=errors,
            )

        component_aggregate: dict[str, Any] = {}
        metrics = BASE_METRICS + (config["invariant_metric"],)
        for case_name, family, expected_input in expected_manifest:
            copies = case_runs[case_name]
            if len(copies) != len(runs):
                errors.append(f"{component}/{case_name}: missing from one or more runs")
                continue
            first_input = copies[0][0]["input"]
            if any(case["input"] != first_input for case, _summary in copies[1:]):
                errors.append(f"{component}/{case_name}: input drift across runs")
            if first_input != expected_input:
                errors.append(f"{component}/{case_name}: pinned input contract drift")
            metric_aggregate: dict[str, Any] = {}
            for metric in metrics:
                medians = [
                    summaries[metric]["median"]
                    for _case, summaries in copies
                    if metric in summaries
                ]
                if len(medians) != len(copies):
                    continue
                outer = _summary(medians)
                if metric == config["invariant_metric"]:
                    if any(value != medians[0] for value in medians[1:]):
                        errors.append(
                            f"{component}/{case_name}: cross-process {metric} drift"
                        )
                else:
                    _validate_dispersion(
                        outer,
                        context=f"{component}/{case_name}: cross-process {metric}",
                        max_robust_cv=max_robust_cv,
                        max_raw_cv=max_raw_cv,
                        errors=errors,
                    )
                metric_aggregate[metric] = outer
            component_aggregate[case_name] = {
                "family": family,
                "input": first_input,
                "metrics": metric_aggregate,
            }
        aggregated[component] = component_aggregate
    return aggregated, errors


def _change(current: float, baseline: float) -> float:
    if baseline:
        return current / baseline - 1.0
    return 0.0 if current == 0.0 else 1.0


def _invalid_comparison(errors: list[str]) -> dict[str, Any]:
    return {
        "status": "invalid",
        "performance_claim": False,
        "errors": errors,
        "rows": [],
        "process_rows": [],
        "violations": [],
    }


def _compare_to_baseline(
    current: dict[str, Any],
    baseline: dict[str, Any],
    *,
    schema: dict[str, Any],
    max_robust_cv: float,
    max_raw_cv: float,
    max_time_regression: float,
    max_allocation_regression: float,
    max_allocated_bytes_regression: float,
    max_peak_live_regression: float,
    max_rss_regression: float,
) -> dict[str, Any]:
    schema_errors = _schema_errors(baseline, schema, root=schema)
    if schema_errors:
        return _invalid_comparison(
            [f"baseline schema: {error}" for error in schema_errors]
        )
    current_agg, current_errors = _aggregate_bundle(
        current, max_robust_cv, max_raw_cv
    )
    baseline_agg, baseline_errors = _aggregate_bundle(
        baseline, max_robust_cv, max_raw_cv
    )
    errors = [f"current: {error}" for error in current_errors]
    errors.extend(f"baseline: {error}" for error in baseline_errors)
    if current["host"]["fingerprint"] != baseline["host"]["fingerprint"]:
        errors.append("baseline host fingerprint differs from current host")
    if (
        current["runner"]["execution_control"]
        != baseline["runner"]["execution_control"]
    ):
        errors.append("baseline execution control differs from current apparatus")
    for key in ("runner_sha256", "schema_sha256"):
        if current["source"][key] != baseline["source"][key]:
            errors.append(f"baseline {key} differs from current apparatus")
    for component in COMPONENTS:
        current_build = current["process"][component]["runner"]["build"]
        baseline_build = baseline["process"][component]["runner"]["build"]
        if (
            current_build["configuration_fingerprint"]
            != baseline_build["configuration_fingerprint"]
        ):
            errors.append(f"{component}: build configuration fingerprint differs")
        if set(current_agg.get(component, {})) != set(baseline_agg.get(component, {})):
            errors.append(f"{component}: baseline/current case sets differ")
            continue
        for case_name in current_agg.get(component, {}):
            current_case = current_agg[component][case_name]
            baseline_case = baseline_agg[component][case_name]
            if current_case["family"] != baseline_case["family"]:
                errors.append(f"{component}/{case_name}: family differs from baseline")
            if current_case["input"] != baseline_case["input"]:
                errors.append(f"{component}/{case_name}: input differs from baseline")
    if errors:
        return _invalid_comparison(errors)

    rows: list[dict[str, Any]] = []
    violations: list[dict[str, Any]] = []
    thresholds = {
        "ns_per_op": max_time_regression,
        "allocations_per_op": max_allocation_regression,
        "allocated_bytes_per_op": max_allocated_bytes_regression,
        "peak_live_bytes": max_peak_live_regression,
    }
    for component, cases in current_agg.items():
        invariant_metric = COMPONENTS[component]["invariant_metric"]
        for case_name, current_case in cases.items():
            baseline_case = baseline_agg[component][case_name]
            row: dict[str, Any] = {
                "component": component,
                "case": case_name,
                "metrics": {},
            }
            failed = False
            for metric, threshold in thresholds.items():
                current_value = current_case["metrics"][metric]["median"]
                baseline_value = baseline_case["metrics"][metric]["median"]
                delta = _change(current_value, baseline_value)
                metric_failed = delta > threshold
                failed = failed or metric_failed
                row["metrics"][metric] = {
                    "baseline": baseline_value,
                    "current": current_value,
                    "change_fraction": delta,
                    "threshold_fraction": threshold,
                    "status": "regression" if metric_failed else "pass",
                }
            current_invariant = current_case["metrics"][invariant_metric]["median"]
            baseline_invariant = baseline_case["metrics"][invariant_metric]["median"]
            invariant_failed = current_invariant != baseline_invariant
            failed = failed or invariant_failed
            row["metrics"][invariant_metric] = {
                "baseline": baseline_invariant,
                "current": current_invariant,
                "invariant": "exact equality",
                "status": "regression" if invariant_failed else "pass",
            }
            row["status"] = "regression" if failed else "pass"
            rows.append(row)
            if failed:
                violations.append(row)

    process_rows: list[dict[str, Any]] = []
    for component in COMPONENTS:
        baseline_rss = baseline["process"][component]["peak_rss_bytes"]["median"]
        current_rss = current["process"][component]["peak_rss_bytes"]["median"]
        delta = _change(current_rss, baseline_rss)
        row = {
            "component": component,
            "scope": "whole child harness; provenance commands excluded",
            "baseline_peak_rss_bytes": baseline_rss,
            "current_peak_rss_bytes": current_rss,
            "change_fraction": delta,
            "threshold_fraction": max_rss_regression,
            "status": "regression" if delta > max_rss_regression else "pass",
        }
        process_rows.append(row)
        if row["status"] == "regression":
            violations.append(row)
    return {
        "status": "fail" if violations else "pass",
        "performance_claim": not violations,
        "errors": [],
        "rows": rows,
        "process_rows": process_rows,
        "violations": violations,
    }


def run_attestation(
    runs: int,
    timeout: float,
    *,
    baseline: dict[str, Any] | None,
    max_robust_cv: float,
    max_raw_cv: float,
    max_time_regression: float,
    max_allocation_regression: float,
    max_allocated_bytes_regression: float,
    max_peak_live_regression: float,
    max_rss_regression: float,
    max_measured_rss_bytes: int | None,
    execution_control: dict[str, Any],
) -> dict[str, Any]:
    _validate_policy(
        runs=runs,
        timeout=timeout,
        max_robust_cv=max_robust_cv,
        max_raw_cv=max_raw_cv,
        max_time_regression=max_time_regression,
        max_allocation_regression=max_allocation_regression,
        max_allocated_bytes_regression=max_allocated_bytes_regression,
        max_peak_live_regression=max_peak_live_regression,
        max_rss_regression=max_rss_regression,
        max_measured_rss_bytes=max_measured_rss_bytes,
    )
    affinity_mask = execution_control["affinity_mask"]
    schema = _load_schema()
    source_start = _source_snapshot()
    rustc = _parent_command(["rustc", "--version", "--verbose"]).decode().strip()
    cargo_lock_sha256 = _sha256_file(REPO_ROOT / "Cargo.lock")
    runner_sha256 = _sha256_file(Path(__file__).resolve())
    schema_sha256 = _sha256_file(SCHEMA_PATH)
    run_nonce = secrets.token_hex(16)

    process: dict[str, Any] = {}
    attestations: dict[str, Any] = {}
    for name, config in COMPONENTS.items():
        process[name], attestations[name] = _run_component(
            name,
            config,
            runs=runs,
            timeout=timeout,
            schema=schema,
            source=source_start,
            rustc=rustc,
            cargo_lock_sha256=cargo_lock_sha256,
            run_nonce=run_nonce,
            max_measured_rss_bytes=max_measured_rss_bytes,
            affinity_mask=affinity_mask,
        )
    source_end = _source_snapshot()
    fingerprint = perf_calibration.host_fingerprint()
    fingerprint_data = asdict(fingerprint)
    fingerprint_data["key"] = fingerprint.key()
    policy = {
        "max_robust_cv": max_robust_cv,
        "max_raw_cv": max_raw_cv,
        "max_time_regression": max_time_regression,
        "max_allocation_regression": max_allocation_regression,
        "max_allocated_bytes_regression": max_allocated_bytes_regression,
        "max_peak_live_regression": max_peak_live_regression,
        "max_rss_regression": max_rss_regression,
        "max_measured_rss_bytes": max_measured_rss_bytes,
    }
    result: dict[str, Any] = {
        "schema_version": BUNDLE_SCHEMA_VERSION,
        "kind": BUNDLE_KIND,
        "generated_at_utc": _utc_now(),
        "runner": {
            "path": str(Path(__file__).resolve().relative_to(REPO_ROOT)),
            "runs_per_component": runs,
            "timing_scope": TIMING_SCOPE,
            "case_order": "exact component manifests; order is validated",
            "scope": "native release harness; no wasm32, assembly, or code-size claim",
            "execution_control": execution_control,
            "policy": policy,
        },
        "source": {
            "run_nonce": run_nonce,
            "rustc": rustc,
            "cargo_lock_sha256": cargo_lock_sha256,
            "runner_sha256": runner_sha256,
            "schema_sha256": schema_sha256,
            "start": source_start,
            "end": source_end,
        },
        "host": {
            "fingerprint": fingerprint_data,
            "quiescence_policy": "certified immediately before and after every direct run",
        },
        "process": process,
        "attestations": attestations,
        "aggregated_cases": {},
        "validation": {
            "valid": False,
            "max_robust_cv": max_robust_cv,
            "max_raw_cv": max_raw_cv,
            "errors": [],
        },
        "comparison": _invalid_comparison(["validation has not run"]),
    }
    aggregated, validation_errors = _aggregate_bundle(
        result, max_robust_cv, max_raw_cv
    )
    result["aggregated_cases"] = aggregated
    result["validation"] = {
        "valid": not validation_errors,
        "max_robust_cv": max_robust_cv,
        "max_raw_cv": max_raw_cv,
        "errors": validation_errors,
    }
    if baseline is None:
        result["comparison"] = {
            "status": "invalid" if validation_errors else "evidence_only",
            "performance_claim": False,
            "errors": validation_errors
            + ["a compatible baseline is mandatory for PASS"],
            "rows": [],
            "process_rows": [],
            "violations": [],
        }
    else:
        result["comparison"] = _compare_to_baseline(
            result,
            baseline,
            schema=schema,
            max_robust_cv=max_robust_cv,
            max_raw_cv=max_raw_cv,
            max_time_regression=max_time_regression,
            max_allocation_regression=max_allocation_regression,
            max_allocated_bytes_regression=max_allocated_bytes_regression,
            max_peak_live_regression=max_peak_live_regression,
            max_rss_regression=max_rss_regression,
        )
    generated_schema_errors = _schema_errors(result, schema, root=schema)
    if generated_schema_errors:
        raise RuntimeError(
            "generated bundle violates authoritative schema: "
            + "; ".join(generated_schema_errors[:12])
        )
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=7, choices=range(7, 10))
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--baseline",
        type=Path,
        help="compatible clean-host bundle required for a performance PASS",
    )
    parser.add_argument("--max-robust-cv", type=float, default=0.10)
    parser.add_argument("--max-raw-cv", type=float, default=0.25)
    parser.add_argument(
        "--affinity-mask",
        default="auto",
        help=(
            "single-logical-CPU mask, or auto to choose the third allowed logical "
            "CPU and avoid the primary housekeeping lane (default: auto)"
        ),
    )
    parser.add_argument("--max-time-regression", type=float, default=0.15)
    parser.add_argument("--max-allocation-regression", type=float, default=0.0)
    parser.add_argument("--max-allocated-bytes-regression", type=float, default=0.0)
    parser.add_argument("--max-peak-live-regression", type=float, default=0.15)
    parser.add_argument("--max-rss-regression", type=float, default=0.15)
    parser.add_argument(
        "--max-measured-rss-bytes",
        type=int,
        default=None,
        help="abort if the precise Job/process-tree peak exceeds this ceiling",
    )
    args = parser.parse_args(argv)
    try:
        _validate_policy(
            runs=args.runs,
            timeout=args.timeout,
            max_robust_cv=args.max_robust_cv,
            max_raw_cv=args.max_raw_cv,
            max_time_regression=args.max_time_regression,
            max_allocation_regression=args.max_allocation_regression,
            max_allocated_bytes_regression=args.max_allocated_bytes_regression,
            max_peak_live_regression=args.max_peak_live_regression,
            max_rss_regression=args.max_rss_regression,
            max_measured_rss_bytes=args.max_measured_rss_bytes,
        )
        execution_control = _resolve_execution_control(args.affinity_mask)
    except ValueError as exc:
        parser.error(str(exc))

    baseline = None
    if args.baseline:
        baseline_path = (
            args.baseline if args.baseline.is_absolute() else REPO_ROOT / args.baseline
        )
        baseline = _load_json_strict(baseline_path)
    result = run_attestation(
        args.runs,
        args.timeout,
        baseline=baseline,
        max_robust_cv=args.max_robust_cv,
        max_raw_cv=args.max_raw_cv,
        max_time_regression=args.max_time_regression,
        max_allocation_regression=args.max_allocation_regression,
        max_allocated_bytes_regression=args.max_allocated_bytes_regression,
        max_peak_live_regression=args.max_peak_live_regression,
        max_rss_regression=args.max_rss_regression,
        max_measured_rss_bytes=args.max_measured_rss_bytes,
        execution_control=execution_control,
    )
    output = args.output if args.output.is_absolute() else REPO_ROOT / args.output
    _write_json_atomic(output, result)
    print(output)
    return 2 if result["comparison"]["status"] in {"fail", "invalid"} else 0


if __name__ == "__main__":
    raise SystemExit(main())
