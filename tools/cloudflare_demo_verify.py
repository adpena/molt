#!/usr/bin/env python3
"""Cloudflare split-runtime artifact validation and live verification helpers."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as _datetime
import json
import shlex
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path, PurePosixPath
from typing import Any

_WORKER_URL_RE = re.compile(r"https://[^\s\"'<>]+\.workers\.dev[^\s\"'<>]*")
_EXPECTED_PROBE_PATHS = (
    "/",
    "/fib/100",
    "/primes/1000",
    "/diamond/21",
    "/mandelbrot",
    "/fizzbuzz/30",
    "/pi/100000",
    "/generate/1",
    "/bench",
    "/sql?q=SELECT%20name%20FROM%20cities%20LIMIT%201",
    "/demo",
)


@dataclasses.dataclass(frozen=True)
class CloudflareBundleContract:
    bundle_root: Path
    wrangler_config: Path
    worker_js: Path
    app_wasm: Path
    runtime_wasm: Path
    manifest: Path
    name: str
    main: str
    compatibility_date: str
    no_bundle: bool
    find_additional_modules: bool
    rules: list[dict[str, Any]]


def _logs_root(project_root: Path) -> Path:
    return project_root / "logs" / "cloudflare-demo"


def _tmp_root(project_root: Path) -> Path:
    return project_root / "tmp" / "cloudflare-demo"


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _load_json_config(path: Path) -> dict[str, Any]:
    raw = path.read_text()
    return json.loads(raw)


def _rule_matches_path(rel_path: str, pattern: str) -> bool:
    path_obj = PurePosixPath(rel_path)
    if path_obj.match(pattern):
        return True
    if pattern.startswith("**/"):
        return path_obj.match(pattern[3:])
    return False


def _rule_type_covers_suffix(
    *,
    bundle_root: Path,
    rules: list[dict[str, Any]],
    rule_type: str,
    suffix: str,
) -> bool:
    required_paths = sorted(
        str(path.relative_to(bundle_root)).replace("\\", "/")
        for path in bundle_root.rglob(f"*{suffix}")
        if path.is_file()
    )
    if not required_paths:
        return False
    patterns: list[str] = []
    for rule in rules:
        if not isinstance(rule, dict):
            continue
        if rule.get("type") != rule_type or rule.get("fallthrough") is not False:
            continue
        globs = rule.get("globs")
        if not isinstance(globs, list):
            continue
        patterns.extend(str(item) for item in globs if isinstance(item, str))
    if not patterns:
        return False
    return all(
        any(_rule_matches_path(rel_path, pattern) for pattern in patterns)
        for rel_path in required_paths
    )


def _worker_json_const(worker_source: str, name: str) -> dict[str, Any]:
    match = re.search(rf"^\s*const {re.escape(name)} = (.+);$", worker_source, re.M)
    if match is None:
        raise RuntimeError(f"Cloudflare worker missing {name} ABI map")
    try:
        value = json.loads(match.group(1))
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"Cloudflare worker {name} ABI map is not valid JSON") from exc
    if not isinstance(value, dict):
        raise RuntimeError(f"Cloudflare worker {name} ABI map must be a JSON object")
    return value


def _validate_split_runtime_abi_manifest(
    *,
    manifest_data: dict[str, Any],
    worker_js: Path,
) -> None:
    abi = manifest_data.get("abi")
    if not isinstance(abi, dict):
        raise RuntimeError("Cloudflare manifest missing split-runtime ABI")

    runtime_imports = abi.get("runtime_imports")
    if not isinstance(runtime_imports, dict):
        raise RuntimeError("Cloudflare manifest missing runtime_imports ABI")
    if runtime_imports.get("module") != "molt_runtime":
        raise RuntimeError("Cloudflare manifest runtime_imports module must be molt_runtime")

    names = runtime_imports.get("names")
    signatures = runtime_imports.get("signatures")
    result_kinds = runtime_imports.get("result_kinds")
    if not isinstance(names, list) or not all(isinstance(name, str) for name in names):
        raise RuntimeError("Cloudflare manifest runtime_imports names must be strings")
    if not isinstance(signatures, dict):
        raise RuntimeError("Cloudflare manifest runtime_imports signatures must be an object")
    if not isinstance(result_kinds, dict):
        raise RuntimeError("Cloudflare manifest runtime_imports result_kinds must be an object")
    if names != sorted(signatures):
        raise RuntimeError("Cloudflare manifest runtime_imports names drifted from signatures")

    table_refs = abi.get("table_refs")
    if not isinstance(table_refs, dict):
        raise RuntimeError("Cloudflare manifest missing table_refs ABI")
    app_table_refs = table_refs.get("app")
    runtime_table_refs = table_refs.get("runtime")
    if not isinstance(app_table_refs, dict) or not isinstance(runtime_table_refs, dict):
        raise RuntimeError("Cloudflare manifest table_refs app/runtime entries must be objects")

    worker_source = worker_js.read_text(encoding="utf-8")
    if _worker_json_const(worker_source, "runtimeImportSignatures") != signatures:
        raise RuntimeError("Cloudflare worker runtime import signatures drifted from manifest")
    if _worker_json_const(worker_source, "runtimeImportResultKinds") != result_kinds:
        raise RuntimeError("Cloudflare worker runtime import result kinds drifted from manifest")
    if _worker_json_const(worker_source, "appTableRefSignatures") != app_table_refs:
        raise RuntimeError("Cloudflare worker app table ref signatures drifted from manifest")
    if _worker_json_const(worker_source, "runtimeTableRefSignatures") != runtime_table_refs:
        raise RuntimeError("Cloudflare worker runtime table ref signatures drifted from manifest")


def validate_bundle_contract(
    bundle_root: Path, wrangler_config: Path
) -> CloudflareBundleContract:
    if not bundle_root.is_dir():
        raise RuntimeError(f"Cloudflare bundle root not found: {bundle_root}")
    if not wrangler_config.is_file():
        raise RuntimeError(f"Cloudflare wrangler config not found: {wrangler_config}")

    worker_js = bundle_root / "worker.js"
    app_wasm = bundle_root / "app.wasm"
    runtime_wasm = bundle_root / "molt_runtime.wasm"
    manifest = bundle_root / "manifest.json"
    missing = [
        path.name
        for path in (worker_js, app_wasm, runtime_wasm, manifest)
        if not path.exists()
    ]
    if missing:
        raise RuntimeError(
            "Cloudflare bundle contract is incomplete: " + ", ".join(sorted(missing))
        )

    config = _load_json_config(wrangler_config)
    name = str(config.get("name", "")).strip()
    main = str(config.get("main", "")).strip()
    compatibility_date = str(config.get("compatibility_date", "")).strip()
    no_bundle = bool(config.get("no_bundle", False))
    find_additional_modules = bool(config.get("find_additional_modules", False))
    rules = config.get("rules")
    if not isinstance(rules, list):
        rules = []

    if not name:
        raise RuntimeError("Cloudflare wrangler config is missing name")
    if main != "worker.js":
        raise RuntimeError(
            f"Cloudflare wrangler config must point at worker.js, got: {main!r}"
        )
    try:
        _datetime.date.fromisoformat(compatibility_date)
    except ValueError as exc:
        raise RuntimeError(
            "Cloudflare wrangler config compatibility_date must be an ISO date, "
            f"got: {compatibility_date!r}"
        ) from exc
    if not no_bundle:
        raise RuntimeError("Cloudflare wrangler config must set no_bundle=true")
    if not find_additional_modules:
        raise RuntimeError(
            "Cloudflare wrangler config must set find_additional_modules=true"
        )
    if not _rule_type_covers_suffix(
        bundle_root=bundle_root,
        rules=rules,
        rule_type="ESModule",
        suffix=".js",
    ):
        raise RuntimeError(
            "Cloudflare wrangler config must include fallthrough=false ESModule "
            "rules covering every JavaScript bundle file"
        )
    if not _rule_type_covers_suffix(
        bundle_root=bundle_root,
        rules=rules,
        rule_type="CompiledWasm",
        suffix=".wasm",
    ):
        raise RuntimeError(
            "Cloudflare wrangler config must include fallthrough=false CompiledWasm "
            "rules covering every wasm bundle file"
        )

    manifest_data = json.loads(manifest.read_text())
    if manifest_data.get("mode") != "split-runtime":
        raise RuntimeError("Cloudflare manifest must be split-runtime")
    modules = manifest_data.get("modules")
    if not isinstance(modules, dict):
        raise RuntimeError("Cloudflare manifest missing modules map")
    expected_paths = {
        "app": "app.wasm",
        "runtime": "molt_runtime.wasm",
    }
    for module_name, expected_path in expected_paths.items():
        module_entry = modules.get(module_name)
        if not isinstance(module_entry, dict):
            raise RuntimeError(f"Cloudflare manifest missing {module_name} module")
        if module_entry.get("path") != expected_path:
            raise RuntimeError(
                f"Cloudflare manifest {module_name} path drifted: "
                f"{module_entry.get('path')!r} != {expected_path!r}"
            )
    _validate_split_runtime_abi_manifest(
        manifest_data=manifest_data,
        worker_js=worker_js,
    )

    return CloudflareBundleContract(
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
        worker_js=worker_js,
        app_wasm=app_wasm,
        runtime_wasm=runtime_wasm,
        manifest=manifest,
        name=name,
        main=main,
        compatibility_date=compatibility_date,
        no_bundle=no_bundle,
        find_additional_modules=find_additional_modules,
        rules=rules,
    )


def _run_command(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    verbose: bool,
) -> subprocess.CompletedProcess[str]:
    if verbose:
        print(f"Running: {subprocess.list2cmdline(cmd)}", file=sys.stderr)
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
    )


def run_wrangler_dry_run(
    *,
    wrangler: str,
    bundle_root: Path,
    wrangler_config: Path,
    project_root: Path,
    env: dict[str, str],
    json_output: bool,
    verbose: bool,
    run_id: str | None = None,
) -> subprocess.CompletedProcess[str]:
    session = run_id or uuid.uuid4().hex
    outdir = _tmp_root(project_root) / session / "dry-run"
    outdir.mkdir(parents=True, exist_ok=True)
    cmd = [
        wrangler,
        "deploy",
        "--dry-run",
        "--no-bundle",
        "--outdir",
        str(outdir),
        "--config",
        str(wrangler_config),
    ]
    result = _run_command(cmd, cwd=bundle_root, env=env, verbose=verbose)
    log_root = _logs_root(project_root) / session
    _write_text(log_root / "wrangler-dry-run.stdout.log", result.stdout or "")
    _write_text(log_root / "wrangler-dry-run.stderr.log", result.stderr or "")
    _write_text(
        log_root / "wrangler-dry-run.json",
        json.dumps(
            {
                "command": cmd,
                "returncode": result.returncode,
                "outdir": str(outdir),
                "bundle_root": str(bundle_root),
                "wrangler_config": str(wrangler_config),
            },
            indent=2,
        )
        + "\n",
    )
    return result


def run_wrangler_deploy(
    *,
    wrangler: str,
    bundle_root: Path,
    wrangler_config: Path,
    project_root: Path,
    env: dict[str, str],
    wrangler_args: str,
    json_output: bool,
    verbose: bool,
    run_id: str | None = None,
) -> subprocess.CompletedProcess[str]:
    session = run_id or uuid.uuid4().hex
    cmd = [wrangler, "deploy", "--no-bundle", "--config", str(wrangler_config)]
    if wrangler_args:
        cmd.extend(shlex.split(wrangler_args))
    result = _run_command(cmd, cwd=bundle_root, env=env, verbose=verbose)
    log_root = _logs_root(project_root) / session
    _write_text(log_root / "wrangler-deploy.stdout.log", result.stdout or "")
    _write_text(log_root / "wrangler-deploy.stderr.log", result.stderr or "")
    _write_text(
        log_root / "wrangler-deploy.json",
        json.dumps(
            {
                "command": cmd,
                "returncode": result.returncode,
                "bundle_root": str(bundle_root),
                "wrangler_config": str(wrangler_config),
            },
            indent=2,
        )
        + "\n",
    )
    return result


def extract_live_url(output_text: str) -> str | None:
    matches = _WORKER_URL_RE.findall(output_text)
    if not matches:
        return None
    return matches[-1]


@dataclasses.dataclass(frozen=True)
class LiveProbe:
    path: str
    expected_status: int = 200
    expected_content_type_prefix: str | None = "text/"


@dataclasses.dataclass(frozen=True)
class LiveVerificationResult:
    returncode: int
    stdout: str
    stderr: str
    report_path: Path


def _probe_url(base_url: str, path: str, timeout_s: float) -> tuple[int, str, str]:
    url = base_url.rstrip("/") + path
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "molt-cloudflare-demo-verify/1.0"},
    )
    with urllib.request.urlopen(request, timeout=timeout_s) as response:
        body = response.read().decode("utf-8", errors="replace")
        content_type = response.headers.get("Content-Type", "")
        return response.status, content_type, body


def verify_live_endpoint(
    *,
    live_url: str,
    bundle_root: Path,
    project_root: Path,
    json_output: bool,
    verbose: bool,
    run_id: str | None = None,
    probes: list[LiveProbe] | None = None,
    timeout_s: float = 30.0,
) -> LiveVerificationResult:
    session = run_id or uuid.uuid4().hex
    log_root = _logs_root(project_root) / session
    log_root.mkdir(parents=True, exist_ok=True)
    probes = probes or [LiveProbe(path=path) for path in _EXPECTED_PROBE_PATHS]
    report: list[dict[str, Any]] = []
    errors: list[str] = []
    start = time.perf_counter()
    for probe in probes:
        try:
            status, content_type, body = _probe_url(live_url, probe.path, timeout_s)
        except urllib.error.HTTPError as exc:
            status = exc.code
            content_type = exc.headers.get("Content-Type", "") if exc.headers else ""
            body = exc.read().decode("utf-8", errors="replace")
        except (
            Exception
        ) as exc:  # pragma: no cover - network failures are environment-specific
            errors.append(f"{probe.path}: {exc}")
            report.append(
                {
                    "path": probe.path,
                    "error": str(exc),
                }
            )
            continue

        entry = {
            "path": probe.path,
            "status": status,
            "content_type": content_type,
            "body_prefix": body[:120],
        }
        report.append(entry)
        if status != probe.expected_status:
            errors.append(
                f"{probe.path}: expected HTTP {probe.expected_status}, got {status}"
            )
        if probe.expected_content_type_prefix and not content_type.startswith(
            probe.expected_content_type_prefix
        ):
            errors.append(
                f"{probe.path}: expected content-type starting with "
                f"{probe.expected_content_type_prefix!r}, got {content_type!r}"
            )
        if not body:
            errors.append(f"{probe.path}: empty response body")
        if body.startswith("\x00"):
            errors.append(f"{probe.path}: body has a leading NUL byte")
        if "1102" in body or "Error" == body.strip()[:5]:
            errors.append(f"{probe.path}: body indicates a runtime error")

    elapsed = time.perf_counter() - start
    report_path = log_root / "live-verify.json"
    _write_text(
        report_path,
        json.dumps(
            {
                "live_url": live_url,
                "bundle_root": str(bundle_root),
                "elapsed_s": round(elapsed, 3),
                "probes": report,
                "errors": errors,
            },
            indent=2,
        )
        + "\n",
    )
    stdout = "\n".join(
        [
            f"Verified {live_url} with {len(probes)} probes",
            *([f"Errors: {len(errors)}"] if errors else ["Errors: 0"]),
        ]
    )
    stderr = "\n".join(errors)
    return LiveVerificationResult(
        returncode=0 if not errors else 1,
        stdout=stdout,
        stderr=stderr,
        report_path=report_path,
    )


class VerificationError(RuntimeError):
    """Raised when a demo endpoint verification check fails."""


@dataclasses.dataclass(frozen=True)
class EndpointCase:
    name: str
    path: str
    query: str = ""
    body: bytes | None = None
    source_exit_codes: tuple[int, ...] = (0,)
    source_contains: tuple[str, ...] = ()
    source_not_contains: tuple[str, ...] = ()
    expected_status: int = 200
    expected_content_type_prefix: str | None = None
    body_contains: tuple[str, ...] = ()
    body_not_contains: tuple[str, ...] = ()

    @property
    def target(self) -> str:
        if self.query:
            return f"{self.path}?{self.query}"
        return self.path


@dataclasses.dataclass(frozen=True)
class CaseResult:
    name: str
    target: str
    transport: str
    ok: bool
    return_code: int | None = None
    status: int | None = None
    content_type: str | None = None
    stdout: str = ""
    stderr: str = ""
    body: str = ""
    reason: str | None = None


@dataclasses.dataclass(frozen=True)
class VerificationReport:
    ok: bool
    failures: list[str]
    cases: tuple[CaseResult, ...]
    artifact_root: Path | None = None
    tmp_root: Path | None = None
    transport: str = ""
    case_set: str = ""


REPO_ROOT = Path(__file__).resolve().parents[1]


def assert_clean_text_body(raw: bytes) -> str:
    if b"\x00" in raw:
        raise VerificationError("response body contains NUL byte")
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise VerificationError("response body is not valid UTF-8") from exc


def _write_demo_summary(artifact_root: Path, report: VerificationReport) -> None:
    artifact_root.mkdir(parents=True, exist_ok=True)
    payload = {
        "ok": report.ok,
        "transport": report.transport,
        "case_set": report.case_set,
        "failures": report.failures,
        "case_count": len(report.cases),
        "cases": [
            {
                "name": case.name,
                "target": case.target,
                "transport": case.transport,
                "ok": case.ok,
                "return_code": case.return_code,
                "status": case.status,
                "content_type": case.content_type,
                "stdout_preview": case.stdout[:800],
                "stderr_preview": case.stderr[:800],
                "body_preview": case.body[:800],
                "reason": case.reason,
            }
            for case in report.cases
        ],
    }
    _write_text(
        artifact_root / "summary.json",
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n",
    )


def build_demo_matrix() -> tuple[EndpointCase, ...]:
    return (
        EndpointCase(
            name="root",
            path="/",
            source_contains=("Python compiled to WebAssembly.", "Endpoints"),
            body_contains=("Python compiled to WebAssembly.", "Endpoints"),
        ),
        EndpointCase(
            name="fib_500",
            path="/fib/500",
            source_contains=("Fibonacci", "fib(n)"),
            body_contains=("Fibonacci", "fib(n)"),
        ),
        EndpointCase(
            name="primes_1000",
            path="/primes/1000",
            source_contains=("Prime Numbers", "count"),
            body_contains=("Prime Numbers", "count"),
        ),
        EndpointCase(
            name="diamond_21",
            path="/diamond/21",
            source_contains=("*********************",),
            body_contains=("*********************",),
        ),
        EndpointCase(
            name="mandelbrot_default",
            path="/mandelbrot",
            source_contains=("Mandelbrot Set", "resolution:"),
            body_contains=("Mandelbrot Set", "resolution:"),
        ),
        EndpointCase(
            name="mandelbrot_query",
            path="/mandelbrot",
            query="width=120&height=20&iter=40&cx=-0.5&cy=0.0&zoom=1.0",
            source_contains=("Mandelbrot Set", "resolution: 120x20"),
            body_contains=("Mandelbrot Set", "resolution: 120x20"),
        ),
        EndpointCase(
            name="sort_query",
            path="/sort",
            query="data=42,7,19,3,88,1",
            source_contains=("Sort", "after  = [1, 3, 7, 19, 42, 88]"),
            body_contains=("Sort", "after  = [1, 3, 7, 19, 42, 88]"),
        ),
        EndpointCase(
            name="sort_path_fallback",
            path="/sort/9,3,1",
            source_contains=("after  = [1, 3, 9]",),
            body_contains=("after  = [1, 3, 9]",),
        ),
        EndpointCase(
            name="fizzbuzz_30",
            path="/fizzbuzz/30",
            source_contains=("FizzBuzz (1 to 30)", "FizzBuzz"),
            body_contains=("FizzBuzz (1 to 30)", "FizzBuzz"),
        ),
        EndpointCase(
            name="pi_100000",
            path="/pi/100000",
            source_contains=("Pi Approximation", "terms    = 100,000"),
            body_contains=("Pi Approximation", "terms    = 100,000"),
        ),
        EndpointCase(
            name="generate_one",
            path="/generate/1",
            source_contains=("microGPT", "Temperature: 0.5"),
            body_contains=("microGPT", "Temperature: 0.5"),
        ),
        EndpointCase(
            name="bench",
            path="/bench",
            source_contains=("Benchmark Suite", "All benchmarks completed"),
            body_contains=("Benchmark Suite", "All benchmarks completed"),
        ),
        EndpointCase(
            name="sql_landing",
            path="/sql",
            source_contains=("SQL Playground", "Example Queries"),
            expected_content_type_prefix="text/html",
            body_contains=("SQL Playground", "Example Queries"),
        ),
        EndpointCase(
            name="sql_query",
            path="/sql",
            query="q=SELECT%20*%20FROM%20cities%20LIMIT%201",
            source_contains=("Tokyo", "1 row(s)"),
            body_contains=("Tokyo", "1 row(s)"),
        ),
        EndpointCase(
            name="demo_landing",
            path="/demo",
            source_contains=("<title>Moltlang", "Cloudflare Workers"),
            expected_content_type_prefix="text/html",
            body_contains=("<title>Moltlang", "Cloudflare Workers"),
        ),
    )


def build_demo_fuzz_matrix() -> tuple[EndpointCase, ...]:
    return (
        EndpointCase(
            name="fib_negative_clamps",
            path="/fib/-1",
            source_contains=("n      = 0", "fib(n) = 0"),
            body_contains=("n      = 0", "fib(n) = 0"),
        ),
        EndpointCase(
            name="fib_overflow_clamps",
            path="/fib/1000000000000",
            source_contains=("n      = 10,000",),
            body_contains=("n      = 10,000",),
        ),
        EndpointCase(
            name="primes_floor_clamps",
            path="/primes/1",
            source_contains=("range  = 2 to 2", "count  = 1"),
            body_contains=("range  = 2 to 2", "count  = 1"),
        ),
        EndpointCase(
            name="generate_negative_clamps",
            path="/generate/-1",
            source_contains=("microGPT", "Temperature: 0.5"),
            body_contains=("microGPT", "Temperature: 0.5"),
        ),
        EndpointCase(
            name="generate_overflow_clamps",
            path="/generate/999999999999",
            source_contains=("microGPT", "Temperature: 0.5"),
            body_contains=("microGPT", "Temperature: 0.5"),
        ),
        EndpointCase(
            name="sort_sparse_query",
            path="/sort",
            query="data=3,,1,foo,2",
            source_contains=("skipped = foo", "after  = [1, 2, 3]"),
            body_contains=("skipped = foo", "after  = [1, 2, 3]"),
        ),
        EndpointCase(
            name="sort_separator_abuse",
            path="/sort",
            query="data=1&data=2&&",
            source_contains=("count  = 1 elements", "after  = [2]"),
            body_contains=("count  = 1 elements", "after  = [2]"),
        ),
        EndpointCase(
            name="sort_oversize_rejects_cleanly",
            path="/sort",
            query="data=" + ",".join(str(i) for i in range(1001)),
            source_exit_codes=(1,),
            source_not_contains=("Traceback",),
        ),
        EndpointCase(
            name="mandelbrot_boundary_query",
            path="/mandelbrot",
            query="width=0&height=-1&iter=9999999&cx=abc&cy=%00&zoom=0.01",
            source_contains=("Mandelbrot Set", "resolution: 20x10"),
            body_contains=("Mandelbrot Set", "resolution: 20x10"),
        ),
        EndpointCase(
            name="sql_bad_percent_escape",
            path="/sql",
            query="q=SELECT%20*%20FROM%20cities%20WHERE%20name%20LIKE%20%27%ZZ%27",
            source_contains=("0 row(s)",),
            body_contains=("0 row(s)",),
        ),
        EndpointCase(
            name="sql_query_separator_noise",
            path="/sql",
            query="&&q=SELECT%20name%20FROM%20cities%20LIMIT%201&&",
            source_contains=("name", "Tokyo", "1 row(s)"),
            body_contains=("name", "Tokyo", "1 row(s)"),
        ),
        EndpointCase(
            name="pi_floor_clamps",
            path="/pi/-1",
            source_contains=("terms    = 1", "pi       = 4.0"),
            body_contains=("terms    = 1", "pi       = 4.0"),
        ),
    )


def _run_source_case(entry: Path, case: EndpointCase) -> CaseResult:
    cmd = [sys.executable, str(entry.resolve()), case.path, case.query]
    completed = subprocess.run(
        cmd,
        capture_output=True,
        cwd=str(REPO_ROOT),
        text=False,
        check=False,
    )
    stdout = assert_clean_text_body(completed.stdout)
    stderr = assert_clean_text_body(completed.stderr)
    if completed.returncode not in case.source_exit_codes:
        raise VerificationError(
            f"{case.name}: unexpected exit code {completed.returncode}"
        )
    for needle in case.source_contains:
        if needle not in stdout:
            raise VerificationError(f"{case.name}: missing stdout sentinel {needle!r}")
    for needle in case.source_not_contains:
        if needle in stdout or needle in stderr:
            raise VerificationError(
                f"{case.name}: unexpected sentinel {needle!r} in output"
            )
    return CaseResult(
        name=case.name,
        target=case.target,
        transport="source",
        ok=True,
        return_code=completed.returncode,
        stdout=stdout,
        stderr=stderr,
    )


def _http_request(base_url: str, case: EndpointCase) -> tuple[int, str, str]:
    url = base_url.rstrip("/") + case.path
    if case.query:
        url = f"{url}?{case.query}"
    req = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=15.0) as resp:
            status = resp.status
            content_type = resp.headers.get("Content-Type", "")
            body_bytes = resp.read()
    except urllib.error.HTTPError as exc:
        status = exc.code
        content_type = exc.headers.get("Content-Type", "") if exc.headers else ""
        body_bytes = exc.read()
    body = assert_clean_text_body(body_bytes)
    return status, content_type, body


def _run_http_case(base_url: str, case: EndpointCase) -> CaseResult:
    status, content_type, body = _http_request(base_url, case)
    if status != case.expected_status:
        raise VerificationError(
            f"{case.name}: expected status {case.expected_status}, got {status}"
        )
    if case.expected_content_type_prefix is not None and not content_type.startswith(
        case.expected_content_type_prefix
    ):
        raise VerificationError(
            f"{case.name}: expected Content-Type prefix "
            f"{case.expected_content_type_prefix!r}, got {content_type!r}"
        )
    for needle in case.body_contains:
        if needle not in body:
            raise VerificationError(f"{case.name}: missing body sentinel {needle!r}")
    for needle in case.body_not_contains:
        if needle in body:
            raise VerificationError(f"{case.name}: unexpected body sentinel {needle!r}")
    return CaseResult(
        name=case.name,
        target=case.target,
        transport="http",
        ok=True,
        status=status,
        content_type=content_type,
        body=body,
    )


def _verify_cases(
    transport: str,
    cases: tuple[EndpointCase, ...],
    runner: Any,
    artifact_root: Path,
    case_set: str,
    tmp_root: Path | None = None,
) -> VerificationReport:
    case_results: list[CaseResult] = []
    failures: list[str] = []
    for case in cases:
        try:
            result = runner(case)
        except VerificationError as exc:
            failures.append(f"{case.name}: {exc}")
            case_results.append(
                CaseResult(
                    name=case.name,
                    target=case.target,
                    transport=transport,
                    ok=False,
                    reason=str(exc),
                )
            )
            continue
        case_results.append(result)
    report = VerificationReport(
        ok=not failures,
        failures=failures,
        cases=tuple(case_results),
        artifact_root=artifact_root,
        tmp_root=tmp_root,
        transport=transport,
        case_set=case_set,
    )
    _write_demo_summary(artifact_root, report)
    if failures:
        raise VerificationError("; ".join(failures))
    return report


def verify_source_matrix(
    entry: Path,
    cases: tuple[EndpointCase, ...],
    *,
    artifact_root: Path,
    tmp_root: Path | None = None,
    case_set: str = "custom",
) -> VerificationReport:
    def runner(case: EndpointCase) -> CaseResult:
        return _run_source_case(entry, case)

    return _verify_cases("source", cases, runner, artifact_root, case_set, tmp_root)


def verify_http_matrix(
    base_url: str,
    cases: tuple[EndpointCase, ...],
    *,
    artifact_root: Path,
    tmp_root: Path | None = None,
    case_set: str = "custom",
) -> VerificationReport:
    def runner(case: EndpointCase) -> CaseResult:
        return _run_http_case(base_url, case)

    return _verify_cases("http", cases, runner, artifact_root, case_set, tmp_root)


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate Cloudflare demo artifacts.")
    parser.add_argument("bundle_root", type=Path)
    parser.add_argument("--wrangler-config", type=Path, required=True)
    parser.add_argument("--live-url")
    args = parser.parse_args()
    validate_bundle_contract(args.bundle_root, args.wrangler_config)
    if args.live_url:
        result = verify_live_endpoint(
            live_url=args.live_url,
            bundle_root=args.bundle_root,
            project_root=args.bundle_root.parent.parent,
            json_output=False,
            verbose=False,
        )
        if result.returncode != 0:
            print(result.stderr, file=sys.stderr)
            return result.returncode
        print(result.stdout)
    else:
        print("Cloudflare bundle contract validated.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
