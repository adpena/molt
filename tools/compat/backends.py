"""Backend adapter registry for the multi-backend parity oracle (doc 66 FACT 2).

doc 66 §1.1 names the critical structural gap: tests/molt_diff.py is
SINGLE-BACKEND (native). It has `--build-profile` but no `--target`, so a
backend-specific divergence — where wasm/llvm/luau produces a different answer
than native/CPython — is INVISIBLE. The pre-existing response was a *separate*
runner per backend (tools/wasm_diff.py), each hand-reimplementing the verdict
loop, which is the very dual-truth the project forbids.

This module is the structural fix's load-bearing half: a backend adapter is the
ONE thing that differs between backends — "given a .py file, produce
(stdout, stderr, returncode) for THIS backend". Everything downstream (the
CPython oracle, the `# MOLT_META` gating, the comparison law in
tools/compat/comparison.py, the cross-backend divergence sub-oracle) is
backend-independent and lives once in molt_diff.diff_test.

Adapters:
  * native — delegates to molt_diff's rich build+run+RSS+retry machinery (passed
    in as a callable to avoid an import cycle); this is the ONLY backend that
    keeps the daemon/dyld/RSS pipeline, because that pipeline is native-shaped.
  * wasm   — `molt build --target wasm` (linked) + the canonical node host shim
    `wasm/run_wasm.js` (lifted from wasm_diff.py, including node-noise stripping).
  * llvm   — `molt build --target llvm` (emits a native binary) + run the binary
    under the shared memory guard. Available-gated on the LLVM toolchain.
  * luau   — `molt build --target luau` + `lune run <out>.luau`. Available-gated
    on the `lune` runtime.

Availability is detected once and a missing toolchain yields a LOUD `uncalibrated`
outcome (never a silent skip, never a false pass) — doc 66 FACT 1's `uncalibrated`
cell semantics.

Fault injection (test seam, not a workaround): when MOLT_COMPAT_FAULT_INJECT
names a backend (e.g. "wasm"), that backend's adapter perturbs its stdout
deterministically. This is how the cross-backend-divergence proof injects a
synthetic per-backend wrong answer to witness the oracle going RED, then reverts.
It lives at the adapter boundary precisely so it cannot leak into the comparison
law or the real backends' codegen.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import hashlib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, replace
from pathlib import Path
from tools.memory_guard_core import harness_outcomes
from types import MappingProxyType
from typing import TYPE_CHECKING, Callable, Literal, Protocol

if TYPE_CHECKING:
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    verify_available_llvm_toolchain,
)
from molt.dx import scratch_dir
from molt.target_python import TargetPythonVersion, _parse_target_python_version
from molt.wasm_artifact import wasm_runtime_manifest_path
from tools.compat import diff_output_layout, test_policy

# molt_diff is imported by the harness before adapters are used; we import the
# already-bootstrapped module here for its capability/CLI-python helpers so the
# wasm/llvm/luau build commands match the native lane's environment exactly.
# The native adapter does NOT import run_molt directly — it is injected — so this
# module never forces a circular import at module-load time.
_REPO_ROOT = Path(__file__).resolve().parents[2]


# The canonical wasm host shim (also used by wasm_run_matrix.py).
# wasmtime/wasmer cannot satisfy the env.molt_*_host imports by design, so
# node is the supported runner for a Molt wasm module.
_RUN_WASM_JS = _REPO_ROOT / "wasm" / "run_wasm.js"


# ---------------------------------------------------------------------------
# Backend result + protocol
# ---------------------------------------------------------------------------


# Only complete allocator/exception diagnostics establish unmeasured exhaustion.
# Command lines, environment values, source excerpts and guard repro JSON are
# context, not allocation observations. In particular, neither SIGKILL nor an
# arbitrary occurrence of "oom" is an OOM verdict.
_ALLOCATION_FAILURE_LINE = re.compile(
    r"(?:MemoryError(?::[^\r\n]*)?"
    r"|(?:OOM|out of memory|cannot allocate memory|allocation failed)(?:: [^\r\n]*)?"
    r"|std::bad_alloc"
    r"|[ \t]*what\(\):[ \t]+std::bad_alloc"
    r"|terminate called after throwing an instance of ['\"]std::bad_alloc['\"]"
    r"|memory allocation (?:of )?[0-9]+ bytes failed"
    r"|LLVM ERROR: out of memory(?::[^\r\n]*)?"
    r"|FATAL ERROR: [^\r\n]*Allocation failed[^\r\n]*heap out of memory"
    r"|OSError: \[Errno 12\] Cannot allocate memory[^\r\n]*"
    r"|RuntimeError: (?:memory allocation failed|out of memory)(?::[^\r\n]*)?"
    r")",
    re.IGNORECASE,
)


@dataclass(frozen=True)
class BackendResult:
    """One backend's outcome for a single test.

    `stdout` is None when the backend never produced output (build failed before
    execution). `build_failed` distinguishes a build failure from a run that
    produced empty stdout, so diff_test can mirror its existing "Molt failed to
    build" branch (the CPython-compile-error parity case).
    """

    stdout: str | None
    stderr: str
    returncode: int
    build_failed: bool = False
    detail: str = ""
    timed_out: bool = False
    child_returncode: int | None = None
    infrastructure_failure: GuardInfrastructureFailure | None = None
    rss_limit_exceeded: bool = False
    guard_signal: int | None = None
    child_stderr: str | None = None

    def __post_init__(self) -> None:
        # Preserve the child stream before adapters append build stdout,
        # deadline prose, cleanup messages or reproduction instructions.
        if self.child_stderr is None:
            object.__setattr__(self, "child_stderr", self.stderr)

    @property
    def resource_failure(
        self,
    ) -> Literal["rss_limit_exceeded", "allocation_failed"] | None:
        if self.infrastructure_failure is not None:
            return None
        if self.rss_limit_exceeded:
            return "rss_limit_exceeded"
        if (
            self.timed_out
            or self.guard_signal is not None
            or self.returncode in (0, 124)
        ):
            return None
        lines = (self.child_stderr or "").splitlines()
        # An ownership/header abort must remain visible even if cleanup also
        # reports an allocation exception. Measured RSS remains authoritative.
        if any(line.startswith("molt fatal:") for line in lines):
            return None
        # A guarded compiler nested inside a CLI/batch server reports its RSS
        # observation in the child stream. Accept that canonical diagnostic,
        # never the guard's reproduction/environment/ancestry payload.
        if any(line.startswith("memory_guard: RSS limit exceeded; ") for line in lines):
            return "rss_limit_exceeded"
        if any(_ALLOCATION_FAILURE_LINE.fullmatch(line) for line in lines):
            return "allocation_failed"
        return None

    @property
    def blocks_build_recovery(self) -> bool:
        return (
            self.timed_out
            or self.infrastructure_failure is not None
            or self.guard_signal is not None
            or self.resource_failure is not None
        )

    @classmethod
    def from_process(
        cls,
        proc: subprocess.CompletedProcess[str]
        | subprocess.CompletedProcess[bytes]
        | BackendResult,
    ) -> BackendResult:
        if isinstance(proc, cls):
            return proc
        return cls(
            None if proc.stdout is None else cls._text(proc.stdout),
            cls._text(proc.stderr),
            proc.returncode,
            timed_out=bool(getattr(proc, "timed_out", False)),
            child_returncode=getattr(proc, "child_returncode", None),
            infrastructure_failure=getattr(proc, "infrastructure_failure", None),
            rss_limit_exceeded=getattr(proc, "violation", None) is not None,
            guard_signal=getattr(proc, "guard_signal", None),
            child_stderr=cls._text(getattr(proc, "child_stderr", proc.stderr)),
        )

    @staticmethod
    def _text(value: str | bytes | None) -> str:
        return (
            value.decode("utf-8", errors="surrogateescape")
            if isinstance(value, bytes)
            else value or ""
        )

    @classmethod
    def from_deadline(
        cls,
        *,
        timeout: float | int,
        stdout: str | bytes | None = None,
        stderr: str | bytes | None = None,
        build_failed: bool = False,
    ) -> BackendResult:
        out_text = cls._text(stdout)
        err_text = cls._text(stderr)
        child_stderr = err_text
        deadline = f"timeout after {timeout}s"
        err_text = "\n".join(part for part in (err_text, deadline) if part)
        return cls(
            stdout=None if build_failed else out_text,
            stderr=(
                "\n".join(part for part in (out_text, err_text) if part)
                if build_failed
                else err_text
            ),
            returncode=124,
            build_failed=build_failed,
            timed_out=True,
            child_stderr=child_stderr,
        )

    def as_build_failure(self, *, detail: str, fallback: str) -> BackendResult:
        diagnostics = "\n".join(
            part for part in (self.stdout or "", self.stderr) if part
        )
        return replace(
            self,
            stdout=None,
            stderr=diagnostics or fallback,
            returncode=self.returncode if self.returncode != 0 else 1,
            build_failed=True,
            detail=detail,
        )


@dataclass(frozen=True)
class BackendAvailability:
    available: bool
    reason: str = ""


def stdlib_profile_from_environment(
    environment: Mapping[str, str],
) -> Literal["micro", "full"] | None:
    """Resolve the differential selector once for every execution transport."""
    raw = environment.get("MOLT_DIFF_STDLIB_PROFILE", "").strip().lower()
    if not raw:
        return None
    if raw == "micro":
        return "micro"
    if raw == "full":
        return "full"
    raise ValueError("MOLT_DIFF_STDLIB_PROFILE must be 'micro' or 'full'")


@dataclass(frozen=True, slots=True)
class BackendExecutionContext:
    """One immutable compiler/runtime policy shared by every backend."""

    target_python: TargetPythonVersion
    build_profile: str
    capabilities: str
    environment: Mapping[str, str]
    stdlib_profile: Literal["micro", "full"] | None = field(init=False)

    def __post_init__(self) -> None:
        if not isinstance(self.target_python, TargetPythonVersion):
            raise TypeError("backend target Python must use TargetPythonVersion")
        canonical_target = _parse_target_python_version(self.target_python.short)
        if self.target_python != canonical_target:
            raise ValueError("backend target Python must be canonical")
        if any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in self.environment.items()
        ):
            raise TypeError("backend execution environment must be string-to-string")
        object.__setattr__(
            self, "stdlib_profile", stdlib_profile_from_environment(self.environment)
        )
        object.__setattr__(
            self,
            "environment",
            MappingProxyType(dict(sorted(self.environment.items()))),
        )


class BackendAdapter(Protocol):
    """A backend knows how to (a) report availability and (b) build+run a file."""

    name: str

    def availability(self) -> BackendAvailability: ...

    def build_and_run(
        self,
        file_path: str,
        *,
        context: BackendExecutionContext,
    ) -> BackendResult: ...


# ---------------------------------------------------------------------------
# Fault injection (test seam for the cross-backend-divergence proof)
# ---------------------------------------------------------------------------


def _fault_injection_targets() -> set[str]:
    raw = os.environ.get("MOLT_COMPAT_FAULT_INJECT", "").strip()
    if not raw:
        return set()
    return {tok.strip().lower() for tok in raw.split(",") if tok.strip()}


def _apply_fault_injection(backend: str, result: BackendResult) -> BackendResult:
    """Deterministically perturb a backend's stdout when fault-injected.

    Applied at exactly ONE layer — the harness's per-backend boundary
    (molt_diff._run_backend_for_diff) — uniformly for every backend, so adapters
    stay pure build+run. Used ONLY by the divergence-catch proof to inject a
    synthetic per-backend wrong answer: it appends a stable marker line so the
    perturbed backend diverges from CPython AND from the other backends,
    exercising both the per-backend-vs-CPython and cross-backend checks. Inert
    unless MOLT_COMPAT_FAULT_INJECT names this backend.
    """
    if (
        result.infrastructure_failure is not None
        or backend.lower() not in _fault_injection_targets()
    ):
        return result
    if result.stdout is None:
        # Even a build failure becomes a visible, distinct divergence so the
        # proof can witness the RED regardless of where the fault lands.
        return replace(
            result,
            stdout=f"<MOLT_COMPAT_FAULT_INJECT::{backend}>\n",
            build_failed=False,
            detail="fault-injected stdout (was build failure)",
        )
    perturbed = result.stdout + f"<MOLT_COMPAT_FAULT_INJECT::{backend}>\n"
    return replace(
        result,
        stdout=perturbed,
        detail="fault-injected stdout",
    )


# ---------------------------------------------------------------------------
# Native adapter — delegates to molt_diff's rich build+run machinery
# ---------------------------------------------------------------------------

# The native build+run path is deeply tied to molt_diff's daemon custody, RSS
# measurement, dyld-retry pipeline and build-lock pruning. Rather than fork that
# machinery, the native adapter is constructed with a callable that runs it
# (molt_diff.run_molt), keeping the single rich implementation as the source of
# truth and this module free of an import cycle.
RunMoltCallable = Callable[..., BackendResult]


@dataclass
class NativeAdapter:
    """Native backend: the existing molt_diff build+run path, unchanged."""

    run_molt: RunMoltCallable
    name: str = "native"

    def availability(self) -> BackendAvailability:
        # Native is always available in the differential harness — it is the
        # backend molt_diff has always driven.
        return BackendAvailability(available=True)

    def build_and_run(
        self,
        file_path: str,
        *,
        context: BackendExecutionContext,
    ) -> BackendResult:
        return self.run_molt(
            file_path,
            context.build_profile,
            execution_context=context,
        )


# ---------------------------------------------------------------------------
# Shared cross-backend build/run helpers
# ---------------------------------------------------------------------------


def _molt_cli_python() -> str:
    import molt_diff  # bootstrapped by the harness before adapters run

    return molt_diff._resolve_molt_cli_python()


def suite_trip_outcome(
    evidence: harness_outcomes.SuiteTripEvidence | None,
) -> BackendResult | None:
    """Suite-level admission/summary outcome, with no claimed child execution."""
    if evidence is None:
        return None
    return BackendResult(
        "",
        evidence.message + "\n",
        harness_outcomes.memory_guard.INFRASTRUCTURE_RETURN_CODE
        if evidence.infrastructure_failure is not None
        else harness_outcomes.memory_guard.GUARD_RETURN_CODE,
        infrastructure_failure=evidence.infrastructure_failure,
        rss_limit_exceeded=bool(evidence.trips),
        child_stderr="",
    )


def merge_suite_trip_result(
    result: BackendResult,
    process: object,
    evidence: harness_outcomes.SuiteTripEvidence | None,
) -> BackendResult:
    """Attribute a suite kill only to the captured launch instance it targeted.

    A malformed marker belongs to the parent suite outcome, not an unrelated
    child's diagnostics. A child that completed normally retains that success.
    Existing child deadline/signal/infrastructure observations keep precedence.
    """
    if (
        evidence is None
        or evidence.infrastructure_failure is not None
        or result.timed_out
        or result.guard_signal is not None
    ):
        return result
    child_returncode = getattr(process, "child_returncode", None)
    if child_returncode is None:
        child_returncode = getattr(process, "returncode", None)
    if child_returncode in {None, 0}:
        return result
    child = getattr(process, "child_process", None)
    owned = getattr(process, "owned_process_identities", ())
    matched = tuple(
        trip
        for trip in evidence.trips
        if trip.matches(
            child,
            owned,
            request_started_at_ns=getattr(process, "request_started_at_ns", None),
        )
    )
    if not matched:
        return result
    message = "\n".join(dict.fromkeys(trip.details for trip in matched)) + "\n"
    return replace(
        result,
        returncode=result.returncode
        if result.infrastructure_failure is not None
        else harness_outcomes.memory_guard.GUARD_RETURN_CODE,
        stderr=result.stderr if message in result.stderr else result.stderr + message,
        rss_limit_exceeded=True,
    )


def _guarded_run(
    cmd: list[str],
    *,
    prefix: str,
    env: dict[str, str],
    timeout_default: float,
    cwd: str | None = None,
) -> BackendResult:
    """Run every cross-backend build/artifact under the shared typed guard."""
    from tools import harness_memory_guard

    trip = suite_trip_outcome(harness_outcomes.read_suite_trip(env))
    if trip is not None:
        return trip
    timeout = harness_memory_guard.timeout_from_env(
        prefix, env, default=timeout_default
    )
    proc = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=False,
        timeout=timeout,
    )
    return merge_suite_trip_result(
        BackendResult.from_process(proc), proc, harness_outcomes.read_suite_trip(env)
    )


def _cross_build_env(context: BackendExecutionContext) -> dict[str, str]:
    env = dict(context.environment)
    selected = diff_output_layout.enforce_child(env, repo_root=_REPO_ROOT)
    if selected is not None:
        for name in ("TMPDIR", "TEMP", "TMP"):
            env[name] = str(selected / "guest-tmp")
    env["PYTHONHASHSEED"] = "0"
    if context.capabilities:
        env["MOLT_CAPABILITIES"] = context.capabilities
    return env


def _build_cmd(
    file_path: str,
    target: str,
    out_dir: Path,
    context: BackendExecutionContext,
    *,
    extra_build_args: list[str] | None = None,
) -> list[str]:
    cmd = [
        _molt_cli_python(),
        "-m",
        "molt.cli",
        "build",
        file_path,
        "--target",
        target,
        "--build-profile",
        context.build_profile,
        "--python-version",
        context.target_python.short,
        "--respect-pythonpath",
        "--out-dir",
        str(out_dir),
    ]
    if context.capabilities:
        cmd.extend(["--capabilities", context.capabilities])
    if context.stdlib_profile is not None:
        cmd.extend(["--stdlib-profile", context.stdlib_profile])
    if extra_build_args:
        cmd.extend(extra_build_args)
    return cmd


def _with_adapter_scratch(
    backend: str,
    file_path: str,
    run: Callable[[Path], BackendResult],
    *,
    environment: Mapping[str, str],
) -> BackendResult:
    if backend not in {"wasm", "llvm", "luau"}:
        raise ValueError("unknown adapter scratch namespace")
    npath = test_policy.normalize_repo_relative(file_path)
    environment = dict(environment)
    run_id = environment.get("MOLT_DIFF_RUN_ID", "").strip() or "adhoc"
    run_key = hashlib.sha256(run_id.encode()).hexdigest()[:16]
    file_key = hashlib.sha256(npath.encode()).hexdigest()[:16]
    root = _cross_scratch_root(environment)
    lease = diff_output_layout.new_guest_leaf(
        root / backend,
        prefix=f"{run_key}-{file_key}-",
        boundary=root,
        environment=environment,
        repo_root=_REPO_ROOT,
    )
    return run_with_guest_outputs(
        [lease], lambda: run(lease.path), environment=environment, repo_root=_REPO_ROOT
    )


def run_with_guest_outputs(
    leases: Sequence[diff_output_layout.GuestOutputLease],
    run: Callable[[], BackendResult],
    *,
    environment: Mapping[str, str],
    repo_root: Path,
    keep: bool | None = None,
) -> BackendResult:
    """Retire owned leases in reverse order without replacing guest evidence.

    A runner may append a newly acquired lease to the supplied list during
    setup. Both setup failures and returned outcomes drain that same list.
    """
    if keep is None:
        keep = diff_output_layout.keep_artifacts(environment)

    def retire() -> tuple[str, ...]:
        diagnostics = []
        for lease in reversed(leases):
            diagnostic = lease.retire(
                environment=environment, repo_root=repo_root, keep=keep
            )
            if diagnostic:
                diagnostics.append(diagnostic)
        return tuple(diagnostics)

    try:
        result = run()
    except BaseException as error:
        for diagnostic in retire():
            error.add_note(diagnostic)
        raise
    diagnostics = retire()
    if not diagnostics:
        return result
    from tools.memory_guard_core.process_custody import GuardInfrastructureFailure

    existing = result.infrastructure_failure
    return replace(
        result,
        infrastructure_failure=GuardInfrastructureFailure(
            phase=existing.phase
            if existing is not None
            else "temporary_artifact_custody",
            details=(*existing.details, *diagnostics)
            if existing is not None
            else diagnostics,
        ),
        detail="\n".join(part for part in (result.detail, *diagnostics) if part),
    )


def _cross_scratch_root(environment: Mapping[str, str]) -> Path:
    selected = diff_output_layout.selected_for_compat(environment, repo_root=_REPO_ROOT)
    if selected is not None:
        root = selected / "compat-scratch"
        root.mkdir(parents=True, exist_ok=True)
        return root
    raw = environment.get("MOLT_COMPAT_SCRATCH_ROOT", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        root = scratch_dir(_REPO_ROOT, "compat_backends", environment)
    root.mkdir(parents=True, exist_ok=True)
    return root


def _is_compile_error(err: str) -> bool:
    return any(tag in err for tag in ("SyntaxError", "IndentationError", "TabError"))


# ---------------------------------------------------------------------------
# WASM adapter
# ---------------------------------------------------------------------------


@dataclass
class WasmAdapter:
    name: str = "wasm"

    def availability(self) -> BackendAvailability:
        if shutil.which("node") is None:
            return BackendAvailability(
                available=False,
                reason="node not on PATH (canonical wasm host shim "
                "wasm/run_wasm.js requires node)",
            )
        if not _RUN_WASM_JS.exists():
            return BackendAvailability(
                available=False, reason=f"missing wasm host shim {_RUN_WASM_JS}"
            )
        return BackendAvailability(available=True)

    def build_and_run(
        self,
        file_path: str,
        *,
        context: BackendExecutionContext,
    ) -> BackendResult:
        return _with_adapter_scratch(
            self.name,
            file_path,
            lambda out_dir: self._build_and_run_owned(
                file_path, context=context, out_dir=out_dir
            ),
            environment=context.environment,
        )

    def _build_and_run_owned(
        self, file_path: str, *, context: BackendExecutionContext, out_dir: Path
    ) -> BackendResult:
        env = _cross_build_env(context)
        # Build a linked module so the canonical node shim can run it directly.
        env.setdefault("MOLT_WASM_LINKED", "1")
        cmd = _build_cmd(
            file_path,
            "wasm",
            out_dir,
            context,
            extra_build_args=["--linked", "--require-linked"],
        )
        build = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_WASM_BUILD",
            env=env,
            timeout_default=600.0,
            cwd=str(_REPO_ROOT),
        )
        linked = out_dir / "output_linked.wasm"
        if not linked.exists():
            linked = out_dir / "output.wasm"
        if build.returncode != 0 or not linked.exists():
            return build.as_build_failure(
                detail="wasm build produced no linked module",
                fallback="wasm build failed",
            )
        run_env = dict(env)
        manifest = wasm_runtime_manifest_path(linked)
        return _guarded_run(
            [shutil.which("node") or "node", str(_RUN_WASM_JS), str(manifest)],
            prefix="MOLT_COMPAT_WASM_RUN",
            env=run_env,
            timeout_default=60.0,
            cwd=str(_REPO_ROOT),
        )


# ---------------------------------------------------------------------------
# LLVM adapter
# ---------------------------------------------------------------------------


@dataclass
class LlvmAdapter:
    name: str = "llvm"

    def availability(self) -> BackendAvailability:
        try:
            verification = verify_available_llvm_toolchain(_REPO_ROOT)
        except LlvmToolchainConfigError as exc:
            return BackendAvailability(available=False, reason=str(exc))
        if verification is not None:
            return BackendAvailability(available=True)
        return BackendAvailability(
            available=False,
            reason="complete manifest-compatible LLVM/MLIR/LLD/Polly SDK not found",
        )

    def build_and_run(
        self,
        file_path: str,
        *,
        context: BackendExecutionContext,
    ) -> BackendResult:
        return _with_adapter_scratch(
            self.name,
            file_path,
            lambda out_dir: self._build_and_run_owned(
                file_path, context=context, out_dir=out_dir
            ),
            environment=context.environment,
        )

    def _build_and_run_owned(
        self, file_path: str, *, context: BackendExecutionContext, out_dir: Path
    ) -> BackendResult:
        env = _cross_build_env(context)

        stem = Path(file_path).stem
        output_binary = out_dir / f"{stem}_molt"
        cmd = _build_cmd(
            file_path,
            "llvm",
            out_dir,
            context,
            extra_build_args=["--emit", "bin", "--output", str(output_binary)],
        )
        build = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_LLVM_BUILD",
            env=env,
            timeout_default=900.0,
            cwd=str(_REPO_ROOT),
        )
        binary = output_binary
        if not binary.exists() and (out_dir / f"{stem}_molt.exe").exists():
            binary = out_dir / f"{stem}_molt.exe"
        if build.returncode != 0 or not binary.exists():
            return build.as_build_failure(
                detail="llvm build produced no binary",
                fallback="llvm build failed",
            )
        return _guarded_run(
            [str(binary)],
            prefix="MOLT_COMPAT_LLVM_RUN",
            env=env,
            timeout_default=60.0,
            cwd=str(_REPO_ROOT),
        )


# ---------------------------------------------------------------------------
# Luau adapter
# ---------------------------------------------------------------------------


@dataclass
class LuauAdapter:
    name: str = "luau"

    def availability(self) -> BackendAvailability:
        if shutil.which("lune") is None:
            return BackendAvailability(
                available=False,
                reason="lune not on PATH (Molt luau output runs under the lune "
                "runtime; install via `cargo install lune`)",
            )
        return BackendAvailability(available=True)

    def build_and_run(
        self,
        file_path: str,
        *,
        context: BackendExecutionContext,
    ) -> BackendResult:
        return _with_adapter_scratch(
            self.name,
            file_path,
            lambda out_dir: self._build_and_run_owned(
                file_path, context=context, out_dir=out_dir
            ),
            environment=context.environment,
        )

    def _build_and_run_owned(
        self, file_path: str, *, context: BackendExecutionContext, out_dir: Path
    ) -> BackendResult:
        env = _cross_build_env(context)
        stem = Path(file_path).stem
        cmd = _build_cmd(file_path, "luau", out_dir, context)
        build = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_LUAU_BUILD",
            env=env,
            timeout_default=600.0,
            cwd=str(_REPO_ROOT),
        )
        luau_out = out_dir / f"{stem}.luau"
        if not luau_out.exists():
            # Some layouts name it output.luau; accept either.
            alt = out_dir / "output.luau"
            if alt.exists():
                luau_out = alt
        if build.returncode != 0 or not luau_out.exists():
            return build.as_build_failure(
                detail="luau build produced no .luau source",
                fallback="luau build failed",
            )
        return _guarded_run(
            [shutil.which("lune") or "lune", "run", str(luau_out)],
            prefix="MOLT_COMPAT_LUAU_RUN",
            env=env,
            timeout_default=60.0,
            cwd=str(_REPO_ROOT),
        )


# ---------------------------------------------------------------------------
# Registry
# ---------------------------------------------------------------------------

#: Re-export the test-policy authority for existing command-line consumers.
ALL_BACKENDS = test_policy.ALL_BACKENDS


def build_registry(run_molt: RunMoltCallable) -> dict[str, BackendAdapter]:
    """Build the adapter registry, wiring the native adapter to molt_diff.run_molt.

    `run_molt` is injected (not imported) so this module has no import cycle with
    tests/molt_diff.py.
    """
    return {
        "native": NativeAdapter(run_molt=run_molt),
        "wasm": WasmAdapter(),
        "llvm": LlvmAdapter(),
        "luau": LuauAdapter(),
    }


def normalize_targets(raw_targets: list[str]) -> list[str]:
    """Resolve a user --target list into a deduplicated, ordered backend list.

    Accepts the alias "all" (expands to ALL_BACKENDS) and rejects unknown
    backends loudly (fail-closed — never silently drop a requested backend).
    """
    out: list[str] = []
    for raw in raw_targets:
        for tok in str(raw).split(","):
            name = tok.strip().lower()
            if not name:
                continue
            if name == "all":
                for b in ALL_BACKENDS:
                    if b not in out:
                        out.append(b)
                continue
            if name not in ALL_BACKENDS:
                raise ValueError(
                    f"unknown --target backend {name!r}; "
                    f"choose from {list(ALL_BACKENDS)} or 'all'"
                )
            if name not in out:
                out.append(name)
    return out or ["native"]
