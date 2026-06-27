#!/usr/bin/env python3
"""Measure Molt hello-world output size and startup shape across targets.

The audit is intentionally artifact-centered rather than benchmark-centered:
each row records the exact build command, output artifact, byte size, startup
runner, and skipped-runner reason when a target cannot be launched locally. For
native executables it also measures fresh-path copies because macOS can hide
fixed dyld/code-signature startup tax when repeatedly launching the same path.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import statistics
import sys
from collections.abc import Callable, Iterable
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = ROOT / "tools"
SRC_ROOT = ROOT / "src"
BENCH_RESULTS_ROOT = ROOT / "bench" / "results"
TMP_ROOT = ROOT / "tmp" / "output_startup_size_audit"
WASM_RUNNER = ROOT / "wasm" / "run_wasm.js"

if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools import harness_memory_guard  # noqa: E402
from molt.dx import development_artifact_env  # noqa: E402

DEFAULT_PROBE_SOURCE = 'print("hello world")\n'
DEFAULT_TARGETS = ("native", "wasm", "luau", "mlir")
ALL_TARGETS = ("native", "wasm", "wasm-freestanding", "luau", "mlir")
DEFAULT_BUILD_PROFILES = ("dev", "release")
DEFAULT_NATIVE_BACKENDS = ("auto",)
ALL_NATIVE_BACKENDS = ("auto", "llvm")


@dataclass(frozen=True)
class MatrixCase:
    target: str
    build_profile: str
    backend: str
    stdlib_profile: str | None = "micro"
    wasm_opt_level: str = "Oz"
    linked: bool = False
    require_linked: bool = False

    @property
    def id(self) -> str:
        parts = [self.target, self.build_profile, self.backend]
        if self.target.startswith("wasm"):
            parts.append(self.wasm_opt_level)
        if self.stdlib_profile:
            parts.append(f"stdlib-{self.stdlib_profile}")
        return "-".join(parts).replace("/", "_")


@dataclass(frozen=True)
class BuildResult:
    case: MatrixCase
    command: list[str]
    artifact: Path
    artifacts: dict[str, Path]
    returncode: int
    elapsed_s: float | None
    stdout: str
    stderr: str
    payload: dict[str, Any] | None


def _utc_stamp() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


def _progress(event: str, **fields: Any) -> None:
    payload = {
        "event": f"output_startup_size_audit.{event}",
        "timestamp_utc": _utc_stamp(),
        **fields,
    }
    print(json.dumps(payload, sort_keys=True), file=sys.stderr, flush=True)


def _canonical_env(base: dict[str, str] | None = None) -> dict[str, str]:
    env = development_artifact_env(
        ROOT,
        base or os.environ,
        session_prefix="output-startup-size-audit",
        session_id=(
            (base or os.environ).get("MOLT_SESSION_ID")
            or f"output-startup-size-audit-{os.getpid()}"
        ),
        create_dirs=True,
    )
    env.setdefault("PYTHONHASHSEED", "0")
    env.setdefault("PYTHONUNBUFFERED", "1")
    pythonpath = env.get("PYTHONPATH", "")
    if str(SRC_ROOT) not in pythonpath.split(os.pathsep):
        env["PYTHONPATH"] = (
            f"{SRC_ROOT}{os.pathsep}{pythonpath}" if pythonpath else str(SRC_ROOT)
        )
    return env


def _run_guarded(
    command: list[str],
    *,
    env: dict[str, str],
    timeout: float | None,
    prefix: str = "MOLT_BENCH",
    progress_label: str | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    return harness_memory_guard.guarded_completed_process(
        command,
        prefix=prefix,
        cwd=ROOT,
        env=env,
        capture_output=True,
        timeout=timeout,
        progress_label=progress_label,
    )


def _stats(samples: list[float]) -> dict[str, Any]:
    if not samples:
        return {
            "count": 0,
            "min_s": None,
            "median_s": None,
            "mean_s": None,
            "max_s": None,
            "samples_s": [],
        }
    return {
        "count": len(samples),
        "min_s": round(min(samples), 6),
        "median_s": round(statistics.median(samples), 6),
        "mean_s": round(statistics.fmean(samples), 6),
        "max_s": round(max(samples), 6),
        "samples_s": [round(sample, 6) for sample in samples],
    }


def _ensure_default_probe(path: Path) -> None:
    if path.exists():
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(DEFAULT_PROBE_SOURCE, encoding="utf-8")


def _csv_values(
    raw: str, *, default: Iterable[str], all_values: Iterable[str]
) -> tuple[str, ...]:
    text = raw.strip()
    if not text:
        return tuple(default)
    values: list[str] = []
    for piece in text.split(","):
        value = piece.strip()
        if not value:
            continue
        if value == "all":
            values.extend(all_values)
        else:
            values.append(value)
    deduped: list[str] = []
    for value in values:
        if value not in deduped:
            deduped.append(value)
    return tuple(deduped)


def _iter_matrix_cases(
    *,
    targets: tuple[str, ...],
    build_profiles: tuple[str, ...],
    backends: tuple[str, ...],
    stdlib_profile: str | None,
    wasm_opt_level: str,
) -> list[MatrixCase]:
    cases: list[MatrixCase] = []
    for target in targets:
        if target not in ALL_TARGETS:
            raise ValueError(f"unsupported audit target: {target}")
        for build_profile in build_profiles:
            if build_profile not in {"dev", "release", "dev-release"}:
                raise ValueError(f"unsupported build profile: {build_profile}")
            if target == "native":
                native_backends = ALL_NATIVE_BACKENDS if "all" in backends else backends
                for backend in native_backends:
                    if backend not in ALL_NATIVE_BACKENDS:
                        raise ValueError(f"unsupported native backend: {backend}")
                    cases.append(
                        MatrixCase(
                            target=target,
                            build_profile=build_profile,
                            backend=backend,
                            stdlib_profile=stdlib_profile,
                            wasm_opt_level=wasm_opt_level,
                        )
                    )
                continue
            backend = target
            cases.append(
                MatrixCase(
                    target=target,
                    build_profile=build_profile,
                    backend=backend,
                    stdlib_profile=stdlib_profile,
                    wasm_opt_level=wasm_opt_level,
                    linked=target.startswith("wasm"),
                    require_linked=target.startswith("wasm"),
                )
            )
    return cases


def _json_payload_from_stdout(stdout: str) -> dict[str, Any] | None:
    text = stdout.strip()
    if not text:
        return None
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        for line in reversed(text.splitlines()):
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            break
        else:
            return None
    return payload if isinstance(payload, dict) else None


def _path_from_payload(value: object) -> Path | None:
    if not isinstance(value, str) or not value:
        return None
    path = Path(value)
    return path if path.is_absolute() else ROOT / path


def _artifact_candidates_from_payload(payload: dict[str, Any] | None) -> list[Path]:
    if payload is None:
        return []
    data = payload.get("data")
    if not isinstance(data, dict):
        return []
    candidates: list[Path] = []
    artifacts = data.get("artifacts")
    if isinstance(artifacts, dict):
        for key in (
            "binary",
            "linked_wasm",
            "wasm",
            "luau",
            "object",
            "mlir",
            "rust",
            "app_wasm",
            "runtime_wasm",
        ):
            path = _path_from_payload(artifacts.get(key))
            if path is not None:
                candidates.append(path)
    for key in ("consumer_output", "output", "linked_output", "cwasm_output"):
        path = _path_from_payload(data.get(key))
        if path is not None:
            candidates.append(path)
    deduped: list[Path] = []
    for path in candidates:
        if path not in deduped:
            deduped.append(path)
    return deduped


def _artifact_map_from_payload(payload: dict[str, Any] | None) -> dict[str, Path]:
    if payload is None:
        return {}
    data = payload.get("data")
    if not isinstance(data, dict):
        return {}
    result: dict[str, Path] = {}
    artifacts = data.get("artifacts")
    if isinstance(artifacts, dict):
        for key, value in artifacts.items():
            path = _path_from_payload(value)
            if path is not None:
                result[str(key)] = path
    for key in ("consumer_output", "output", "linked_output", "cwasm_output"):
        path = _path_from_payload(data.get(key))
        if path is not None:
            result[key] = path
    return result


def _fallback_artifact_candidates(
    case: MatrixCase, script: Path, out_dir: Path
) -> list[Path]:
    if case.target == "native":
        return [out_dir / f"{script.stem}_molt"]
    if case.target.startswith("wasm"):
        return [
            out_dir / "output_linked.wasm",
            out_dir / f"{script.stem}_linked.wasm",
            out_dir / "output.wasm",
            *sorted(out_dir.glob("*.wasm")),
        ]
    if case.target == "luau":
        return [*sorted(out_dir.glob("*.luau"))]
    if case.target == "mlir":
        return [*sorted(out_dir.glob("*.mlir"))]
    return [*sorted(path for path in out_dir.iterdir() if path.is_file())]


def _select_artifact(
    case: MatrixCase, script: Path, out_dir: Path, payload: dict[str, Any] | None
) -> Path:
    candidates = [
        *(_artifact_candidates_from_payload(payload)),
        *(_fallback_artifact_candidates(case, script, out_dir)),
    ]
    suffixes = {
        "native": ("",),
        "wasm": (".wasm",),
        "wasm-freestanding": (".wasm",),
        "luau": (".luau",),
        "mlir": (".mlir",),
    }[case.target]
    existing = [path for path in candidates if path.exists() and path.is_file()]
    if case.target == "native":
        executable = [path for path in existing if os.access(path, os.X_OK)]
        if executable:
            return executable[0]
    for path in existing:
        if any(not suffix or path.name.endswith(suffix) for suffix in suffixes):
            return path
    raise FileNotFoundError(
        f"could not find {case.target} output artifact for {script} under {out_dir}"
    )


def _build_molt_artifact(
    *,
    case: MatrixCase,
    script: Path,
    out_dir: Path,
    env: dict[str, str],
    timeout: float,
    extra_molt_args: list[str],
) -> BuildResult:
    out_dir.mkdir(parents=True, exist_ok=True)
    command = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--build-profile",
        case.build_profile,
        "--json",
        "--cache",
        "--out-dir",
        str(out_dir),
    ]
    if case.target == "native":
        command.append("--trusted")
    if case.target != "native":
        command.extend(["--target", case.target])
    if case.target == "native" and case.backend != "auto":
        command.extend(["--backend", case.backend])
    if case.target.startswith("wasm"):
        command.extend(
            ["--linked", "--require-linked", "--wasm-opt-level", case.wasm_opt_level]
        )
    if case.stdlib_profile is not None:
        command.extend(["--stdlib-profile", case.stdlib_profile])
    command.extend(extra_molt_args)
    command.append(str(script))

    _progress(
        "build_start",
        case=case.id,
        target=case.target,
        build_profile=case.build_profile,
        backend=case.backend,
        stdlib_profile=case.stdlib_profile,
        timeout_sec=timeout,
        out_dir=str(out_dir),
    )
    result = _run_guarded(
        command,
        env=env,
        timeout=timeout,
        progress_label=f"output-audit build {case.id}",
    )
    _progress(
        "build_done",
        case=case.id,
        returncode=result.returncode,
        elapsed_s=result.elapsed_s,
        timed_out=bool(getattr(result, "timed_out", False)),
    )
    stdout = result.stdout or ""
    stderr = result.stderr or ""
    payload = _json_payload_from_stdout(stdout)
    artifact = out_dir
    artifacts: dict[str, Path] = _artifact_map_from_payload(payload)
    if result.returncode == 0:
        artifact = _select_artifact(case, script, out_dir, payload)
        artifacts.setdefault("selected", artifact)
    return BuildResult(
        case=case,
        command=command,
        artifact=artifact,
        artifacts=artifacts,
        returncode=result.returncode,
        elapsed_s=result.elapsed_s,
        stdout=stdout,
        stderr=stderr,
        payload=payload,
    )


def _sample_record(
    *,
    index: int,
    command: list[str],
    result: harness_memory_guard.GuardedCompletedProcess,
) -> dict[str, Any]:
    return {
        "index": index,
        "command": command,
        "returncode": result.returncode,
        "elapsed_s": None if result.elapsed_s is None else round(result.elapsed_s, 6),
        "stdout": result.stdout or "",
        "stderr": result.stderr or "",
    }


def _fresh_copy_path(artifact: Path, fresh_dir: Path, index: int) -> Path:
    if artifact.name.endswith("_linked.wasm"):
        stem = artifact.name[: -len("_linked.wasm")]
        return fresh_dir / f"{stem}.fresh-{index}_linked.wasm"
    return fresh_dir / f"{artifact.stem}.fresh-{index}{artifact.suffix}"


RunnerFactory = Callable[[Path, dict[str, str]], tuple[list[str], dict[str, str]]]


def _measure_artifact(
    artifact: Path,
    *,
    samples: int,
    env: dict[str, str],
    timeout: float,
    fresh_copies: bool,
    label: str,
    runner_factory: RunnerFactory,
    executable_copy: bool = False,
) -> dict[str, Any]:
    records: list[dict[str, Any]] = []
    elapsed: list[float] = []
    fresh_dir = artifact.parent / ".fresh_start_samples"
    if fresh_copies:
        fresh_dir.mkdir(parents=True, exist_ok=True)

    for index in range(samples):
        run_path = artifact
        progress_label = f"output-audit startup {label} run {index + 1}/{samples}"
        if fresh_copies:
            run_path = _fresh_copy_path(artifact, fresh_dir, index)
            shutil.copy2(artifact, run_path)
            if executable_copy:
                run_path.chmod(run_path.stat().st_mode | 0o111)
        run_env = env
        try:
            command, env_overrides = runner_factory(run_path, env)
            if env_overrides:
                run_env = {**env, **env_overrides}
            result = _run_guarded(
                command,
                env=run_env,
                timeout=timeout,
                progress_label=progress_label,
            )
        finally:
            if fresh_copies:
                try:
                    run_path.unlink()
                except FileNotFoundError:
                    pass
        records.append(_sample_record(index=index, command=command, result=result))
        if result.returncode == 0 and result.elapsed_s is not None:
            elapsed.append(result.elapsed_s)

    return {
        "label": label,
        "mode": "page_cache_cold_copy" if fresh_copies else "same_path_reuse",
        "ok": len(elapsed) == samples,
        "stats": _stats(elapsed),
        "records": records,
    }


def _measure_executable(
    binary: Path,
    *,
    samples: int,
    env: dict[str, str],
    timeout: float,
    fresh_copies: bool,
    label: str,
) -> dict[str, Any]:
    return _measure_artifact(
        binary,
        samples=samples,
        env=env,
        timeout=timeout,
        fresh_copies=fresh_copies,
        label=label,
        runner_factory=_native_runner_factory(),
        executable_copy=True,
    )


def _resolve_node_binary(env: dict[str, str]) -> str | None:
    requested = env.get("MOLT_NODE_BIN", "").strip()
    if requested:
        return requested if Path(requested).exists() else None
    for candidate in (
        shutil.which("node"),
        "/opt/homebrew/bin/node",
        "/usr/local/bin/node",
    ):
        if candidate and Path(candidate).exists():
            return candidate
    return None


def _node_runner_factory(node_bin: str) -> RunnerFactory:
    def runner(path: Path, _env: dict[str, str]) -> tuple[list[str], dict[str, str]]:
        return (
            [
                node_bin,
                "--no-warnings",
                "--no-wasm-tier-up",
                "--no-wasm-dynamic-tiering",
                "--wasm-num-compilation-tasks=1",
                str(WASM_RUNNER),
                str(path),
            ],
            {
                "NODE_NO_WARNINGS": "1",
                "MOLT_WASM_PREFER_LINKED": "1",
                "MOLT_WASM_LINKED_PATH": str(path),
            },
        )

    return runner


def _lune_runner_factory(lune_bin: str) -> RunnerFactory:
    return lambda path, _env: ([lune_bin, "run", str(path), "--"], {})


def _measure_cold_first_sighting(
    artifact: Path,
    *,
    env: dict[str, str],
    timeout: float,
    label: str,
    runner_factory: RunnerFactory,
) -> dict[str, Any]:
    """Record the single TRUE-cold first run of a freshly built artifact.

    The freshly built artifact has a brand-new cdhash the OS has never seen,
    so its very first execution pays the genuine one-time amfid/provenance tax
    on macOS (and the full page-cache-cold load everywhere). This must run
    BEFORE any other execution of the same bytes, otherwise the cdhash is
    already provenance-warm and the sample is no longer cold. It is therefore a
    single run, recorded ahead of the same-path/page-cache-cold measurements.

    NOTE: this is only genuinely cold when the build did not prime the binary.
    The audit invokes `molt build` without `--prime`, which defaults to off for
    ordinary builds, so the artifact is unprimed when this runs.
    """
    command, env_overrides = runner_factory(artifact, env)
    run_env = {**env, **env_overrides} if env_overrides else env
    result = _run_guarded(
        command,
        env=run_env,
        timeout=timeout,
        progress_label=f"output-audit startup {label} cold-first",
    )
    elapsed = (
        [result.elapsed_s]
        if result.returncode == 0 and result.elapsed_s is not None
        else []
    )
    return {
        "label": label,
        "mode": "cold_first_sighting",
        "ok": len(elapsed) == 1,
        "stats": _stats(elapsed),
        "records": [_sample_record(index=0, command=command, result=result)],
    }


def _native_runner_factory() -> RunnerFactory:
    return lambda path, _env: ([str(path)], {})


def _measure_case_startup(
    case: MatrixCase,
    artifact: Path,
    *,
    samples: int,
    env: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    if case.target == "native":
        native_runner = _native_runner_factory()
        # TRUE-cold first sighting must precede every other run of these bytes.
        cold = _measure_cold_first_sighting(
            artifact,
            env=env,
            timeout=timeout,
            label="molt_cold_first_sighting",
            runner_factory=native_runner,
        )
        return {
            "runner": "native-exec",
            "cold_first_sighting": cold,
            "same_path": _measure_executable(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=False,
                label="molt_same_path",
            ),
            "page_cache_cold": _measure_executable(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=True,
                label="molt_page_cache_cold",
            ),
        }
    if case.target == "wasm":
        if not WASM_RUNNER.exists():
            return {"runner": "node", "skipped": f"missing runner: {WASM_RUNNER}"}
        node_bin = _resolve_node_binary(env)
        if node_bin is None:
            return {"runner": "node", "skipped": "node >=18 not found"}
        runner = _node_runner_factory(node_bin)
        cold = _measure_cold_first_sighting(
            artifact,
            env=env,
            timeout=timeout,
            label="molt_wasm_cold_first_sighting",
            runner_factory=runner,
        )
        return {
            "runner": "node",
            "cold_first_sighting": cold,
            "same_path": _measure_artifact(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=False,
                label="molt_wasm_same_path",
                runner_factory=runner,
            ),
            "page_cache_cold": _measure_artifact(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=True,
                label="molt_wasm_page_cache_cold",
                runner_factory=runner,
            ),
        }
    if case.target == "luau":
        lune_bin = shutil.which("lune")
        if lune_bin is None:
            return {"runner": "lune", "skipped": "lune not found"}
        runner = _lune_runner_factory(lune_bin)
        cold = _measure_cold_first_sighting(
            artifact,
            env=env,
            timeout=timeout,
            label="molt_luau_cold_first_sighting",
            runner_factory=runner,
        )
        return {
            "runner": "lune",
            "cold_first_sighting": cold,
            "same_path": _measure_artifact(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=False,
                label="molt_luau_same_path",
                runner_factory=runner,
            ),
            "page_cache_cold": _measure_artifact(
                artifact,
                samples=samples,
                env=env,
                timeout=timeout,
                fresh_copies=True,
                label="molt_luau_page_cache_cold",
                runner_factory=runner,
            ),
        }
    if case.target == "wasm-freestanding":
        return {
            "runner": None,
            "skipped": "freestanding wasm has no canonical local startup runner",
        }
    if case.target == "mlir":
        return {
            "runner": None,
            "skipped": "MLIR text emission has no canonical local startup runner",
        }
    return {"runner": None, "skipped": f"unsupported startup target: {case.target}"}


def _measure_cpython(
    script: Path,
    *,
    samples: int,
    env: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    records: list[dict[str, Any]] = []
    elapsed: list[float] = []
    interpreter = harness_memory_guard.canonical_interpreter(sys.executable)
    for index in range(samples):
        command = [interpreter, str(script)]
        result = _run_guarded(
            command,
            env=env,
            timeout=timeout,
            prefix="MOLT_BENCH",
            progress_label=f"output-audit cpython run {index + 1}/{samples}",
        )
        records.append(_sample_record(index=index, command=command, result=result))
        if result.returncode == 0 and result.elapsed_s is not None:
            elapsed.append(result.elapsed_s)
    return {
        "label": "cpython",
        "mode": "interpreter_process",
        "ok": len(elapsed) == samples,
        "stats": _stats(elapsed),
        "records": records,
    }


def _measure_c_baseline(
    work_dir: Path,
    *,
    samples: int,
    env: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    cc = shutil.which("cc") or shutil.which("clang")
    if cc is None:
        return {"label": "c_baseline", "ok": False, "skipped": "cc not found"}
    source = work_dir / "c_hello.c"
    binary = work_dir / "c_hello"
    source.write_text(
        '#include <stdio.h>\nint main(void) { puts("hello world"); return 0; }\n',
        encoding="utf-8",
    )
    command = [cc, "-Os", str(source), "-o", str(binary)]
    compile_result = _run_guarded(
        command,
        env=env,
        timeout=timeout,
        progress_label="output-audit c-baseline compile",
    )
    if compile_result.returncode != 0:
        return {
            "label": "c_baseline",
            "ok": False,
            "compile": _sample_record(
                index=0,
                command=command,
                result=compile_result,
            ),
        }
    measured = _measure_executable(
        binary,
        samples=samples,
        env=env,
        timeout=timeout,
        fresh_copies=True,
        label="c_baseline",
    )
    measured["artifact"] = {
        "path": str(binary),
        "bytes": binary.stat().st_size,
    }
    measured["compile"] = _sample_record(
        index=0,
        command=command,
        result=compile_result,
    )
    return measured


def _budget_status(
    *,
    binary_bytes: int,
    fresh_start_stats: dict[str, Any] | None,
    max_artifact_mb: float | None,
    max_fresh_start_ms: float | None,
) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    if max_artifact_mb is not None:
        limit_bytes = int(max_artifact_mb * 1024 * 1024)
        checks.append(
            {
                "name": "artifact_size",
                "limit_bytes": limit_bytes,
                "actual_bytes": binary_bytes,
                "passed": binary_bytes <= limit_bytes,
            }
        )
    if max_fresh_start_ms is not None and fresh_start_stats is not None:
        limit_s = max_fresh_start_ms / 1000.0
        actual = fresh_start_stats.get("median_s")
        checks.append(
            {
                "name": "fresh_start_median",
                "limit_s": limit_s,
                "actual_s": actual,
                "passed": isinstance(actual, int | float) and actual <= limit_s,
            }
        )
    return {
        "checks": checks,
        "passed": all(bool(check["passed"]) for check in checks),
    }


def _startup_fresh_stats(startup: dict[str, Any]) -> dict[str, Any] | None:
    # Budget gates on the multi-sample page-cache-cold metric (median over
    # several copy-based runs), which is stable enough for a hard limit. The
    # single-sample cold_first_sighting is reported for evidence but is too
    # noisy to gate on.
    fresh = startup.get("page_cache_cold")
    return fresh.get("stats") if isinstance(fresh, dict) else None


def _payload_messages(payload: dict[str, Any] | None, key: str) -> list[str]:
    if payload is None:
        return []
    raw = payload.get(key)
    if not isinstance(raw, list):
        return []
    return [str(item) for item in raw]


def _startup_ok(startup: dict[str, Any], *, require_runners: bool) -> bool:
    if startup.get("skipped"):
        return not require_runners
    same = startup.get("same_path")
    fresh = startup.get("page_cache_cold")
    cold = startup.get("cold_first_sighting")
    return bool(
        isinstance(same, dict)
        and isinstance(fresh, dict)
        and isinstance(cold, dict)
        and same.get("ok")
        and fresh.get("ok")
        and cold.get("ok")
    )


def _build_case_row(
    *,
    case: MatrixCase,
    script: Path,
    out_dir: Path,
    env: dict[str, str],
    args: argparse.Namespace,
) -> dict[str, Any]:
    build = _build_molt_artifact(
        case=case,
        script=script,
        out_dir=out_dir,
        env=env,
        timeout=args.build_timeout_sec,
        extra_molt_args=args.molt_arg,
    )
    build_record = {
        "command": build.command,
        "returncode": build.returncode,
        "elapsed_s": build.elapsed_s,
        "stdout": build.stdout,
        "stderr": build.stderr,
        "errors": _payload_messages(build.payload, "errors"),
        "warnings": _payload_messages(build.payload, "warnings"),
    }
    if build.returncode != 0:
        return {
            "case": asdict(case),
            "id": case.id,
            "ok": False,
            "status": "build_failed",
            "build": build_record,
            "artifact": None,
            "artifacts": {},
            "startup": None,
            "budgets": _budget_status(
                binary_bytes=0,
                fresh_start_stats=None,
                max_artifact_mb=None,
                max_fresh_start_ms=None,
            ),
        }

    artifact_bytes = build.artifact.stat().st_size
    startup = _measure_case_startup(
        case,
        build.artifact,
        samples=args.samples,
        env=env,
        timeout=args.run_timeout_sec,
    )
    budgets = _budget_status(
        binary_bytes=artifact_bytes,
        fresh_start_stats=_startup_fresh_stats(startup),
        max_artifact_mb=args.max_artifact_mb,
        max_fresh_start_ms=args.max_fresh_start_ms,
    )
    row_ok = bool(
        _startup_ok(startup, require_runners=args.require_runners) and budgets["passed"]
    )
    status = (
        "ok"
        if row_ok
        else "startup_skipped"
        if startup.get("skipped")
        else "run_failed"
    )
    if not budgets["passed"]:
        status = "budget_failed"
    return {
        "case": asdict(case),
        "id": case.id,
        "ok": row_ok,
        "status": status,
        "build": build_record,
        "artifact": {
            "path": str(build.artifact),
            "bytes": artifact_bytes,
            "kb": round(artifact_bytes / 1024, 3),
            "mb": round(artifact_bytes / 1024 / 1024, 6),
        },
        "artifacts": {
            key: str(value) for key, value in sorted(build.artifacts.items())
        },
        "startup": startup,
        "budgets": budgets,
    }


def _summary(cases: list[dict[str, Any]]) -> dict[str, Any]:
    built = [case for case in cases if case.get("artifact")]
    startup_measured = [
        case
        for case in cases
        if isinstance(case.get("startup"), dict) and not case["startup"].get("skipped")
    ]
    startup_skipped = [
        case
        for case in cases
        if isinstance(case.get("startup"), dict) and case["startup"].get("skipped")
    ]
    return {
        "cases": len(cases),
        "ok_cases": sum(1 for case in cases if case.get("ok")),
        "built_cases": len(built),
        "startup_measured_cases": len(startup_measured),
        "startup_skipped_cases": len(startup_skipped),
        "build_failed_cases": sum(
            1 for case in cases if case.get("status") == "build_failed"
        ),
        "budget_failed_cases": sum(
            1 for case in cases if case.get("status") == "budget_failed"
        ),
    }


def build_report(args: argparse.Namespace) -> dict[str, Any]:
    stamp = _utc_stamp()
    work_dir = args.work_dir or (TMP_ROOT / stamp)
    work_dir.mkdir(parents=True, exist_ok=True)
    script = args.script or (work_dir / "hello_world.py")
    _ensure_default_probe(script)

    env = _canonical_env()
    targets = _csv_values(args.targets, default=DEFAULT_TARGETS, all_values=ALL_TARGETS)
    profiles = _csv_values(
        args.build_profiles,
        default=DEFAULT_BUILD_PROFILES,
        all_values=("dev", "release", "dev-release"),
    )
    backends = _csv_values(
        args.backends,
        default=DEFAULT_NATIVE_BACKENDS,
        all_values=ALL_NATIVE_BACKENDS,
    )
    cases = _iter_matrix_cases(
        targets=targets,
        build_profiles=profiles,
        backends=backends,
        stdlib_profile=args.stdlib_profile,
        wasm_opt_level=args.wasm_opt_level,
    )
    rows: list[dict[str, Any]] = []
    for case in cases:
        out_dir = (args.out_dir or work_dir / "outputs") / case.id
        _progress("case_start", case=case.id)
        row = _build_case_row(
            case=case,
            script=script,
            out_dir=out_dir,
            env=env,
            args=args,
        )
        rows.append(row)
        _progress(
            "case_done",
            case=case.id,
            ok=bool(row.get("ok")),
            status=str(row.get("status")),
        )

    _progress("baseline_start", baseline="cpython", enabled=not args.no_cpython_baseline)
    cpython = (
        None
        if args.no_cpython_baseline
        else _measure_cpython(
            script,
            samples=args.samples,
            env=env,
            timeout=args.run_timeout_sec,
        )
    )
    _progress("baseline_done", baseline="cpython", measured=cpython is not None)
    _progress("baseline_start", baseline="c", enabled=not args.no_c_baseline)
    c_baseline = (
        None
        if args.no_c_baseline
        else _measure_c_baseline(
            work_dir,
            samples=args.samples,
            env=env,
            timeout=args.run_timeout_sec,
        )
    )
    _progress("baseline_done", baseline="c", measured=c_baseline is not None)
    summary = _summary(rows)
    strict_ok = all(bool(row.get("ok")) for row in rows)
    return {
        "schema_version": "2.0",
        "event": "output_startup_size_audit",
        "recorded_at": stamp,
        "ok": strict_ok,
        "strict_ok": strict_ok,
        "repo_root": str(ROOT),
        "system": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
        },
        "script": str(script),
        "probe_source": script.read_text(encoding="utf-8"),
        "config": {
            "targets": list(targets),
            "build_profiles": list(profiles),
            "backends": list(backends),
            "stdlib_profile": args.stdlib_profile,
            "wasm_opt_level": args.wasm_opt_level,
            "samples": args.samples,
            "require_runners": args.require_runners,
            "strict": args.strict,
        },
        "summary": summary,
        "cases": rows,
        "baselines": {
            "cpython": cpython,
            "c": c_baseline,
        },
    }


def _format_optional_seconds(value: object) -> str:
    if isinstance(value, int | float):
        return f"{value:.3f}s"
    return "n/a"


def _case_startup_median(row: dict[str, Any], key: str) -> str:
    startup = row.get("startup")
    if not isinstance(startup, dict):
        return "n/a"
    item = startup.get(key)
    if not isinstance(item, dict):
        return "n/a"
    stats = item.get("stats")
    if not isinstance(stats, dict):
        return "n/a"
    return _format_optional_seconds(stats.get("median_s"))


def format_report(report: dict[str, Any]) -> str:
    summary = report["summary"]
    lines = [
        "Output startup/size audit:",
        (
            "  cases: "
            f"{summary['ok_cases']}/{summary['cases']} ok, "
            f"{summary['built_cases']} built, "
            f"{summary['startup_measured_cases']} startup-measured, "
            f"{summary['startup_skipped_cases']} startup-skipped"
        ),
    ]
    for row in report["cases"]:
        case = row["case"]
        label = f"{case['target']}/{case['build_profile']}/{case['backend']}"
        artifact = row.get("artifact")
        if not artifact:
            build = row.get("build", {})
            lines.append(
                f"  {label}: {row.get('status')} rc={build.get('returncode')} "
                f"build={_format_optional_seconds(build.get('elapsed_s'))}"
            )
            continue
        size = f"{artifact['bytes']} bytes ({artifact['mb']:.2f} MB)"
        startup = row.get("startup") or {}
        skipped = startup.get("skipped") if isinstance(startup, dict) else None
        if skipped:
            lines.append(f"  {label}: {size}; startup skipped: {skipped}")
        else:
            same = _case_startup_median(row, "same_path")
            page_cold = _case_startup_median(row, "page_cache_cold")
            cold = _case_startup_median(row, "cold_first_sighting")
            lines.append(
                f"  {label}: {size}; same={same}; "
                f"page_cache_cold={page_cold}; cold_first_sighting={cold}"
            )
    cpython = report.get("baselines", {}).get("cpython")
    if cpython is not None:
        lines.append(
            "  CPython process median: "
            f"{_format_optional_seconds(cpython['stats'].get('median_s'))}"
        )
    c_baseline = report.get("baselines", {}).get("c")
    if c_baseline is not None and c_baseline.get("ok"):
        lines.append(
            "  C fresh-path median: "
            f"{_format_optional_seconds(c_baseline['stats'].get('median_s'))}"
        )
    failed = [
        f"{row['id']}={row['status']}" for row in report["cases"] if not row.get("ok")
    ]
    if failed:
        lines.append("  non-ok rows: " + ", ".join(failed))
    return "\n".join(lines)


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Build a hello-world Molt output matrix and measure artifact size "
            "plus same-path and fresh-path startup where a canonical runner exists."
        )
    )
    parser.add_argument("--script", type=Path, default=None)
    parser.add_argument(
        "--targets",
        default=",".join(DEFAULT_TARGETS),
        help=(
            "Comma-separated targets: native, wasm, wasm-freestanding, luau, "
            "mlir, or all."
        ),
    )
    parser.add_argument(
        "--build-profiles",
        default=",".join(DEFAULT_BUILD_PROFILES),
        help="Comma-separated build profiles: dev, release, dev-release, or all.",
    )
    parser.add_argument(
        "--build-profile",
        choices=("dev", "release", "dev-release"),
        default=None,
        help="Compatibility alias for a single --build-profiles value.",
    )
    parser.add_argument(
        "--backends",
        default=",".join(DEFAULT_NATIVE_BACKENDS),
        help="Comma-separated native backends: auto, llvm, or all.",
    )
    parser.add_argument(
        "--stdlib-profile",
        choices=("full", "micro"),
        default="micro",
        help="Runtime stdlib profile passed to `molt build`.",
    )
    parser.add_argument(
        "--wasm-opt-level",
        choices=("Oz", "O3"),
        default="Oz",
        help="Linked WASM optimization level.",
    )
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--build-timeout-sec", type=float, default=900.0)
    parser.add_argument("--run-timeout-sec", type=float, default=10.0)
    parser.add_argument("--work-dir", type=Path, default=None)
    parser.add_argument("--out-dir", type=Path, default=None)
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help=(
            "Output JSON path. Defaults to bench/results/"
            "output_startup_size_audit_<timestamp>.json."
        ),
    )
    parser.add_argument(
        "--max-artifact-mb",
        "--max-binary-mb",
        dest="max_artifact_mb",
        type=float,
        default=None,
    )
    parser.add_argument("--max-fresh-start-ms", type=float, default=None)
    parser.add_argument("--require-runners", action="store_true")
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Exit nonzero when any requested row is non-ok.",
    )
    parser.add_argument("--no-c-baseline", action="store_true")
    parser.add_argument("--no-cpython-baseline", action="store_true")
    parser.add_argument(
        "--molt-arg",
        action="append",
        default=[],
        help="Additional argument to pass through to `molt build`; repeatable.",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if args.build_profile is not None:
        args.build_profiles = args.build_profile
    return args


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(argv)
    args.samples = max(1, args.samples)
    report = build_report(args)
    json_out = args.json_out or (
        BENCH_RESULTS_ROOT
        / f"output_startup_size_audit_{report.get('recorded_at', _utc_stamp())}.json"
    )
    json_out.parent.mkdir(parents=True, exist_ok=True)
    json_out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(format_report(report))
        print(f"  artifact: {json_out}")
    return 1 if args.strict and not report.get("strict_ok") else 0


if __name__ == "__main__":
    raise SystemExit(main())
