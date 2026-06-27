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
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Protocol

# molt_diff is imported by the harness before adapters are used; we import the
# already-bootstrapped module here for its capability/CLI-python helpers so the
# wasm/llvm/luau build commands match the native lane's environment exactly.
# The native adapter does NOT import run_molt directly — it is injected — so this
# module never forces a circular import at module-load time.
_REPO_ROOT = Path(__file__).resolve().parents[2]


# The canonical wasm host shim (same one tools/wasm_diff.py + wasm_run_matrix.py
# use). wasmtime/wasmer cannot satisfy the env.molt_*_host imports by design, so
# node is the supported runner for a Molt wasm module.
_RUN_WASM_JS = _REPO_ROOT / "wasm" / "run_wasm.js"


# ---------------------------------------------------------------------------
# Backend result + protocol
# ---------------------------------------------------------------------------


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


@dataclass(frozen=True)
class BackendAvailability:
    available: bool
    reason: str = ""


class BackendAdapter(Protocol):
    """A backend knows how to (a) report availability and (b) build+run a file."""

    name: str

    def availability(self) -> BackendAvailability: ...

    def build_and_run(
        self,
        file_path: str,
        build_profile: str,
        *,
        extra_env: dict[str, str] | None,
        capabilities: str,
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
    if backend.lower() not in _fault_injection_targets():
        return result
    if result.stdout is None:
        # Even a build failure becomes a visible, distinct divergence so the
        # proof can witness the RED regardless of where the fault lands.
        return BackendResult(
            stdout=f"<MOLT_COMPAT_FAULT_INJECT::{backend}>\n",
            stderr=result.stderr,
            returncode=result.returncode,
            build_failed=False,
            detail="fault-injected stdout (was build failure)",
        )
    perturbed = result.stdout + f"<MOLT_COMPAT_FAULT_INJECT::{backend}>\n"
    return BackendResult(
        stdout=perturbed,
        stderr=result.stderr,
        returncode=result.returncode,
        build_failed=result.build_failed,
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
RunMoltCallable = Callable[..., "tuple[str | None, str, int]"]


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
        build_profile: str,
        *,
        extra_env: dict[str, str] | None,
        capabilities: str,
    ) -> BackendResult:
        stdout, stderr, rc = self.run_molt(
            file_path,
            build_profile,
            extra_env=extra_env,
        )
        return BackendResult(
            stdout=stdout,
            stderr=stderr,
            returncode=rc,
            build_failed=stdout is None,
        )


# ---------------------------------------------------------------------------
# Shared cross-backend build/run helpers
# ---------------------------------------------------------------------------


def _molt_cli_python() -> str:
    import molt_diff  # bootstrapped by the harness before adapters run

    return molt_diff._resolve_molt_cli_python()


def _guarded_run(
    cmd: list[str],
    *,
    prefix: str,
    env: dict[str, str],
    timeout: float,
    cwd: str | None = None,
) -> tuple[str, str, int, bool]:
    """Run a child under the shared harness memory guard.

    Returns (stdout, stderr, returncode, timed_out). Every cross-backend build
    and artifact run goes through the guard so a runaway compile or program can
    never OOM/hang the host (CLAUDE.md Safe Execution).
    """
    from tools import harness_memory_guard

    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=prefix,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", f"timeout after {timeout}s", 124, True
    timed_out = bool(getattr(proc, "timed_out", False))
    return proc.stdout, proc.stderr, proc.returncode, timed_out


def _cross_build_env(extra_env: dict[str, str] | None, capabilities: str) -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHONHASHSEED"] = "0"
    if capabilities:
        env.setdefault("MOLT_DIFF_CAPABILITIES", capabilities)
        env.setdefault("MOLT_CAPABILITIES", capabilities)
    if extra_env:
        env.update(extra_env)
    return env


def _build_cmd(
    file_path: str,
    target: str,
    build_profile: str,
    out_dir: Path,
    capabilities: str,
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
        build_profile,
        "--respect-pythonpath",
        "--out-dir",
        str(out_dir),
    ]
    if capabilities:
        cmd.extend(["--capabilities", capabilities])
    if extra_build_args:
        cmd.extend(extra_build_args)
    return cmd


def _build_timeout(env_key: str, default: float) -> float:
    raw = os.environ.get(env_key, "").strip()
    try:
        v = float(raw)
        if v > 0:
            return v
    except ValueError:
        pass
    return default


def _scratch_dir(backend: str, file_path: str) -> Path:
    import molt_diff

    npath = molt_diff._normalize_repo_relative(file_path)
    safe = npath.replace("/", "__").replace("\\", "__").replace(".py", "")
    root = _cross_scratch_root() / backend
    out_dir = root / safe
    out_dir.mkdir(parents=True, exist_ok=True)
    return out_dir


def _cross_scratch_root() -> Path:
    raw = os.environ.get("MOLT_COMPAT_SCRATCH_ROOT", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        ext_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
        base = Path(ext_root).expanduser() if ext_root else _REPO_ROOT
        root = base / "tmp" / "compat_backends"
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
        build_profile: str,
        *,
        extra_env: dict[str, str] | None,
        capabilities: str,
    ) -> BackendResult:
        out_dir = _scratch_dir(self.name, file_path)
        env = _cross_build_env(extra_env, capabilities)
        # Build a linked module so the canonical node shim can run it directly.
        env.setdefault("MOLT_WASM_LINKED", "1")
        cmd = _build_cmd(
            file_path,
            "wasm",
            build_profile,
            out_dir,
            capabilities,
            extra_build_args=["--linked"],
        )
        build_timeout = _build_timeout("MOLT_COMPAT_WASM_BUILD_TIMEOUT", 600.0)
        b_out, b_err, b_rc, b_to = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_WASM",
            env=env,
            timeout=build_timeout,
            cwd=str(_REPO_ROOT),
        )
        linked = out_dir / "output_linked.wasm"
        if not linked.exists():
            linked = out_dir / "output.wasm"
        if b_rc != 0 or not linked.exists():
            return BackendResult(
                stdout=None,
                stderr=(b_err or b_out or "wasm build failed"),
                returncode=b_rc if b_rc != 0 else 1,
                build_failed=True,
                detail="wasm build produced no linked module",
            )
        run_env = dict(env)
        run_env["MOLT_WASM_PREFER_LINKED"] = "1"
        runtime_wasm = out_dir / "molt_runtime.wasm"
        if runtime_wasm.exists():
            run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
        run_timeout = _build_timeout("MOLT_COMPAT_WASM_RUN_TIMEOUT", 60.0)
        r_out, r_err, r_rc, r_to = _guarded_run(
            [shutil.which("node") or "node", str(_RUN_WASM_JS), str(linked)],
            prefix="MOLT_COMPAT_WASM",
            env=run_env,
            timeout=run_timeout,
            cwd=str(_REPO_ROOT),
        )
        r_err = _strip_node_noise(r_err)
        return BackendResult(
            stdout=r_out,
            stderr=r_err,
            returncode=r_rc,
            build_failed=False,
        )


def _wasm_stderr_is_noise(line: str) -> bool:
    """Node emits an unconditional WASI ExperimentalWarning that is not a program
    diagnostic; strip it so stderr comparison is fair (lifted from wasm_diff.py)."""
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


# ---------------------------------------------------------------------------
# LLVM adapter
# ---------------------------------------------------------------------------


@dataclass
class LlvmAdapter:
    name: str = "llvm"

    def availability(self) -> BackendAvailability:
        # The LLVM backend links a native binary via llvm-config + the system
        # toolchain. Probe for llvm-config (or an LLVM_SYS prefix) the way the
        # CLI's readiness check does; a missing toolchain is a LOUD uncalibrated.
        if shutil.which("llvm-config"):
            return BackendAvailability(available=True)
        for key, val in os.environ.items():
            if key.startswith("LLVM_SYS_") and key.endswith("_PREFIX") and val.strip():
                prefix = Path(val.strip())
                if (prefix / "bin" / "llvm-config").exists() or (
                    prefix / "bin" / "llvm-config.exe"
                ).exists():
                    return BackendAvailability(available=True)
        return BackendAvailability(
            available=False,
            reason="llvm-config not on PATH and no LLVM_SYS_*_PREFIX with "
            "llvm-config found (LLVM backend requires the LLVM toolchain)",
        )

    def build_and_run(
        self,
        file_path: str,
        build_profile: str,
        *,
        extra_env: dict[str, str] | None,
        capabilities: str,
    ) -> BackendResult:
        out_dir = _scratch_dir(self.name, file_path)
        env = _cross_build_env(extra_env, capabilities)
        import molt_diff

        stem = Path(file_path).stem
        output_binary = out_dir / f"{stem}_molt"
        cmd = _build_cmd(
            file_path,
            "llvm",
            build_profile,
            out_dir,
            capabilities,
            extra_build_args=["--emit", "bin", "--output", str(output_binary)],
        )
        build_timeout = _build_timeout("MOLT_COMPAT_LLVM_BUILD_TIMEOUT", 900.0)
        b_out, b_err, b_rc, b_to = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_LLVM",
            env=env,
            timeout=build_timeout,
            cwd=str(_REPO_ROOT),
        )
        binary = output_binary
        if not binary.exists() and (out_dir / f"{stem}_molt.exe").exists():
            binary = out_dir / f"{stem}_molt.exe"
        if b_rc != 0 or not binary.exists():
            return BackendResult(
                stdout=None,
                stderr=(b_err or b_out or "llvm build failed"),
                returncode=b_rc if b_rc != 0 else 1,
                build_failed=True,
                detail="llvm build produced no binary",
            )
        run_timeout = _build_timeout("MOLT_COMPAT_LLVM_RUN_TIMEOUT", 60.0)
        r_out, r_err, r_rc, r_to = _guarded_run(
            [str(binary)],
            prefix="MOLT_COMPAT_LLVM",
            env=env,
            timeout=run_timeout,
            cwd=str(_REPO_ROOT),
        )
        return BackendResult(
            stdout=r_out,
            stderr=r_err,
            returncode=r_rc,
            build_failed=False,
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
        build_profile: str,
        *,
        extra_env: dict[str, str] | None,
        capabilities: str,
    ) -> BackendResult:
        out_dir = _scratch_dir(self.name, file_path)
        env = _cross_build_env(extra_env, capabilities)
        stem = Path(file_path).stem
        cmd = _build_cmd(file_path, "luau", build_profile, out_dir, capabilities)
        build_timeout = _build_timeout("MOLT_COMPAT_LUAU_BUILD_TIMEOUT", 600.0)
        b_out, b_err, b_rc, b_to = _guarded_run(
            cmd,
            prefix="MOLT_COMPAT_LUAU",
            env=env,
            timeout=build_timeout,
            cwd=str(_REPO_ROOT),
        )
        luau_out = out_dir / f"{stem}.luau"
        if not luau_out.exists():
            # Some layouts name it output.luau; accept either.
            alt = out_dir / "output.luau"
            if alt.exists():
                luau_out = alt
        if b_rc != 0 or not luau_out.exists():
            return BackendResult(
                stdout=None,
                stderr=(b_err or b_out or "luau build failed"),
                returncode=b_rc if b_rc != 0 else 1,
                build_failed=True,
                detail="luau build produced no .luau source",
            )
        run_timeout = _build_timeout("MOLT_COMPAT_LUAU_RUN_TIMEOUT", 60.0)
        r_out, r_err, r_rc, r_to = _guarded_run(
            [shutil.which("lune") or "lune", "run", str(luau_out)],
            prefix="MOLT_COMPAT_LUAU",
            env=env,
            timeout=run_timeout,
            cwd=str(_REPO_ROOT),
        )
        return BackendResult(
            stdout=r_out,
            stderr=r_err,
            returncode=r_rc,
            build_failed=False,
        )


# ---------------------------------------------------------------------------
# Registry
# ---------------------------------------------------------------------------

#: The canonical backend ordering for the matrix (matches the suite-honesty
#: BACKENDS set: native / llvm / wasm / luau).
ALL_BACKENDS: tuple[str, ...] = ("native", "llvm", "wasm", "luau")


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
