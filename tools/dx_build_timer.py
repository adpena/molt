#!/usr/bin/env python3
"""DX build-timing harness for the build-throughput arc (foundation/08_DX-buildspeed.md).

Measures wall-clock for the canonical backend-daemon build in well-defined
scenarios:
  - cold      : clean target dir, full build
  - inc-<file>: touch ONE file then rebuild (incremental)
  - test-lib  : `cargo test --lib --no-run` compile time after a touch

It drives `cargo` directly (NOT `molt build`) because the thing being optimised
is the cargo build of the backend crate(s) themselves. Each scenario is run N
times; we report min/median/max so noise from other agents is visible.

This tool never runs a compiled Molt binary, but every Cargo child still routes
through the shared memory guard because build throughput work must not bypass
process-tree custody.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import statistics
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import harness_memory_guard  # noqa: E402

TOUCH_MARKER = b"\n// dx_build_timer touch\n"


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
) -> tuple[harness_memory_guard.GuardedCompletedProcess, float]:
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
        args.profile,
        "-p",
        args.package,
        "--features",
        args.features,
        "--tests",
        "--no-run",
    ]


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
            "package": args.package,
            "features": args.features,
            "runs": args.runs,
            "target_dir": args.target_dir,
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
    ap.add_argument("--package", default="molt-backend")
    ap.add_argument("--features", default="native-backend")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--target-dir", required=True, help="CARGO_TARGET_DIR to use")
    ap.add_argument(
        "--scenarios",
        nargs="+",
        default=[
            "cold",
            "inc-value_range",
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
    args = ap.parse_args()

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = args.target_dir
    touch_journal = TouchJournal(Path(args.target_dir) / ".dx_build_timer_touches.json")
    touch_journal.recover()

    touch_files = {
        "value_range": REPO_ROOT / "runtime/molt-tir/src/tir/passes/value_range.rs",
        "function_compiler": REPO_ROOT
        / "runtime/molt-backend/src/native_backend/function_compiler.rs",
        "modules": REPO_ROOT / "runtime/molt-runtime/src/builtins/modules.rs",
        "gvn": REPO_ROOT / "runtime/molt-tir/src/tir/passes/gvn.rs",
    }

    results: dict[str, dict] = {}
    cargo_version = _output_text(
        _run_completed(["cargo", "--version"], env, REPO_ROOT)[0].stdout
    ).strip()
    prime: dict[str, object] | None = None

    def measure(label: str, prep, cmd: list[str]) -> None:
        samples = []
        rc_last = 0
        tail_last = ""
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
            rc_last, tail_last = rc, tail
            samples.append(round(elapsed, 2))
            print(
                f"  [{label}] run {i + 1}/{args.runs}: {elapsed:.2f}s rc={rc}",
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
            }
            _write_snapshot(
                args,
                results,
                cargo_version=cargo_version,
                prime=prime,
            )
            if rc != 0:
                print(f"    FAILED:\n{tail}", flush=True)
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
            },
        )

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
        else:
            print(f"unknown scenario: {scen}", file=sys.stderr)

    payload = _snapshot_payload(args, results, cargo_version=cargo_version, prime=prime)
    out = json.dumps(payload, indent=2)
    if args.json_out:
        _write_snapshot(args, results, cargo_version=cargo_version, prime=prime)
        print(f"wrote {args.json_out}")
    print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
