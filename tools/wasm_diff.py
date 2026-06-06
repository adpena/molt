#!/usr/bin/env python3
"""WASM differential calibration driver (task #55 — suite-honesty WASM dimension).

Produces a molt_diff-COMPATIBLE results JSONL for the WASM backend, so the
suite-honesty ratchet (tools/check_suite_honesty.py) can reality-check the
`wasm` dimension exactly the way it reality-checks `native` against
native_calibration.jsonl. This is the WASM analogue of running
tests/molt_diff.py with MOLT_DIFF_RESULTS_JSONL set: one JSON line per test
with its RAW status (`pass`/`fail`/`error`/`skip`) and the `expect_molt_fail`
partition flag.

WHY A SEPARATE DRIVER (not a molt_diff flag)
--------------------------------------------
tests/molt_diff.py drives the NATIVE backend through the build daemon and runs
the produced native binary. The WASM path is structurally different: the only
supported way to RUN a Molt wasm module is the canonical Node host shim
(`node wasm/run_wasm.js <output_linked.wasm>`) — bare wasmtime/wasmer cannot
satisfy the `env.molt_*_host` imports by design (see tools/wasm_run_matrix.py
and task #62). So the MOLT half (build + run) is wasm-specific; everything else
— the CPython oracle, the per-test `# MOLT_META` gating, the stdout/stderr
canonicalization, and the `expect_molt_fail` partition (too-dynamic manifest U
inline `expect_fail=molt`) — is REUSED VERBATIM from tests/molt_diff.py so the
wasm verdict is computed with byte-identical semantics to the native lane. No
parallel reimplementation of the comparison rules can drift.

VERDICT (identical to molt_diff.diff_test)
------------------------------------------
For each test file:
  meta        = molt_diff._collect_meta(file)
  skip?       = molt_diff._should_skip(meta, python_version, host_tags+{wasm})
  cp_out/err/ret = molt_diff.run_cpython(file)            # the oracle
  molt_out/err/ret = <build --target wasm, run via node>  # this driver
  PASS iff canonicalize(cp_out)==canonicalize(molt_out)
           and cp_ret==molt_ret and stderr_matches(...)
A build failure (no linked wasm emitted) is a `fail` (matching molt_diff's
"Molt failed to build" branch), except the CPython-compile-error parity case.

The `expect_molt_fail` flag PARTITIONS the fail space precisely as molt_diff
does, so the suite-honesty ratchet owns only the SILENT wasm failures and never
overlaps the too-dynamic / inline-meta channels.

USAGE
-----
  MOLT_DIFF_RESULTS_JSONL=tools/suite_honesty/wasm_calibration.jsonl \\
  python3 tools/wasm_diff.py --files-from LIST.txt --jobs 1

Run SERIAL (jobs=1) for calibration: a contended parallel build can produce
false build-failures; the calibration's trustworthiness rests on each seeded
fail being reproducible in isolation.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Import the differential harness as a library: it owns the oracle + meta +
# comparison + partition logic, which we reuse verbatim.
sys.path.insert(0, str(ROOT / "tests"))
import molt_diff  # noqa: E402

RUN_WASM_JS = ROOT / "wasm" / "run_wasm.js"


def _node_bin() -> str:
    import shutil

    node = shutil.which("node")
    if node is None:
        raise RuntimeError(
            "node is required to run Molt wasm modules (canonical host shim "
            "wasm/run_wasm.js); none found on PATH."
        )
    return node


def _wasm_build_timeout() -> float:
    raw = os.environ.get("MOLT_WASM_DIFF_BUILD_TIMEOUT", "").strip()
    try:
        v = float(raw)
        if v > 0:
            return v
    except ValueError:
        pass
    return 600.0


def _wasm_run_timeout() -> float:
    raw = os.environ.get("MOLT_WASM_DIFF_RUN_TIMEOUT", "").strip()
    try:
        v = float(raw)
        if v > 0:
            return v
    except ValueError:
        pass
    return 60.0


def _build_wasm(
    file_path: str, out_dir: Path, build_profile: str
) -> tuple[Path | None, str]:
    """Build `file_path` to a linked wasm module. Returns (linked_wasm | None, err)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    cmd = [
        molt_diff._resolve_molt_cli_python(),
        "-m",
        "molt.cli",
        "build",
        file_path,
        "--target",
        "wasm",
        "--build-profile",
        build_profile,
        "--respect-pythonpath",
        "--out-dir",
        str(out_dir),
    ]
    env = dict(os.environ)
    # The differential build grants the same standard capabilities the native
    # lane grants, so wasm parity is measured on equal footing.
    env.setdefault("MOLT_DIFF_CAPABILITIES", "fs,env,time,random")
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(ROOT),
            env=env,
            capture_output=True,
            text=True,
            timeout=_wasm_build_timeout(),
        )
    except subprocess.TimeoutExpired:
        return None, f"wasm build timeout after {_wasm_build_timeout()}s"
    linked = out_dir / "output_linked.wasm"
    if proc.returncode != 0 or not linked.exists():
        return None, (proc.stderr or proc.stdout or "wasm build failed")
    return linked, ""


def _run_wasm(linked: Path, runtime_wasm: Path | None) -> tuple[str, str, int]:
    """Run the linked wasm via the canonical node host shim."""
    env = dict(os.environ)
    env["MOLT_WASM_PREFER_LINKED"] = "1"
    if runtime_wasm is not None and runtime_wasm.exists():
        env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    cmd = [_node_bin(), str(RUN_WASM_JS), str(linked)]
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(ROOT),
            env=env,
            capture_output=True,
            text=True,
            timeout=_wasm_run_timeout(),
        )
    except subprocess.TimeoutExpired:
        return "", f"wasm run timeout after {_wasm_run_timeout()}s", 124
    return proc.stdout, proc.stderr, proc.returncode


def _wasm_stderr_is_noise(line: str) -> bool:
    """Node emits an unconditional WASI ExperimentalWarning on stderr that is
    not a program diagnostic; strip it so stderr comparison is fair."""
    s = line.strip()
    if not s:
        return True
    return (
        "ExperimentalWarning" in s
        or "Use `node --trace-warnings" in s
        or s.startswith("(node:")
    )


def _strip_node_noise(err: str) -> str:
    return "\n".join(ln for ln in err.splitlines() if not _wasm_stderr_is_noise(ln))


def diff_wasm_test(
    file_path: str, python_exe: str, build_profile: str, out_root: Path
) -> dict:
    """Compute the wasm raw status for one test, reusing molt_diff semantics.

    Returns the record dict {file, raw_status, expect_molt_fail, resolved_status}.
    """
    npath = molt_diff._normalize_repo_relative(file_path)
    record: dict = {
        "file": npath,
        "raw_status": "skip",
        "expect_molt_fail": False,
        "resolved_status": "skip",
    }

    meta = molt_diff._collect_meta(file_path)
    manifest_expect_fail = molt_diff._manifest_marks_expected_failure(file_path)
    explicit_expect_fail = molt_diff._meta_expect_molt_fail(meta)
    expect_molt_fail = manifest_expect_fail or explicit_expect_fail
    record["expect_molt_fail"] = expect_molt_fail

    python_version = molt_diff._python_exe_version(python_exe)
    host_tags = molt_diff._host_platform_tags()  # MOLT_TARGET=wasm -> includes 'wasm'
    skip, _reason = molt_diff._should_skip(
        meta, python_version=python_version, host_tags=host_tags
    )
    if skip:
        record["raw_status"] = "skip"
        record["resolved_status"] = "skip"
        return record

    normalize = {v.lower() for v in meta.get("normalize", [])}
    stdout_mode = (meta.get("stdout", ["exact"])[0]).lower()
    stderr_mode = (meta.get("stderr", ["ignore"])[0]).lower()

    cp_out, cp_err, cp_ret = molt_diff.run_cpython(file_path, python_exe)

    safe_stem = npath.replace("/", "__").replace(".py", "")
    out_dir = out_root / safe_stem
    linked, build_err = _build_wasm(file_path, out_dir, build_profile)

    cp_out_n = molt_diff._normalize_output(cp_out, normalize)
    cp_err_n = molt_diff._normalize_output(cp_err, normalize)

    if linked is None:
        # Mirror molt_diff: a CPython compile-error that Molt also rejects at
        # build time is a PASS; otherwise a build failure is a fail.
        def is_compile_error(err: str) -> bool:
            return any(
                tag in err for tag in ("SyntaxError", "IndentationError", "TabError")
            )

        if cp_ret != 0 and is_compile_error(cp_err) and is_compile_error(build_err):
            record["raw_status"] = "pass"
            record["resolved_status"] = "pass"
            return record
        record["raw_status"] = "fail"
        record["resolved_status"] = "fail"
        record["detail"] = "wasm build failed: " + build_err.strip()[-400:]
        return record

    runtime_wasm = out_dir / "molt_runtime.wasm"
    molt_out, molt_err, molt_ret = _run_wasm(linked, runtime_wasm)
    molt_err = _strip_node_noise(molt_err)

    molt_out_n = molt_diff._normalize_output(molt_out, normalize)
    molt_err_n = molt_diff._normalize_output(molt_err, normalize)

    stderr_ok = molt_diff._stderr_matches(cp_err_n, molt_err_n, stderr_mode)
    cp_cmp = molt_diff._canonicalize_stdout(cp_out_n, stdout_mode)
    molt_cmp = molt_diff._canonicalize_stdout(molt_out_n, stdout_mode)

    if cp_cmp == molt_cmp and cp_ret == molt_ret and stderr_ok:
        record["raw_status"] = "pass"
        record["resolved_status"] = "pass"
    else:
        record["raw_status"] = "fail"
        record["resolved_status"] = "fail"
        record["detail"] = (
            f"cp_ret={cp_ret} molt_ret={molt_ret} "
            f"cp_out={cp_out_n!r:.200} molt_out={molt_out_n!r:.200} "
            f"molt_err={molt_err_n!r:.200}"
        )
    return record


molt_diff_FAILING = {"fail", "error", "oom"}


def _worker(args: tuple[str, str, str, str]) -> dict:
    file_path, python_exe, build_profile, out_root = args
    try:
        return diff_wasm_test(file_path, python_exe, build_profile, Path(out_root))
    except Exception as exc:  # fail-closed: a driver crash is an `error`, never a pass
        return {
            "file": molt_diff._normalize_repo_relative(file_path),
            "raw_status": "error",
            "expect_molt_fail": False,
            "resolved_status": "error",
            "detail": f"{type(exc).__name__}: {exc}",
        }


def _collect_files(paths: list[str], files_from: list[str]) -> list[str]:
    out: list[str] = []
    for lp in files_from:
        for line in Path(lp).read_text().splitlines():
            s = line.strip()
            if s and not s.startswith("#"):
                out.append(s)
    for p in paths:
        pp = Path(p)
        if pp.is_dir():
            out.extend(sorted(str(x) for x in pp.glob("*.py")))
        else:
            out.append(p)
    # Deduplicate preserving order.
    seen: set[str] = set()
    uniq: list[str] = []
    for f in out:
        n = molt_diff._normalize_repo_relative(f)
        if n not in seen:
            seen.add(n)
            uniq.append(f)
    return uniq


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("paths", nargs="*", help="test files or dirs")
    ap.add_argument("--files-from", action="append", default=[], help="list file(s)")
    ap.add_argument("--jobs", type=int, default=1, help="parallel workers (default 1)")
    ap.add_argument("--build-profile", default="dev", choices=["dev", "release"])
    ap.add_argument(
        "--python", default=sys.executable, help="CPython oracle interpreter"
    )
    ap.add_argument(
        "--out-root",
        default=str(ROOT / "tmp" / "wasm_diff"),
        help="scratch dir for wasm artifacts",
    )
    args = ap.parse_args(argv)

    # Ensure the host carries the wasm tag so _should_skip honors wasm gating.
    os.environ.setdefault("MOLT_TARGET", "wasm")

    files = _collect_files(args.paths, args.files_from)
    if not files:
        print("no test files", file=sys.stderr)
        return 2

    python_exe = molt_diff._resolve_python_exe(args.python)
    out_root = Path(args.out_root)
    out_root.mkdir(parents=True, exist_ok=True)

    results_path = os.environ.get("MOLT_DIFF_RESULTS_JSONL", "").strip()
    sink = open(results_path, "a", encoding="utf-8") if results_path else None

    total = len(files)
    counts = {"pass": 0, "fail": 0, "error": 0, "skip": 0}
    silent_fail = 0
    try:
        work = [(f, python_exe, args.build_profile, str(out_root)) for f in files]
        if args.jobs <= 1:
            done = 0
            for w in work:
                rec = _worker(w)
                done += 1
                _emit(rec, sink, done, total, counts)
                if (
                    rec["raw_status"] in molt_diff_FAILING
                    and not rec["expect_molt_fail"]
                ):
                    silent_fail += 1
        else:
            with ProcessPoolExecutor(max_workers=args.jobs) as ex:
                futs = {ex.submit(_worker, w): w for w in work}
                done = 0
                for fut in as_completed(futs):
                    rec = fut.result()
                    done += 1
                    _emit(rec, sink, done, total, counts)
                    if (
                        rec["raw_status"] in molt_diff_FAILING
                        and not rec["expect_molt_fail"]
                    ):
                        silent_fail += 1
    finally:
        if sink is not None:
            sink.close()

    print(
        f"\nwasm diff complete: {total} tests | "
        + " ".join(f"{k}={v}" for k, v in counts.items())
        + f" | {silent_fail} SILENT (untracked-channel) failures"
    )
    return 0


def _emit(rec: dict, sink, done: int, total: int, counts: dict) -> None:
    st = rec["raw_status"]
    counts[st] = counts.get(st, 0) + 1
    tag = st.upper()
    print(f"[{tag}] ({done}/{total}) {rec['file']}", flush=True)
    if sink is not None:
        sink.write(json.dumps(rec) + "\n")
        sink.flush()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
