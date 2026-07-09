#!/usr/bin/env python3
"""DX build-timing harness for the build-throughput arc (foundation/08_DX-buildspeed.md).

Measures wall-clock for the canonical backend-daemon build in well-defined
scenarios:
  - cold      : clean target dir, full build
  - inc-<file>: touch ONE file then rebuild (incremental)
  - test-lib  : `cargo test --lib --no-run` compile time after a touch

The daemon build profile and the lib-test profile are separate authorities:
`release-fast` is optimized for the compiler daemon binary, while `dev-fast`
is the low-latency Rust proof profile. Keeping them separate prevents the
diagnostic harness from turning a proof-timing scenario into an optimized
release test build.

It drives `cargo` directly (NOT `molt build`) because the thing being optimised
is the cargo build of the backend crate(s) themselves. Each scenario is run N
times; we report min/median/max so noise from other agents is visible.
Synthetic source touches are restored and then repaired with an unmeasured
baseline build so the persistent target is not left stale for the next scenario,
the next queue row, or the next agent.

This tool never runs a compiled Molt binary, but every Cargo child still routes
through the shared memory guard because build throughput work must not bypass
process-tree custody.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict
import hashlib
import json
import os
import shutil
import subprocess
import statistics
import sys
import tempfile
import time
from pathlib import Path
from typing import Mapping

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools.throughput_measurement import elapsed_sec, phase_result  # noqa: E402

TOUCH_MARKER = b"\n// dx_build_timer touch\n"
PYTHON_TOUCH_MARKER = "\n__molt_dx_build_timer_edit__ = 1\n"

MOLT_BUILD_SCENARIO_PREFIX = "molt-build-"
MOLT_BUILD_TARGETS = frozenset({"native", "wasm", "wasm-split"})


def _now() -> float:
    return time.perf_counter()


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class TouchJournal:
    """Crash-recoverable source touch journal for incremental build probes."""

    def __init__(self, path: Path):
        self.path = path

    def recover(self) -> None:
        entries = self._read()
        if not entries:
            return
        remaining = []
        for entry in entries:
            source = Path(entry["path"])
            if not source.exists():
                remaining.append(entry)
                continue
            current = source.read_bytes()
            current_sha = _sha256(current)
            if current_sha == entry["touched_sha256"]:
                source.write_bytes(current[: -len(TOUCH_MARKER)])
                continue
            if current_sha != entry["original_sha256"]:
                remaining.append(entry)
        self._write(remaining)

    def touch(self, source: Path) -> dict[str, str]:
        """Append a marker and persist enough state to recover after a crash.

        A real content edit is the honest model of what an agent edit does: it
        invalidates Cargo's fingerprint and the incremental compilation cache for
        that file's codegen unit. The journal is written before the source edit,
        so an interrupted harness can be recovered on the next run without
        guessing or overwriting unrelated user edits.
        """
        self.recover()
        original = source.read_bytes()
        if original.endswith(TOUCH_MARKER):
            raise RuntimeError(f"{source} already ends with dx_build_timer marker")
        touched = original + TOUCH_MARKER
        entry = {
            "path": str(source),
            "original_sha256": _sha256(original),
            "touched_sha256": _sha256(touched),
        }
        entries = [e for e in self._read() if e.get("path") != entry["path"]]
        entries.append(entry)
        self._write(entries)
        source.write_bytes(touched)
        return entry

    def restore(self, entry: dict[str, str]) -> None:
        source = Path(entry["path"])
        current = source.read_bytes()
        current_sha = _sha256(current)
        if current_sha == entry["touched_sha256"]:
            source.write_bytes(current[: -len(TOUCH_MARKER)])
        elif current_sha != entry["original_sha256"]:
            raise RuntimeError(
                f"refusing to restore {source}: content changed outside dx_build_timer"
            )
        entries = [e for e in self._read() if e.get("path") != entry["path"]]
        self._write(entries)

    def _read(self) -> list[dict[str, str]]:
        if not self.path.exists():
            return []
        raw = json.loads(self.path.read_text(encoding="utf-8"))
        entries = raw.get("entries", [])
        if not isinstance(entries, list):
            raise RuntimeError(f"invalid dx_build_timer touch journal: {self.path}")
        return entries

    def _write(self, entries: list[dict[str, str]]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if entries:
            payload = json.dumps({"entries": entries}, indent=2) + "\n"
            tmp = self.path.with_suffix(self.path.suffix + ".tmp")
            tmp.write_text(payload, encoding="utf-8")
            tmp.replace(self.path)
        elif self.path.exists():
            self.path.unlink()


def _output_text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def _run_completed(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
    *,
    progress_label: str | None = None,
    prefer_outer_guard_when_active: bool = False,
) -> tuple[harness_memory_guard.GuardedCompletedProcess, float]:
    if prefer_outer_guard_when_active and _outer_memory_guard_reuse_enabled(env):
        return _run_completed_inside_active_guard(
            cmd,
            env,
            cwd,
            progress_label=progress_label,
        )
    start = _now()
    proc = harness_memory_guard.guarded_completed_process(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        prefix="MOLT_DX_BUILD",
        progress_label=progress_label,
    )
    elapsed = proc.elapsed_s if proc.elapsed_s is not None else _now() - start
    return proc, elapsed


def _outer_memory_guard_active(env: Mapping[str, str]) -> bool:
    raw = env.get("MOLT_MEMORY_GUARD_ACTIVE") or os.environ.get(
        "MOLT_MEMORY_GUARD_ACTIVE",
        "",
    )
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _outer_memory_guard_reuse_enabled(env: Mapping[str, str]) -> bool:
    if not _outer_memory_guard_active(env):
        return False
    explicit = env.get("MOLT_DX_BUILD_TIMER_REUSE_OUTER_GUARD", "")
    if explicit.strip().lower() in {"1", "true", "yes", "on"}:
        return True
    proof_queue = env.get("MOLT_PROOF_QUEUE") or os.environ.get("MOLT_PROOF_QUEUE", "")
    return proof_queue.strip().lower() in {"1", "true", "yes", "on"}


def _run_completed_inside_active_guard(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
    *,
    progress_label: str | None = None,
) -> tuple[harness_memory_guard.GuardedCompletedProcess, float]:
    """Run a child directly when an outer memory guard already owns the suite.

    `molt build` starts a warm backend daemon for reuse across build commands.
    Wrapping every phase in a nested guard puts each phase in its own kill-on-close
    job on Windows, which turns cold/warm/edit timing into forced-cold daemon
    timing and emits orphan cleanup warnings. In proof-queue rows the outer guard
    already owns this whole tool process and its descendants, so direct children
    keep custody while letting intentional warm daemons live until suite exit.
    """

    start = _now()
    interval = (
        harness_memory_guard._subprocess_keepalive_interval_secs(  # noqa: SLF001
            env,
            prefix="MOLT_DX_BUILD",
        )
        if progress_label is not None
        else None
    )
    next_keepalive = start + interval if interval is not None else None
    with tempfile.TemporaryFile(
        mode="w+t",
        encoding="utf-8",
        errors="replace",
    ) as stdout_tmp, tempfile.TemporaryFile(
        mode="w+t",
        encoding="utf-8",
        errors="replace",
    ) as stderr_tmp:
        proc = subprocess.Popen(
            cmd,
            cwd=cwd,
            env=env,
            stdout=stdout_tmp,
            stderr=stderr_tmp,
            text=True,
        )
        while proc.poll() is None:
            now = _now()
            if next_keepalive is not None and now >= next_keepalive:
                elapsed = int(now - start)
                print(
                    (
                        f"{progress_label}: still running elapsed={elapsed}s "
                        f"timeout=unbounded pid={proc.pid}"
                    ),
                    flush=True,
                )
                next_keepalive = now + interval  # type: ignore[operator]
            time.sleep(0.1)
        elapsed = _now() - start
        stdout_tmp.seek(0)
        stderr_tmp.seek(0)
        completed = harness_memory_guard.GuardedCompletedProcess(
            cmd,
            proc.returncode,
            stdout_tmp.read(),
            stderr_tmp.read(),
            elapsed_s=elapsed,
        )
    return completed, elapsed


def _run(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
    *,
    progress_label: str | None = None,
) -> tuple[int, float, str]:
    proc, elapsed = _run_completed(cmd, env, cwd, progress_label=progress_label)
    tail = "\n".join(_output_text(proc.stderr).splitlines()[-8:])
    return proc.returncode, elapsed, tail


def _build_cmd(args: argparse.Namespace, extra: list[str] | None = None) -> list[str]:
    cmd = [
        "cargo",
        "build",
        "--profile",
        args.profile,
        "-p",
        args.package,
        "--bin",
        args.bin_name,
        "--features",
        args.features,
    ]
    if extra:
        cmd.extend(extra)
    return cmd


def _test_build_cmd(args: argparse.Namespace) -> list[str]:
    return [
        "cargo",
        "test",
        "--profile",
        args.test_profile,
        "-p",
        args.package,
        "--features",
        args.features,
        "--lib",
        "--no-run",
    ]


def _is_molt_build_scenario(scenario: str) -> bool:
    return scenario.startswith(MOLT_BUILD_SCENARIO_PREFIX)


def _is_cargo_scenario(scenario: str) -> bool:
    return scenario == "cold" or scenario == "test-lib" or scenario.startswith("inc-")


def _molt_build_target_for_scenario(scenario: str) -> str | None:
    if not _is_molt_build_scenario(scenario):
        return None
    target = scenario[len(MOLT_BUILD_SCENARIO_PREFIX) :]
    return target if target in MOLT_BUILD_TARGETS else None


def _molt_build_output_root(args: argparse.Namespace) -> Path:
    configured = getattr(args, "molt_output_root", None)
    if configured:
        return Path(configured).expanduser().resolve()
    json_out = getattr(args, "json_out", None)
    if json_out:
        json_path = Path(json_out)
        return (
            json_path.with_name(f"{json_path.stem}.molt-builds")
            .expanduser()
            .resolve()
        )
    return (Path(args.target_dir) / "dx_molt_builds").expanduser().resolve()


def _molt_build_python_executable(env: Mapping[str, str] | None = None) -> str:
    environment = os.environ if env is None else env
    explicit = environment.get("MOLT_BUILD_PYTHON")
    if explicit:
        return explicit
    for root_name in ("UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        root = environment.get(root_name)
        if not root:
            continue
        env_root = Path(root)
        for relative in (
            Path("Scripts") / "python.exe",
            Path("Scripts") / "python",
            Path("bin") / "python3",
            Path("bin") / "python",
        ):
            candidate = env_root / relative
            if candidate.exists():
                return str(candidate)
    return sys.executable


def _drain_current_session_backend_daemons(env: Mapping[str, str]) -> int:
    """Terminate only backend daemons proven by this dx session's identity files."""

    try:
        from molt import backend_daemon_custody
    except ImportError:
        return 0
    terminated = backend_daemon_custody.terminate_backend_daemons_for_session(
        env,
        project_root=REPO_ROOT,
        grace=1.0,
    )
    return len(terminated)


def _molt_build_command(
    *,
    source: Path,
    target: str,
    profile: str,
    out_dir: Path,
    diagnostics_file: Path,
    python_executable: str | None = None,
) -> list[str]:
    cli_target = "wasm" if target == "wasm-split" else target
    cmd = [
        python_executable or _molt_build_python_executable(),
        "-m",
        "molt.cli",
        "build",
        str(source),
        "--target",
        cli_target,
        "--profile",
        profile,
        "--out-dir",
        str(out_dir),
        "--cache-report",
        "--diagnostics",
        "--diagnostics-file",
        str(diagnostics_file),
    ]
    if target == "wasm-split":
        cmd.append("--split-runtime")
    return cmd


def _apply_python_edit(source: Path) -> str:
    with source.open("a", encoding="utf-8") as handle:
        handle.write(PYTHON_TOUCH_MARKER)
    return PYTHON_TOUCH_MARKER.strip()


def _touch_files(repo_root: Path = REPO_ROOT) -> dict[str, Path]:
    return {
        "value_range": repo_root
        / "runtime/molt-passes/src/tir/passes/value_range/mod.rs",
        "function_compiler": repo_root
        / "runtime/molt-backend-native/src/native_backend/function_compiler.rs",
        "modules": repo_root / "runtime/molt-runtime/src/builtins/modules.rs",
        "gvn": repo_root / "runtime/molt-passes/src/tir/passes/gvn.rs",
    }


def _scenario_preflight_errors(
    scenarios: list[str],
    touch_files: dict[str, Path],
) -> list[str]:
    errors: list[str] = []
    for scenario in scenarios:
        if scenario == "cold":
            continue
        if _is_molt_build_scenario(scenario):
            target = _molt_build_target_for_scenario(scenario)
            if target is None:
                choices = ", ".join(sorted(MOLT_BUILD_TARGETS))
                errors.append(
                    f"scenario={scenario} unknown molt build target; choices={choices}"
                )
            continue
        if scenario == "test-lib":
            key = "value_range"
        elif scenario.startswith("inc-"):
            key = scenario[len("inc-") :]
        else:
            errors.append(f"unknown scenario: {scenario}")
            continue
        path = touch_files.get(key)
        if path is None:
            errors.append(f"scenario={scenario} unknown touch key: {key}")
        elif not path.exists():
            errors.append(
                f"scenario={scenario} touch_key={key} missing touch_path={path}"
            )
    return errors


def _snapshot_payload(
    args: argparse.Namespace,
    results: dict[str, dict],
    *,
    cargo_version: str,
    prime: dict[str, object] | None = None,
    active: dict[str, object] | None = None,
) -> dict[str, object]:
    payload: dict[str, object] = {
        "meta": {
            "profile": args.profile,
            "test_profile": args.test_profile,
            "package": args.package,
            "bin": args.bin_name,
            "features": args.features,
            "runs": args.runs,
            "target_dir": args.target_dir,
            "molt_source": getattr(args, "molt_source", None),
            "molt_profile": getattr(args, "molt_profile", None),
            "molt_output_root": getattr(args, "molt_output_root", None),
            "molt_output_root_resolved": str(_molt_build_output_root(args)),
            "cargo": cargo_version,
        },
        "results": results,
    }
    if prime is not None:
        payload["prime"] = prime
    if active is not None:
        payload["active"] = active
    return payload


def _write_snapshot(
    args: argparse.Namespace,
    results: dict[str, dict],
    *,
    cargo_version: str,
    prime: dict[str, object] | None = None,
    active: dict[str, object] | None = None,
) -> None:
    if not args.json_out:
        return
    path = Path(args.json_out)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(
        json.dumps(
            _snapshot_payload(
                args,
                results,
                cargo_version=cargo_version,
                prime=prime,
                active=active,
            ),
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    tmp.replace(path)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default="release-fast")
    ap.add_argument(
        "--test-profile",
        default="dev-fast",
        help=(
            "Cargo profile for test-lib scenarios. Defaults to dev-fast because "
            "release-fast is the daemon iteration profile, not the Rust proof "
            "latency profile."
        ),
    )
    ap.add_argument("--package", default="molt-backend")
    ap.add_argument("--bin", dest="bin_name", default="molt-backend")
    ap.add_argument("--features", default="native-backend")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--target-dir", required=True, help="CARGO_TARGET_DIR to use")
    ap.add_argument(
        "--molt-source",
        default="examples/hello.py",
        help=(
            "Source file for real `molt build` scenarios "
            "(molt-build-native, molt-build-wasm, molt-build-wasm-split)."
        ),
    )
    ap.add_argument(
        "--molt-profile",
        default="dev",
        help="Profile passed to real `molt build` scenarios (default: dev).",
    )
    ap.add_argument(
        "--molt-output-root",
        default=None,
        help=(
            "Output root for real `molt build` scenarios "
            "(default: <json-out-stem>.molt-builds when --json-out is set, "
            "otherwise <target-dir>/dx_molt_builds)."
        ),
    )
    ap.add_argument(
        "--scenarios",
        nargs="+",
        default=[
            "cold",
            "inc-value_range",
            "inc-gvn",
            "inc-function_compiler",
            "inc-modules",
            "test-lib",
        ],
    )
    ap.add_argument("--json-out", default=None)
    ap.add_argument(
        "--cold-clean",
        action="store_true",
        help="rm -rf target dir before the cold scenario (true cold)",
    )
    ap.add_argument(
        "--no-repair-after-touch",
        action="store_true",
        help=(
            "restore touched files without rebuilding the baseline target; "
            "diagnostic only, leaves Cargo artifacts stale until the next build"
        ),
    )
    args = ap.parse_args()

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = args.target_dir
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", args.target_dir)
    touch_journal = TouchJournal(Path(args.target_dir) / ".dx_build_timer_touches.json")
    touch_journal.recover()

    touch_files = _touch_files(REPO_ROOT)
    preflight_errors = _scenario_preflight_errors(args.scenarios, touch_files)
    molt_source = Path(args.molt_source)
    if not molt_source.is_absolute():
        molt_source = (REPO_ROOT / molt_source).resolve()
    if any(_is_molt_build_scenario(scenario) for scenario in args.scenarios):
        if not molt_source.is_file():
            preflight_errors.append(f"molt_source missing: {molt_source}")
    if preflight_errors:
        print("dx_build_timer scenario preflight failed:", file=sys.stderr)
        for error in preflight_errors:
            print(f"  - {error}", file=sys.stderr)
        return 2

    results: dict[str, dict] = {}
    cargo_version = _output_text(
        _run_completed(["cargo", "--version"], env, REPO_ROOT)[0].stdout
    ).strip()
    prime: dict[str, object] | None = None

    def measure(label: str, prep, cmd: list[str]) -> None:
        samples = []
        repair_samples = []
        rc_last = 0
        tail_last = ""
        repair_rc_last = 0
        repair_tail_last = ""
        repair_cmd = _build_cmd(args)
        for i in range(args.runs):
            touch_entry = None
            if prep:
                touch_entry = prep()
            _write_snapshot(
                args,
                results,
                cargo_version=cargo_version,
                prime=prime,
                active={
                    "label": label,
                    "run": i + 1,
                    "runs": args.runs,
                    "cmd": cmd,
                    "touch": touch_entry,
                    "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                },
            )
            try:
                rc, elapsed, tail = _run(
                    cmd,
                    env,
                    REPO_ROOT,
                    progress_label=f"dx-build {label} run {i + 1}/{args.runs}",
                )
            finally:
                if touch_entry is not None:
                    touch_journal.restore(touch_entry)
            repair_elapsed = None
            repair_tail = ""
            repair_rc = 0
            if touch_entry is not None and not args.no_repair_after_touch:
                _write_snapshot(
                    args,
                    results,
                    cargo_version=cargo_version,
                    prime=prime,
                    active={
                        "label": label,
                        "phase": "repair",
                        "run": i + 1,
                        "runs": args.runs,
                        "cmd": repair_cmd,
                        "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    },
                )
                repair_rc, repair_elapsed, repair_tail = _run(
                    repair_cmd,
                    env,
                    REPO_ROOT,
                    progress_label=f"dx-build {label} repair {i + 1}/{args.runs}",
                )
                repair_samples.append(round(repair_elapsed, 2))
                repair_rc_last = repair_rc
                repair_tail_last = repair_tail
            rc_last, tail_last = rc, tail
            samples.append(round(elapsed, 2))
            print(
                f"  [{label}] run {i + 1}/{args.runs}: {elapsed:.2f}s rc={rc}",
                flush=True,
            )
            if repair_elapsed is not None:
                print(
                    f"  [{label}] repair {i + 1}/{args.runs}: "
                    f"{repair_elapsed:.2f}s rc={repair_rc}",
                    flush=True,
                )
            results[label] = {
                "samples_sec": samples,
                "min": min(samples) if samples else None,
                "median": round(statistics.median(samples), 2) if samples else None,
                "max": max(samples) if samples else None,
                "rc": rc_last,
                "cmd": cmd,
                "stderr_tail": tail_last if rc_last != 0 else "",
                "repair_samples_sec": repair_samples,
                "repair_rc": repair_rc_last,
                "repair_cmd": repair_cmd if repair_samples else None,
                "repair_stderr_tail": (
                    repair_tail_last if repair_rc_last != 0 else ""
                ),
            }
            _write_snapshot(
                args,
                results,
                cargo_version=cargo_version,
                prime=prime,
            )
            if rc != 0 or repair_rc != 0:
                if rc != 0:
                    print(f"    FAILED:\n{tail}", flush=True)
                if repair_rc != 0:
                    print(f"    REPAIR FAILED:\n{repair_tail}", flush=True)
                break
        results.setdefault(
            label,
            {
                "samples_sec": samples,
                "min": min(samples) if samples else None,
                "median": round(statistics.median(samples), 2) if samples else None,
                "max": max(samples) if samples else None,
                "rc": rc_last,
                "cmd": cmd,
                "stderr_tail": tail_last if rc_last != 0 else "",
                "repair_samples_sec": repair_samples,
                "repair_rc": repair_rc_last,
                "repair_cmd": repair_cmd if repair_samples else None,
                "repair_stderr_tail": (
                    repair_tail_last if repair_rc_last != 0 else ""
                ),
            },
        )

    def measure_molt_build(label: str, target: str) -> None:
        source_name = molt_source.name
        phases = []
        molt_build_python = _molt_build_python_executable(env)
        try:
            for i in range(args.runs):
                run_root = _molt_build_output_root(args) / label / f"run-{i + 1}"
                if run_root.exists():
                    shutil.rmtree(run_root)
                work_root = run_root / "work"
                out_dir = run_root / "out"
                diagnostics_root = run_root / "diagnostics"
                work_root.mkdir(parents=True, exist_ok=True)
                out_dir.mkdir(parents=True, exist_ok=True)
                diagnostics_root.mkdir(parents=True, exist_ok=True)
                source_copy = work_root / source_name
                shutil.copy2(molt_source, source_copy)

                for phase in ("cold", "warm", "edit"):
                    if phase == "edit":
                        _apply_python_edit(source_copy)
                    diagnostics_file = diagnostics_root / f"{phase}.json"
                    cmd = _molt_build_command(
                        source=source_copy,
                        target=target,
                        profile=args.molt_profile,
                        out_dir=out_dir,
                        diagnostics_file=diagnostics_file,
                        python_executable=molt_build_python,
                    )
                    _write_snapshot(
                        args,
                        results,
                        cargo_version=cargo_version,
                        prime=prime,
                        active={
                            "label": label,
                            "phase": phase,
                            "run": i + 1,
                            "runs": args.runs,
                            "cmd": cmd,
                            "started_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                        },
                    )
                    proc, raw_elapsed = _run_completed(
                        cmd,
                        env,
                        REPO_ROOT,
                        progress_label=(
                            f"dx-build {label} {phase} run {i + 1}/{args.runs}"
                        ),
                        prefer_outer_guard_when_active=True,
                    )
                    timed_out = (
                        proc.returncode
                        == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
                    )
                    phase_payload = asdict(
                        phase_result(
                            phase=phase,
                            command=cmd,
                            cwd=REPO_ROOT,
                            returncode=proc.returncode,
                            elapsed=elapsed_sec(0.0, raw_elapsed),
                            timed_out=timed_out,
                            stdout=_output_text(proc.stdout),
                            stderr=_output_text(proc.stderr),
                            output_path=out_dir,
                        )
                    )
                    phase_payload["diagnostics_file"] = str(diagnostics_file)
                    phase_payload["source"] = str(source_copy)
                    phase_payload["run"] = i + 1
                    phases.append(phase_payload)
                    print(
                        (
                            f"  [{label}] {phase} run {i + 1}/{args.runs}: "
                            f"{raw_elapsed:.2f}s rc={proc.returncode}"
                        ),
                        flush=True,
                    )
                    results[label] = {
                        "target": target,
                        "profile": args.molt_profile,
                        "source": str(molt_source),
                        "phases": phases,
                    }
                    _write_snapshot(
                        args,
                        results,
                        cargo_version=cargo_version,
                        prime=prime,
                    )
                    if proc.returncode != 0:
                        break
                if phases and phases[-1].get("returncode") != 0:
                    break
        finally:
            drained = _drain_current_session_backend_daemons(env)
            results.setdefault(
                label,
                {
                    "target": target,
                    "profile": args.molt_profile,
                    "source": str(molt_source),
                    "phases": phases,
                },
            )
            results[label]["backend_daemons_drained"] = drained
            if drained:
                print(
                    f"  [{label}] drained {drained} current-session backend daemon(s)",
                    flush=True,
                )
            _write_snapshot(args, results, cargo_version=cargo_version, prime=prime)

    if any(_is_cargo_scenario(scenario) for scenario in args.scenarios):
        # Ensure a warm baseline build exists first (so incremental scenarios are real).
        print("[dx] priming warm build ...", flush=True)
        rc, elapsed, tail = _run(
            _build_cmd(args),
            env,
            REPO_ROOT,
            progress_label="dx-build prime",
        )
        print(f"[dx] prime build: {elapsed:.2f}s rc={rc}", flush=True)
        prime = {
            "elapsed_sec": round(elapsed, 2),
            "rc": rc,
            "cmd": _build_cmd(args),
            "stderr_tail": tail if rc != 0 else "",
        }
        _write_snapshot(args, results, cargo_version=cargo_version, prime=prime)
        if rc != 0:
            print(f"[dx] prime FAILED:\n{tail}", file=sys.stderr)
            return 1

    for scen in args.scenarios:
        if scen == "cold":

            def cold_prep():
                if args.cold_clean:
                    td = Path(args.target_dir)
                    if td.exists():
                        shutil.rmtree(td)

            measure("cold", cold_prep, _build_cmd(args))
        elif scen.startswith("inc-"):
            key = scen[len("inc-") :]
            f = touch_files[key]
            measure(scen, (lambda f=f: touch_journal.touch(f)), _build_cmd(args))
        elif scen == "test-lib":
            measure(
                "test-lib",
                (lambda: touch_journal.touch(touch_files["value_range"])),
                _test_build_cmd(args),
            )
        elif _is_molt_build_scenario(scen):
            target = _molt_build_target_for_scenario(scen)
            if target is None:
                print(f"unknown scenario: {scen}", file=sys.stderr)
                continue
            measure_molt_build(scen, target)
        else:
            print(f"unknown scenario: {scen}", file=sys.stderr)

    payload = _snapshot_payload(args, results, cargo_version=cargo_version, prime=prime)
    out = json.dumps(payload, indent=2)
    if args.json_out:
        _write_snapshot(args, results, cargo_version=cargo_version, prime=prime)
        print(f"wrote {args.json_out}")
    print(out)
    failed = [
        label
        for label, result in results.items()
        if result.get("rc", 0) != 0
        or result.get("repair_rc", 0) != 0
        or any(
            phase.get("returncode", 0) != 0
            for phase in result.get("phases", [])
            if isinstance(phase, dict)
        )
    ]
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
