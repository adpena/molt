#!/usr/bin/env python3
"""Run Cargo's canonical workspace tests and enforce the exact known-red set."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import NamedTuple

import check_suite_honesty

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_ROOT = ROOT / "proof-receipts" / "evidence"
RUNS_ROOT = EVIDENCE_ROOT / "cargo-test-truth-runs"
RECEIPT = EVIDENCE_ROOT / "cargo-test-truth.json"
TARGET_RUNNER = ROOT / "tools" / "cargo_test_binary_runner.py"
BINARY_TIMEOUT_SECONDS = 300.0
LOCKED_WORKSPACES = (
    ("root", ROOT / "Cargo.toml"),
    ("runtime", ROOT / "runtime" / "Cargo.toml"),
)
FETCH_TIMEOUT_SECONDS = 600.0
METADATA_TIMEOUT_SECONDS = 300.0
WORKSPACE_TEST_TIMEOUT_SECONDS = 7_200.0
CANONICAL_COMMAND = (
    "cargo",
    "test",
    "--locked",
    "--workspace",
    "--tests",
    "--no-fail-fast",
)
_RUN_ID_RE = re.compile(r"^[A-Za-z0-9_.-]+$")
_ACTIVE_RUN_TERMINALIZER: object | None = None
_SOURCE_SUFFIXES = frozenset(
    {
        ".c",
        ".cc",
        ".cpp",
        ".css",
        ".h",
        ".hpp",
        ".html",
        ".js",
        ".json",
        ".md",
        ".mjs",
        ".py",
        ".pyi",
        ".rs",
        ".sh",
        ".toml",
        ".ts",
        ".tsx",
        ".wat",
        ".wit",
        ".yaml",
        ".yml",
    }
)


def host_context() -> dict[str, str]:
    platform = {"win32": "windows", "darwin": "macos"}.get(sys.platform, "linux")
    return {"platform": platform, "target": "default"}


def parse_test_results(output: str, context: dict[str, str]) -> list[dict]:
    rows: dict[str, dict] = {}
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line.startswith("test ") or " ... " not in line:
            continue
        identity, status = line[5:].rsplit(" ... ", 1)
        identity = identity.removesuffix(" - should panic")
        if status == "ok":
            rows[identity] = {
                "identity": identity,
                "status": "pass",
                "context": context,
            }
        elif status == "FAILED":
            rows[identity] = {
                "identity": identity,
                "status": "fail",
                "context": context,
            }
    return list(rows.values())


def verdict(
    output: str,
    returncode: int,
    context: dict[str, str],
    *,
    binary_receipts: list[dict],
    expected_binaries: dict[str, dict[str, str]],
) -> list[str]:
    data = check_suite_honesty.load_manifest()
    problems = check_suite_honesty.validate_manifest(
        data, check_suite_honesty.load_too_dynamic_set()
    )
    rows, receipt_problems = receipt_test_rows(
        binary_receipts, expected_binaries, context
    )
    problems += receipt_problems
    problems += check_suite_honesty.execution_reality_check(data, rows)
    if returncode != 0 and not any(row["status"] == "fail" for row in rows):
        problems.append(
            "canonical Cargo truth command failed without an attributable test identity "
            "(compile/link/process failure cannot be registered as a test red)"
        )
    if "could not compile" in output or "error[" in output:
        problems.append("canonical Cargo truth command contained a compiler error")
    return problems


def host_target() -> str:
    process = _COMMANDS.run(
        ["rustc", "-vV"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if process.returncode != 0:
        raise RuntimeError(process.stderr.strip() or "rustc -vV failed")
    for line in process.stdout.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ").strip()
    raise RuntimeError("rustc -vV did not report a host target")


def target_runner_config(
    target: str,
    receipt_dir: Path,
    run_id: str | None = None,
    source_identity: dict[str, object] | None = None,
) -> str:
    argv = [
        sys.executable,
        str(TARGET_RUNNER),
        "--timeout-seconds",
        str(BINARY_TIMEOUT_SECONDS),
        "--receipt-dir",
        str(receipt_dir),
    ]
    if run_id is not None:
        argv.extend(("--run-id", run_id))
    if source_identity is not None:
        argv.extend(
            (
                "--source-identity-json",
                json.dumps(source_identity, sort_keys=True, separators=(",", ":")),
            )
        )
    argv.append("--")
    encoded = ",".join(json.dumps(item) for item in argv)
    return f"target.{target}.runner=[{encoded}]"


def load_binary_receipts(
    receipt_dir: Path,
    *,
    expected_run_id: str | None = None,
    expected_source_identity: dict[str, object] | None = None,
    workspace: str = "",
) -> list[dict]:
    receipts = []
    invocation_ids: set[str] = set()
    for path in sorted(receipt_dir.glob("*.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        if payload.get("schema") != "molt.cargo-test-binary.v1":
            raise RuntimeError(f"invalid Cargo test binary receipt schema: {path}")
        invocation_id = payload.get("invocation_id")
        if not isinstance(invocation_id, str) or not invocation_id:
            raise RuntimeError(f"Cargo test binary receipt lacks invocation identity: {path}")
        if invocation_id in invocation_ids:
            raise RuntimeError(
                f"duplicate Cargo test binary invocation identity: {invocation_id}"
            )
        invocation_ids.add(invocation_id)
        if expected_run_id is not None and payload.get("run_id") != expected_run_id:
            raise RuntimeError(
                f"Cargo test binary receipt escaped run custody: {path}"
            )
        if (
            expected_source_identity is not None
            and payload.get("source_identity") != expected_source_identity
        ):
            raise RuntimeError(
                f"Cargo test binary receipt escaped source custody: {path}"
            )
        executable = payload.get("executable_resolved")
        size = payload.get("executable_size")
        digest = payload.get("executable_sha256")
        if expected_run_id is not None:
            if (
                not isinstance(executable, str)
                or not isinstance(size, int)
                or size < 0
                or not isinstance(digest, str)
                or not re.fullmatch(r"[0-9a-f]{64}", digest)
            ):
                raise RuntimeError(
                    f"Cargo test binary receipt lacks executable byte identity: {path}"
                )
            current_size, current_digest = _file_identity(Path(executable))
            if (current_size, current_digest) != (size, digest):
                raise RuntimeError(
                    f"Cargo test binary changed after receipt publication: {executable}"
                )
        if workspace:
            payload["workspace"] = workspace
        receipts.append(payload)
    return receipts


def _file_identity(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return size, digest.hexdigest()


def _git_output(*args: str) -> bytes:
    process = _COMMANDS.run(
        ["git", *args],
        cwd=ROOT,
        check=False,
        capture_output=True,
    )
    if process.returncode != 0:
        stderr = process.stderr
        if isinstance(stderr, bytes):
            problem = stderr.decode("utf-8", errors="replace").strip()
        else:
            problem = str(stderr).strip()
        raise RuntimeError(problem or f"git {' '.join(args)} failed")
    stdout = process.stdout
    return stdout if isinstance(stdout, bytes) else str(stdout).encode("utf-8")


def git_source_identity() -> dict[str, object]:
    """Bind receipts to the exact Git-visible source tree used for the run."""
    head = _git_output("rev-parse", "HEAD").decode().strip()
    tree = _git_output("rev-parse", "HEAD^{tree}").decode().strip()
    tracked_patch = _git_output("diff", "--binary", "--no-ext-diff", "HEAD", "--")
    status = _git_output(
        "status", "--porcelain=v1", "-z", "--untracked-files=all"
    )
    untracked_paths = [
        Path(raw.decode("utf-8", errors="surrogateescape"))
        for raw in _git_output("ls-files", "--others", "--exclude-standard", "-z").split(
            b"\0"
        )
        if raw
    ]
    untracked_source_paths = [
        path
        for path in untracked_paths
        if path.suffix.lower() in _SOURCE_SUFFIXES
        and not path.name.endswith(".generation.json")
    ]
    untracked_source_digest = hashlib.sha256()
    untracked_source_bytes = 0
    for relative in sorted(
        untracked_source_paths, key=lambda path: path.as_posix()
    ):
        path = ROOT / relative
        if not path.is_file():
            continue
        size, digest = _file_identity(path)
        encoded_path = relative.as_posix().encode("utf-8", errors="surrogateescape")
        untracked_source_digest.update(encoded_path)
        untracked_source_digest.update(b"\0")
        untracked_source_digest.update(str(size).encode("ascii"))
        untracked_source_digest.update(b"\0")
        untracked_source_digest.update(digest.encode("ascii"))
        untracked_source_digest.update(b"\0")
        untracked_source_bytes += size
    return {
        "schema": "molt.git-source.v1",
        "head": head,
        "tree": tree,
        "dirty": bool(status),
        "status_sha256": hashlib.sha256(status).hexdigest(),
        "tracked_patch_sha256": hashlib.sha256(tracked_patch).hexdigest(),
        "untracked_file_count": len(untracked_paths),
        "untracked_source_file_count": len(untracked_source_paths),
        "untracked_source_bytes": untracked_source_bytes,
        "untracked_source_sha256": untracked_source_digest.hexdigest(),
        "truth_driver_sha256": _file_identity(Path(__file__).resolve())[1],
        "binary_runner_sha256": _file_identity(TARGET_RUNNER)[1],
    }


def run_identity(started: datetime) -> str:
    configured = os.environ.get("MOLT_CARGO_TEST_TRUTH_RUN_ID")
    identity = configured or f"{started.strftime('%Y%m%dT%H%M%S.%fZ')}-{os.getpid()}"
    if not _RUN_ID_RE.fullmatch(identity):
        raise RuntimeError(f"invalid Cargo truth run identity: {identity!r}")
    return identity


def prepare_run_directory(identity: str) -> Path:
    RUNS_ROOT.mkdir(parents=True, exist_ok=True)
    run_dir = (RUNS_ROOT / identity).resolve()
    if run_dir.parent != RUNS_ROOT.resolve():
        raise RuntimeError("Cargo truth run directory escaped evidence root")
    if run_dir.exists():
        raise RuntimeError(
            f"Cargo truth run identity already exists and is immutable: {identity}"
        )
    run_dir.mkdir()
    return run_dir


def _executable_key(value: str, workspace: str = "") -> str:
    executable = os.path.normcase(str(Path(value).resolve()))
    return f"{workspace}\0{executable}" if workspace else executable


def package_identities_from_metadata(metadata_output: str) -> dict[str, str]:
    """Build Cargo's exact package-id -> stable name@version authority."""
    documents = []
    for line in metadata_output.splitlines():
        if not line.startswith("{"):
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and isinstance(payload.get("packages"), list):
            documents.append(payload)
    if len(documents) != 1:
        raise RuntimeError(
            f"Cargo metadata emitted {len(documents)} package documents instead of one"
        )
    identities: dict[str, str] = {}
    for package in documents[0]["packages"]:
        if not isinstance(package, dict):
            raise RuntimeError("Cargo metadata contained a non-object package")
        package_id = package.get("id")
        name = package.get("name")
        version = package.get("version")
        if not all(isinstance(value, str) and value for value in (package_id, name, version)):
            raise RuntimeError("Cargo metadata package lacked id/name/version authority")
        identity = f"{name}@{version}"
        previous = identities.setdefault(package_id, identity)
        if previous != identity:
            raise RuntimeError(f"Cargo metadata contradicted package identity {package_id!r}")
    return identities


def expected_test_binaries(
    cargo_output: str, package_identities: dict[str, str]
) -> dict[str, dict[str, str]]:
    """Read Cargo's compiler-artifact JSON as the executable authority."""
    artifacts: list[dict] = []
    for line in cargo_output.splitlines():
        if not line.startswith("{"):
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if payload.get("reason") != "compiler-artifact":
            continue
        profile = payload.get("profile")
        if not isinstance(profile, dict) or profile.get("test") is not True:
            continue
        artifacts.append(payload)
    return expected_test_binaries_from_artifacts(artifacts, package_identities)


def expected_test_binaries_from_artifacts(
    artifacts: list[dict] | tuple[dict, ...],
    package_identities: dict[str, str],
    *,
    workspace: str = "",
) -> dict[str, dict[str, str]]:
    """Reduce streamed test artifacts to Cargo's stable executable identity map."""
    expected: dict[str, dict[str, str]] = {}
    for payload in artifacts:
        executable = payload.get("executable")
        target = payload.get("target")
        package_id = payload.get("package_id")
        if not isinstance(executable, str) or not executable:
            continue
        if (
            not isinstance(target, dict)
            or not isinstance(target.get("name"), str)
            or not isinstance(target.get("kind"), list)
            or not target["kind"]
            or not all(isinstance(kind, str) for kind in target["kind"])
            or not isinstance(package_id, str)
        ):
            raise RuntimeError(
                "Cargo test compiler-artifact lacked package/target namespace authority"
            )
        key = _executable_key(executable, workspace)
        package_identity = package_identities.get(package_id)
        if package_identity is None:
            raise RuntimeError(
                "Cargo compiler-artifact package_id was absent from metadata authority: "
                f"{package_id!r}"
            )
        metadata = {
            "workspace": workspace,
            "package": package_identity,
            "target_name": target["name"],
            "target_kind": "+".join(sorted(target["kind"])),
            "executable": str(Path(executable).resolve()),
        }
        previous = expected.setdefault(key, metadata)
        if previous != metadata:
            raise RuntimeError(
                f"Cargo published conflicting identities for test executable: {executable}"
            )
    return expected


def binary_coverage_problems(
    expected: dict[str, dict[str, str]], receipts: list[dict]
) -> list[str]:
    problems: list[str] = []
    if not expected:
        problems.append("Cargo JSON reported zero expected test binaries")
    if not receipts:
        problems.append("Cargo invoked zero receipt-producing test binaries")
    observed_list = [
        _executable_key(
            str(receipt.get("executable_resolved") or receipt.get("executable") or ""),
            str(receipt.get("workspace") or ""),
        )
        for receipt in receipts
    ]
    observed = set(observed_list)
    if len(observed) != len(observed_list):
        problems.append("duplicate Cargo test binary receipts were published")
    missing = sorted(set(expected) - observed)
    unexpected = sorted(observed - set(expected))
    if missing:
        problems.append(
            "Cargo test binary receipt coverage incomplete; missing="
            + json.dumps(missing)
        )
    if unexpected:
        problems.append(
            "Cargo test binary receipts lack compiler-artifact authority; unexpected="
            + json.dumps(unexpected)
        )
    return problems


def _namespaced_test_identity(metadata: dict[str, str], identity: str) -> str:
    test = identity.removesuffix(" - should panic")
    workspace = metadata.get("workspace", "")
    return (
        f"{f'{workspace}::' if workspace else ''}{metadata['package']}::{metadata['target_kind']}:"
        f"{metadata['target_name']}::{test}"
    )


def receipt_test_rows(
    receipts: list[dict],
    expected: dict[str, dict[str, str]],
    context: dict[str, str],
) -> tuple[list[dict], list[str]]:
    """Derive known-red-eligible reality solely from typed binary receipts."""
    rows: list[dict] = []
    problems: list[str] = []
    observed_rows: dict[str, str] = {}
    published_rows: set[str] = set()
    for receipt in receipts:
        executable = _executable_key(
            str(receipt.get("executable_resolved") or receipt.get("executable") or ""),
            str(receipt.get("workspace") or ""),
        )
        metadata = expected.get(executable)
        if metadata is None:
            continue
        namespaced_context = {
            **context,
            "package": metadata["package"],
            "cargo_target": metadata["target_name"],
            "cargo_target_kind": metadata["target_kind"],
            "executable": metadata["executable"],
            "workspace": metadata.get("workspace", "root"),
        }
        raw_results = receipt.get("test_results")
        if not isinstance(raw_results, list):
            problems.append(
                f"Cargo test binary receipt lacks structured test results: {metadata['executable']}"
            )
            raw_results = []
        confirmed = receipt.get("failure_identities")
        if not isinstance(confirmed, list) or not all(
            isinstance(identity, str) for identity in confirmed
        ):
            problems.append(
                f"Cargo test binary receipt has invalid failure identities: {metadata['executable']}"
            )
            confirmed = []
        confirmed_set = set(confirmed)
        receipt_rows: dict[str, str] = {}
        for result in raw_results:
            if not isinstance(result, dict):
                problems.append("Cargo test binary receipt has a non-object test result")
                continue
            identity = result.get("identity")
            status = result.get("status")
            if not isinstance(identity, str) or status not in {"pass", "fail"}:
                problems.append(
                    f"Cargo test binary receipt has invalid structured result: {result!r}"
                )
                continue
            prior = receipt_rows.setdefault(identity, status)
            if prior != status:
                problems.append(
                    f"Cargo test binary reported contradictory outcomes for {identity!r}"
                )
            if status == "fail" and identity not in confirmed_set:
                problems.append(
                    f"Cargo test binary reported an unconfirmed failure identity: {identity!r}"
                )
        for identity in confirmed_set:
            if receipt_rows.get(identity) == "pass":
                problems.append(
                    "Cargo test binary contradicted pass with confirmed failure "
                    f"for {identity!r}"
                )
                continue
            receipt_rows[identity] = "fail"
        if receipt.get("status") != "success" and not confirmed_set:
            diagnosis = receipt.get("diagnosis")
            kind = diagnosis.get("kind") if isinstance(diagnosis, dict) else None
            candidates = (
                diagnosis.get("candidate_tests")
                if isinstance(diagnosis, dict)
                else None
            )
            problems.append(
                "Cargo test binary failed with structural attribution only; "
                f"kind={kind!r} candidates={candidates!r} executable={metadata['executable']!r}"
            )
        for identity, status in receipt_rows.items():
            namespaced = _namespaced_test_identity(metadata, identity)
            prior = observed_rows.setdefault(namespaced, status)
            if prior != status:
                problems.append(
                    f"contradictory namespaced Cargo test result: {namespaced!r}"
                )
                continue
            if namespaced in published_rows:
                problems.append(f"duplicate namespaced Cargo test result: {namespaced!r}")
                continue
            published_rows.add(namespaced)
            rows.append(
                {
                    "identity": namespaced,
                    "status": status,
                    "context": namespaced_context,
                }
            )
    return rows, problems


class StreamedCommandResult(NamedTuple):
    returncode: int
    retained_output: str
    cargo_test_artifacts: tuple[dict, ...]
    evidence: dict[str, object]


def run_streamed(
    command: tuple[str, ...],
    *,
    evidence_path: Path,
    retain_cargo_artifacts: bool = False,
    timeout_seconds: float = WORKSPACE_TEST_TIMEOUT_SECONDS,
) -> StreamedCommandResult:
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path = evidence_path.with_suffix(evidence_path.suffix + ".guard.json")
    process = _COMMANDS.start_guarded(
        command,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        timeout=timeout_seconds,
        summary_json=summary_path,
    )
    retained: list[str] = []
    cargo_test_artifacts: list[dict] = []
    digest = hashlib.sha256()
    byte_count = 0
    tail = b""
    compiler_error = False
    assert process.stdout is not None
    stream_error: BaseException | None = None
    wait_error: BaseException | None = None
    returncode = 2
    temporary: Path | None = None
    try:
        handle_context = tempfile.NamedTemporaryFile(
            mode="wb",
            dir=evidence_path.parent,
            prefix=f".{evidence_path.name}.",
            suffix=".tmp",
            delete=False,
        )
        with handle_context as handle:
            temporary = Path(handle.name)
            for line in process.stdout:
                encoded = line.encode("utf-8")
                handle.write(encoded)
                digest.update(encoded)
                byte_count += len(encoded)
                tail = (tail + encoded)[-16_384:]
                compiler_error |= "could not compile" in line or "error[" in line
                if not retain_cargo_artifacts:
                    retained.append(line)
                    continue
                try:
                    payload = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if payload.get("reason") == "compiler-artifact" and payload.get(
                    "profile", {}
                ).get("test"):
                    cargo_test_artifacts.append(
                        {
                            "executable": payload.get("executable"),
                            "package_id": payload.get("package_id"),
                            "target": payload.get("target"),
                        }
                    )
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException as exc:
        stream_error = exc
    finally:
        try:
            returncode = process.wait()
        except BaseException as exc:
            wait_error = exc
        finally:
            close_stdout = getattr(process.stdout, "close", None)
            if callable(close_stdout):
                close_stdout()
            if temporary is not None and temporary.exists():
                os.replace(temporary, evidence_path)
    controller_errors = [
        {
            "stage": stage,
            "type": type(error).__name__,
            "message": str(error),
        }
        for stage, error in (("stream", stream_error), ("wait", wait_error))
        if error is not None
    ]
    if controller_errors:
        returncode = 2
    guard_summary: dict[str, object] | None = None
    if summary_path.is_file():
        try:
            payload = json.loads(summary_path.read_text(encoding="utf-8"))
            if isinstance(payload, dict):
                guard_summary = payload
        except (OSError, json.JSONDecodeError):
            guard_summary = None
    return StreamedCommandResult(
        returncode=returncode,
        retained_output="".join(retained),
        cargo_test_artifacts=tuple(cargo_test_artifacts),
        evidence={
            "path": str(evidence_path),
            "bytes": byte_count,
            "sha256": digest.hexdigest(),
            "tail": tail.decode("utf-8", errors="replace"),
            "contains_compiler_error": compiler_error,
            "timeout_seconds": timeout_seconds,
            "guard_summary": guard_summary,
            "controller_errors": controller_errors,
        },
    )


def write_receipt(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        handle.write(encoded)
        temporary = Path(handle.name)
    os.replace(temporary, path)


def command_termination(returncode: int) -> dict[str, object]:
    if returncode < 0:
        return {
            "kind": "signal",
            "returncode": returncode,
            "signal": -returncode,
        }
    unsigned = returncode & 0xFFFFFFFF
    if unsigned & 0xC0000000 == 0xC0000000:
        return {
            "kind": "windows-exception",
            "returncode": returncode,
            "code": f"0x{unsigned:08X}",
            "raw_code": unsigned,
        }
    return {"kind": "exit", "returncode": returncode}


def terminal_failure_receipt(
    *,
    identity: str,
    run_dir: Path,
    started: datetime,
    context: dict[str, str],
    phases: list[dict],
    problem: str,
    source_identity: dict[str, object] | None = None,
) -> dict:
    finished = datetime.now(timezone.utc)
    return {
        "schema": "molt.cargo-test-truth.v2",
        "run_id": identity,
        "run_directory": str(run_dir),
        "binary_receipt_directory": str(run_dir / "binaries"),
        "started_at": started.isoformat(),
        "finished_at": finished.isoformat(),
        "duration_seconds": round((finished - started).total_seconds(), 3),
        "context": context,
        "source_identity": source_identity,
        "status": "failed",
        "phases": phases,
        "observed_test_count": 0,
        "failed_tests": [],
        "expected_test_binaries": [],
        "problems": [problem],
        "test_binaries": [],
    }


def publish_terminal_failure(
    run_manifest: Path,
    *,
    identity: str,
    run_dir: Path,
    started: datetime,
    context: dict[str, str],
    phases: list[dict],
    problem: str,
    source_identity: dict[str, object] | None = None,
) -> None:
    receipt = terminal_failure_receipt(
        identity=identity,
        run_dir=run_dir,
        started=started,
        context=context,
        phases=phases,
        problem=problem,
        source_identity=source_identity,
    )
    write_receipt(run_manifest, receipt)
    write_receipt(RECEIPT, receipt)


def _main() -> int:
    global _ACTIVE_RUN_TERMINALIZER
    started = datetime.now(timezone.utc)
    identity = run_identity(started)
    run_dir = prepare_run_directory(identity)
    run_manifest = run_dir / "manifest.json"
    binary_receipt_dir = run_dir / "binaries"
    binary_receipt_dir.mkdir()
    source_identity = git_source_identity()
    write_receipt(
        run_manifest,
        {
            "schema": "molt.cargo-test-truth.v2",
            "run_id": identity,
            "started_at": started.isoformat(),
            "status": "running",
            "binary_receipt_directory": str(binary_receipt_dir),
            "source_identity": source_identity,
        },
    )
    context = host_context()
    phases: list[dict] = []
    combined: list[str] = []
    binary_receipts: list[dict] = []
    expected_binaries: dict[str, dict[str, str]] = {}
    package_identities: dict[str, str] = {}
    coverage_problems: list[str] = []
    compiler_error_observed = False
    failed = False

    def terminalize_unfinished_run() -> None:
        try:
            current = json.loads(run_manifest.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            current = {"status": "running"}
        if current.get("status") != "running":
            return
        publish_terminal_failure(
            run_manifest,
            identity=identity,
            run_dir=run_dir,
            started=started,
            context=context,
            phases=phases,
            problem=(
                "Cargo truth controller exited before publishing a terminal verdict; "
                "all completed phase evidence was reconciled"
            ),
            source_identity=source_identity,
        )

    _ACTIVE_RUN_TERMINALIZER = terminalize_unfinished_run
    for workspace, manifest in LOCKED_WORKSPACES:
        command = (
            "cargo",
            "fetch",
            "--locked",
            "--manifest-path",
            str(manifest),
        )
        phase_index = len(phases)
        result = run_streamed(
            command,
            evidence_path=run_dir / "phases" / f"{phase_index:02d}-dependency-prefetch.log",
            retain_cargo_artifacts=True,
            timeout_seconds=FETCH_TIMEOUT_SECONDS,
        )
        returncode = result.returncode
        phase = {
            "kind": "dependency-prefetch",
            "workspace": workspace,
            "manifest": str(manifest.relative_to(ROOT)).replace("\\", "/"),
            "argv": list(command),
            "returncode": returncode,
            "termination": command_termination(returncode),
            "evidence": result.evidence,
        }
        phases.append(phase)
        if returncode != 0:
            publish_terminal_failure(
                run_manifest,
                identity=identity,
                run_dir=run_dir,
                started=started,
                context=context,
                phases=phases,
                problem="Cargo dependency prefetch failed; diagnostics retained in terminal phase",
                source_identity=source_identity,
            )
            failed = True
            break
        metadata_command = (
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version=1",
            "--manifest-path",
            str(manifest),
        )
        phase_index = len(phases)
        metadata_result = run_streamed(
            metadata_command,
            evidence_path=run_dir / "phases" / f"{phase_index:02d}-package-metadata.log",
            timeout_seconds=METADATA_TIMEOUT_SECONDS,
        )
        metadata_returncode = metadata_result.returncode
        metadata_output = metadata_result.retained_output
        phase = {
            "kind": "package-metadata",
            "workspace": workspace,
            "manifest": str(manifest.relative_to(ROOT)).replace("\\", "/"),
            "argv": list(metadata_command),
            "returncode": metadata_returncode,
            "termination": command_termination(metadata_returncode),
            "evidence": metadata_result.evidence,
        }
        phases.append(phase)
        if metadata_returncode != 0:
            publish_terminal_failure(
                run_manifest,
                identity=identity,
                run_dir=run_dir,
                started=started,
                context=context,
                phases=phases,
                problem="Cargo package metadata failed; diagnostics retained in terminal phase",
                source_identity=source_identity,
            )
            failed = True
            break
        try:
            for package_id, package_identity in package_identities_from_metadata(
                metadata_output
            ).items():
                previous = package_identities.setdefault(package_id, package_identity)
                if previous != package_identity:
                    raise RuntimeError(
                        f"Cargo workspaces contradicted package identity {package_id!r}"
                    )
        except RuntimeError as exc:
            output = f"cargo-test-truth-metadata: {exc}\n"
            print(output, end="", file=sys.stderr)
            combined.append(output)
            phase["diagnostic_tail"] = output[-16_384:]
            phase["termination"] = {"kind": "metadata-validation", "returncode": 2}
            phase["returncode"] = 2
            publish_terminal_failure(
                run_manifest,
                identity=identity,
                run_dir=run_dir,
                started=started,
                context=context,
                phases=phases,
                problem="Cargo package identity metadata was invalid",
                source_identity=source_identity,
            )
            failed = True
            break

    target = ""
    if not failed:
        try:
            target = host_target()
            for workspace, manifest in LOCKED_WORKSPACES:
                workspace_receipt_dir = binary_receipt_dir / workspace
                workspace_receipt_dir.mkdir()
                command = (
                    *CANONICAL_COMMAND,
                    "--manifest-path",
                    str(manifest),
                    "--message-format=json-render-diagnostics",
                    "--config",
                    target_runner_config(
                        target,
                        workspace_receipt_dir,
                        identity,
                        source_identity,
                    ),
                )
                phase_index = len(phases)
                workspace_result = run_streamed(
                    command,
                    evidence_path=(
                        run_dir
                        / "phases"
                        / f"{phase_index:02d}-{workspace}-workspace-test.log"
                    ),
                    retain_cargo_artifacts=True,
                    timeout_seconds=WORKSPACE_TEST_TIMEOUT_SECONDS,
                )
                returncode = workspace_result.returncode
                failed |= returncode != 0
                compiler_error_observed |= bool(
                    workspace_result.evidence["contains_compiler_error"]
                )
                workspace_phase = {
                    "kind": "workspace-test",
                    "workspace": workspace,
                    "manifest": str(manifest.relative_to(ROOT)).replace("\\", "/"),
                    "host_target": target,
                    "argv": list(command),
                    "returncode": returncode,
                    "termination": command_termination(returncode),
                    "evidence": workspace_result.evidence,
                    "binary_timeout_seconds": BINARY_TIMEOUT_SECONDS,
                    "binary_receipt_count": 0,
                    "expected_binary_count": 0,
                }
                phases.append(workspace_phase)
                workspace_expected = expected_test_binaries_from_artifacts(
                    workspace_result.cargo_test_artifacts,
                    package_identities,
                    workspace=workspace,
                )
                workspace_receipts = load_binary_receipts(
                    workspace_receipt_dir,
                    expected_run_id=identity,
                    expected_source_identity=source_identity,
                    workspace=workspace,
                )
                coverage_problems.extend(
                    binary_coverage_problems(workspace_expected, workspace_receipts)
                )
                overlap = set(expected_binaries).intersection(workspace_expected)
                if overlap:
                    raise RuntimeError(
                        f"workspace executable authority collided: {sorted(overlap)!r}"
                    )
                expected_binaries.update(workspace_expected)
                binary_receipts.extend(workspace_receipts)
                workspace_phase["binary_receipt_count"] = len(workspace_receipts)
                workspace_phase["expected_binary_count"] = len(workspace_expected)
        except RuntimeError as exc:
            returncode = 2
            failed = True
            output = f"cargo-test-truth-runner: {exc}\n"
            print(output, end="", file=sys.stderr)
            if "workspace_phase" not in locals():
                workspace_phase = {
                    "kind": "workspace-test",
                    "host_target": target,
                    "argv": list(command) if target else list(CANONICAL_COMMAND),
                    "returncode": returncode,
                    "termination": {"kind": "runner-validation", "returncode": returncode},
                    "diagnostic_tail": output[-16_384:],
                    "binary_timeout_seconds": BINARY_TIMEOUT_SECONDS,
                    "binary_receipt_count": len(binary_receipts),
                    "expected_binary_count": len(expected_binaries),
                }
                phases.append(workspace_phase)
            else:
                workspace_phase["returncode"] = returncode
                workspace_phase["termination"] = {
                    "kind": "runner-validation",
                    "returncode": returncode,
                }
                workspace_phase["diagnostic_tail"] = output[-16_384:]
            combined.append(output)

    output = "".join(combined)
    if compiler_error_observed:
        output += "\nerror[compiler-diagnostic-retained-in-phase-evidence]\n"
    rows, _receipt_row_problems = receipt_test_rows(
        binary_receipts, expected_binaries, context
    )
    failures = sorted(row["identity"] for row in rows if row.get("status") == "fail")
    problems = verdict(
        output,
        1 if failed else 0,
        context,
        binary_receipts=binary_receipts,
        expected_binaries=expected_binaries,
    ) + coverage_problems
    finished = datetime.now(timezone.utc)
    receipt = {
        "schema": "molt.cargo-test-truth.v2",
        "run_id": identity,
        "run_directory": str(run_dir),
        "started_at": started.isoformat(),
        "finished_at": finished.isoformat(),
        "duration_seconds": round((finished - started).total_seconds(), 3),
        "context": context,
        "source_identity": source_identity,
        "status": "success" if not problems else "failed",
        "phases": phases,
        "observed_test_count": len(rows),
        "failed_tests": failures,
        "expected_test_binaries": [
            expected_binaries[key] for key in sorted(expected_binaries)
        ],
        "problems": problems,
        "test_binaries": binary_receipts,
    }
    write_receipt(run_manifest, receipt)
    write_receipt(RECEIPT, receipt)
    print(f"cargo-test-truth-runner: run-manifest={run_manifest}")
    print(f"cargo-test-truth-runner: receipt={RECEIPT}")
    if problems:
        print("cargo-test-truth-runner: FAIL", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1
    print("cargo-test-truth-runner: OK (exact registered red set)")
    return 0


def main() -> int:
    global _ACTIVE_RUN_TERMINALIZER
    try:
        return _main()
    finally:
        terminalizer = _ACTIVE_RUN_TERMINALIZER
        _ACTIVE_RUN_TERMINALIZER = None
        if callable(terminalizer):
            terminalizer()


if __name__ == "__main__":
    raise SystemExit(main())
