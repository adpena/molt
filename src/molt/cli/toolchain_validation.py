from __future__ import annotations

import datetime as dt
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Literal, Mapping, Sequence

from molt.cli.atomic_io import _write_json_sidecar
from molt.cli.backend_daemon_config import _backend_daemon_enabled
from molt.cli.backend_diagnostics import _FALSY_ENV_VALUES
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _load_cli_harness_memory_guard,
    _run_completed_command,
)
from molt.cli.default_paths import _default_molt_cache
from molt.cli.env_paths import _base_env
from molt.cli.models import _MaintenanceStep, _ToolchainReport, _ValidationStep
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_molt_root, _require_molt_root
from molt.cli.runtime_paths import _runtime_lib_path


_VALIDATE_PROOF_BYPASS_ENV = frozenset(
    {
        "MOLT_SKIP_BINARY_VALIDITY_CHECK",
        "MOLT_SKIP_CARGO_LOCK",
        "MOLT_SKIP_RUNTIME_REBUILD",
    }
)
_VALIDATE_SUITE_CHOICES = (
    "full",
    "smoke",
    "commands",
    "conformance",
    "bench",
    "custody-proof",
)


def _required_llvm_backend_major(root: Path) -> int | None:
    manifest = root / "runtime" / "molt-backend" / "Cargo.toml"
    try:
        text = manifest.read_text(encoding="utf-8")
    except OSError:
        return None
    match = re.search(
        r'inkwell\s*=\s*\{[^}]*features\s*=\s*\[[^\]]*"llvm(\d+)-\d+"', text, re.DOTALL
    )
    if match is None:
        return None
    try:
        return int(match.group(1))
    except ValueError:
        return None


def _llvm_sys_prefix_env_var(major: int) -> str:
    return f"LLVM_SYS_{major * 10 + 1}_PREFIX"


def _default_llvm_release_for_major(major: int) -> str:
    if major == 22:
        return "22.1.8"
    return f"{major}.1.0"


def _llvm_config_names(major: int) -> list[str]:
    if platform.system() == "Windows":
        return [
            f"llvm-config-{major}.exe",
            f"llvm-config{major}.exe",
            "llvm-config.exe",
            f"llvm-config-{major}",
            f"llvm-config{major}",
            "llvm-config",
        ]
    return [
        f"llvm-config-{major}",
        f"llvm-config{major}",
        "llvm-config",
    ]


def _detect_llvm_backend_toolchain(root: Path) -> tuple[int | None, str | None]:
    major = _required_llvm_backend_major(root)
    if major is None:
        return None, None
    candidates = _llvm_config_names(major)
    prefix_env = os.environ.get(_llvm_sys_prefix_env_var(major), "").strip()
    if prefix_env:
        prefix = Path(prefix_env).expanduser()
        candidates = [
            str(prefix / "bin" / name)
            for name in _llvm_config_names(major)
        ] + candidates
    if platform.system() == "Darwin":
        candidates.extend(
            [
                f"/opt/homebrew/opt/llvm@{major}/bin/llvm-config",
                f"/usr/local/opt/llvm@{major}/bin/llvm-config",
            ]
        )
    for candidate in candidates:
        path = Path(candidate)
        if path.is_absolute():
            if path.exists() and _llvm_config_matches_major(path, major):
                return major, str(path)
            continue
        resolved = shutil.which(candidate)
        if resolved and _llvm_config_matches_major(Path(resolved), major):
            return major, resolved
    return major, None


def _llvm_config_matches_major(path: Path, major: int) -> bool:
    try:
        result = subprocess.run(
            [str(path), "--version"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    if result.returncode != 0:
        return False
    match = re.match(r"\s*(\d+)(?:\.|$)", result.stdout.strip())
    return match is not None and int(match.group(1)) == major


def _llvm_backend_unavailable_message(root: Path) -> str | None:
    major, llvm_toolchain = _detect_llvm_backend_toolchain(root)
    if major is None or llvm_toolchain is not None:
        return None
    env_var = _llvm_sys_prefix_env_var(major)
    advice = "\n".join(f"  - {item}" for item in _llvm_backend_advice(major))
    return (
        f"LLVM backend requires LLVM {major}.1 with llvm-config. "
        f"No matching llvm-config was found.\n"
        f"Set {env_var} to a complete LLVM prefix or put matching llvm-config on PATH.\n"
        f"Recommended actions:\n{advice}"
    )


def _llvm_backend_advice(major: int) -> list[str]:
    system = platform.system()
    env_var = _llvm_sys_prefix_env_var(major)
    if system == "Darwin":
        return [
            f"brew install llvm@{major} lld@{major}",
            f"export PATH=/opt/homebrew/opt/llvm@{major}/bin:$PATH",
            f"export {env_var}=/opt/homebrew/opt/llvm@{major}",
        ]
    if system == "Windows":
        release = _default_llvm_release_for_major(major)
        return [
            (
                f"python tools/bootstrap_llvm.py --version {release} "
                f"--prefix target\\toolchains\\llvm-{release}"
            ),
            f"Set {env_var}=<LLVM prefix containing bin\\llvm-config.exe>",
            (
                "Do not use winget/Chocolatey LLVM for the LLVM backend unless "
                "that install includes bin\\llvm-config.exe"
            ),
        ]
    return [
        f"Install llvm-{major}, llvm-{major}-dev, clang-{major}, and lld-{major}",
        f"export {env_var}=<LLVM prefix containing bin/llvm-config>",
    ]


def _cmake_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["brew install cmake"]
    if system == "Windows":
        return ["winget install Kitware.CMake", "or: choco install cmake -y"]
    return ["sudo apt-get install -y cmake"]


def _ninja_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["brew install ninja"]
    if system == "Windows":
        return ["winget install Ninja-build.Ninja", "or: choco install ninja -y"]
    return ["sudo apt-get install -y ninja-build"]


def _wasm_tools_setup_advice(system: str) -> list[str]:
    del system
    return ["cargo install wasm-tools --locked"]


def _wasm_pack_setup_advice(system: str) -> list[str]:
    del system
    return ["cargo install wasm-pack --locked"]


def _wasmtime_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["brew install wasmtime"]
    if system == "Windows":
        return ["winget install BytecodeAlliance.Wasmtime", "or: cargo install wasmtime-cli --locked"]
    return ["curl https://wasmtime.dev/install.sh -sSf | bash"]


def _luau_runner_setup_advice(system: str) -> list[str]:
    del system
    return ["cargo install lune --locked", "or install a luau runner on PATH"]


def _windows_vswhere_path() -> Path | None:
    if platform.system() != "Windows":
        return None
    roots = [
        os.environ.get("ProgramFiles(x86)", ""),
        os.environ.get("ProgramFiles", ""),
    ]
    for root in roots:
        if not root:
            continue
        candidate = (
            Path(root)
            / "Microsoft Visual Studio"
            / "Installer"
            / "vswhere.exe"
        )
        if candidate.exists():
            return candidate
    return None


def _windows_vsdevcmd_path() -> Path | None:
    vswhere = _windows_vswhere_path()
    if vswhere is None:
        return None
    try:
        proc = subprocess.run(
            [
                str(vswhere),
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    first = next((line.strip() for line in proc.stdout.splitlines() if line.strip()), "")
    if not first:
        return None
    candidate = Path(first) / "Common7" / "Tools" / "VsDevCmd.bat"
    return candidate if candidate.exists() else None


def _windows_msvc_env_advice() -> list[str]:
    vsdevcmd = _windows_vsdevcmd_path()
    if vsdevcmd is None:
        return [
            "winget install Microsoft.VisualStudio.2022.BuildTools",
            "Include the x64 C++ build tools workload",
        ]
    return [
        f'Run from an x64 VS developer shell: "{vsdevcmd}" -arch=x64 -host_arch=x64',
        "or let tools/bootstrap_llvm.py activate VsDevCmd.bat for the LLVM source build",
    ]


def _python_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["brew install python@3.12", "Ensure python3 is on PATH"]
    if system == "Windows":
        return ["winget install Python.Python.3.12", "Reopen your terminal"]
    return ["Install Python 3.12+ via your package manager"]


def _uv_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["brew install uv"]
    if system == "Windows":
        return ["winget install Astral.Uv", "or: scoop install uv"]
    return ["curl -LsSf https://astral.sh/uv/install.sh | sh"]


def _rustup_setup_advice(system: str) -> list[str]:
    if system == "Windows":
        return ["winget install Rustlang.Rustup", "Reopen your terminal"]
    return ["curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"]


def _ensure_rustup_target(target_triple: str, warnings: list[str]) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    try:
        result = _run_completed_command(
            [rustup_path, "target", "list", "--installed"],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        warnings.append(f"Failed to query rustup targets: {exc}")
        return False
    installed = result.stdout.split()
    if target_triple in installed:
        return True
    try:
        add = _run_completed_command(
            [rustup_path, "target", "add", target_triple],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        warnings.append(f"Failed to install rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    return True


def _cargo_setup_advice(system: str) -> list[str]:
    return _rustup_setup_advice(system) + ["source $HOME/.cargo/env (Unix)"]


def _clang_setup_advice(system: str) -> list[str]:
    if system == "Darwin":
        return ["xcode-select --install"]
    if system == "Windows":
        return ["winget install LLVM.LLVM", "set CC=clang"]
    return ["sudo apt-get update", "sudo apt-get install -y clang lld"]


def _resolved_env_dir_from_root(root: Path, var: str) -> Path | None:
    raw = os.environ.get(var, "").strip()
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = (root / path).absolute()
    return path


def _is_path_within(path: Path, container: Path) -> bool:
    try:
        path.resolve().relative_to(container.resolve())
    except ValueError:
        return False
    return True


def _canonical_env_defaults(root: Path) -> dict[str, str]:
    tmp_root = root / "tmp"
    target_root = root / "target"
    return {
        "MOLT_EXT_ROOT": str(root),
        "CARGO_TARGET_DIR": str(target_root),
        "MOLT_DIFF_CARGO_TARGET_DIR": str(target_root),
        "MOLT_CACHE": str(root / ".molt_cache"),
        "MOLT_DIFF_ROOT": str(tmp_root / "diff"),
        "MOLT_DIFF_TMPDIR": str(tmp_root),
        "UV_CACHE_DIR": str(root / ".uv-cache"),
        "TMPDIR": str(tmp_root),
    }


def _collect_setup_actions(checks: Sequence[Mapping[str, Any]]) -> list[dict[str, str]]:
    actions: list[dict[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for check in checks:
        name = str(check.get("name", "unknown"))
        level = str(check.get("level", "error"))
        for advice in check.get("advice", []):
            action = str(advice)
            key = (name, action)
            if key in seen:
                continue
            seen.add(key)
            actions.append({"check": name, "level": level, "command": action})
    return actions


def _build_toolchain_report(root: Path) -> _ToolchainReport:
    checks: list[dict[str, Any]] = []
    warnings: list[str] = []
    errors: list[str] = []
    system = platform.system()

    def record(
        name: str,
        ok: bool,
        detail: str,
        *,
        level: Literal["warning", "error"] = "error",
        advice: list[str] | None = None,
    ) -> None:
        entry: dict[str, Any] = {"name": name, "ok": ok, "detail": detail}
        if not ok:
            entry["level"] = level
            if advice:
                entry["advice"] = advice
            message = f"{name}: {detail}"
            if advice:
                message = f"{message}. See advice."
            if level == "error":
                errors.append(message)
            else:
                warnings.append(message)
        checks.append(entry)

    python_ok = sys.version_info >= (3, 12)
    record(
        "python",
        python_ok,
        f"{sys.version.split()[0]} (requires >=3.12)",
        level="error",
        advice=_python_setup_advice(system) if not python_ok else None,
    )

    uv_path = shutil.which("uv")
    record(
        "uv",
        bool(uv_path),
        uv_path or "not found",
        level="warning",
        advice=_uv_setup_advice(system) if not uv_path else None,
    )

    cargo_path = shutil.which("cargo")
    record(
        "cargo",
        bool(cargo_path),
        cargo_path or "not found",
        level="error",
        advice=_cargo_setup_advice(system) if not cargo_path else None,
    )

    rustup_path = shutil.which("rustup")
    record(
        "rustup",
        bool(rustup_path),
        rustup_path or "not found",
        level="warning",
        advice=_rustup_setup_advice(system) if not rustup_path else None,
    )

    cargo_upgrade_path = shutil.which("cargo-upgrade")
    record(
        "cargo-upgrade",
        bool(cargo_upgrade_path),
        cargo_upgrade_path or "not found",
        level="warning",
        advice=["cargo install cargo-edit --locked", "Use `molt update --all`"]
        if not cargo_upgrade_path
        else None,
    )

    cc = os.environ.get("CC", "clang")
    cc_path = shutil.which(cc) or shutil.which("clang")
    record(
        "clang",
        bool(cc_path),
        cc_path or "not found",
        level="error",
        advice=_clang_setup_advice(system) if not cc_path else None,
    )

    cmake_path = shutil.which("cmake")
    record(
        "cmake",
        bool(cmake_path),
        cmake_path or "not found",
        level="error",
        advice=_cmake_setup_advice(system) if not cmake_path else None,
    )

    ninja_path = shutil.which("ninja")
    record(
        "ninja",
        bool(ninja_path),
        ninja_path or "not found",
        level="error",
        advice=_ninja_setup_advice(system) if not ninja_path else None,
    )

    msvc_build_env_ok = True
    if system == "Windows":
        cl_path = shutil.which("cl")
        vsdevcmd_path = _windows_vsdevcmd_path()
        msvc_build_env_ok = cl_path is not None
        detail = (
            cl_path
            if cl_path is not None
            else (
                f"VS Build Tools found at {vsdevcmd_path}, but cl.exe is not active"
                if vsdevcmd_path is not None
                else "Visual Studio C++ build tools not found"
            )
        )
        record(
            "msvc-build-env",
            msvc_build_env_ok,
            detail,
            level="warning",
            advice=_windows_msvc_env_advice() if not msvc_build_env_ok else None,
        )

    llvm_major, llvm_toolchain = _detect_llvm_backend_toolchain(root)
    if llvm_major is None:
        record(
            "llvm-backend-toolchain",
            True,
            "no explicit LLVM backend version pin detected",
        )
    else:
        record(
            "llvm-backend-toolchain",
            llvm_toolchain is not None,
            (
                f"LLVM {llvm_major} via {llvm_toolchain}"
                if llvm_toolchain is not None
                else f"LLVM {llvm_major} toolchain not found"
            ),
            level="warning",
            advice=_llvm_backend_advice(llvm_major) if llvm_toolchain is None else None,
        )

    wasm_ld_path = shutil.which("wasm-ld")
    record(
        "wasm-ld",
        bool(wasm_ld_path),
        wasm_ld_path or "not found",
        level="warning",
        advice=_clang_setup_advice(system) if not wasm_ld_path else None,
    )

    wasm_tools_path = shutil.which("wasm-tools")
    record(
        "wasm-tools",
        bool(wasm_tools_path),
        wasm_tools_path or "not found",
        level="warning",
        advice=_wasm_tools_setup_advice(system) if not wasm_tools_path else None,
    )

    wasm_pack_path = shutil.which("wasm-pack")
    record(
        "wasm-pack",
        bool(wasm_pack_path),
        wasm_pack_path or "not found",
        level="warning",
        advice=_wasm_pack_setup_advice(system) if not wasm_pack_path else None,
    )

    wasmtime_path = shutil.which("wasmtime")
    record(
        "wasmtime",
        bool(wasmtime_path),
        wasmtime_path or "not found",
        level="warning",
        advice=_wasmtime_setup_advice(system) if not wasmtime_path else None,
    )

    luau_runner_path = shutil.which("luau") or shutil.which("lune")
    record(
        "luau-runner",
        bool(luau_runner_path),
        luau_runner_path or "not found",
        level="warning",
        advice=_luau_runner_setup_advice(system) if not luau_runner_path else None,
    )

    zig_path = shutil.which("zig")
    record(
        "zig",
        bool(zig_path),
        zig_path or "not found",
        level="warning",
        advice=["Install zig if you need wasm linking"] if not zig_path else None,
    )

    rustc_wrapper = os.environ.get("RUSTC_WRAPPER", "").strip()
    sccache_mode = os.environ.get("MOLT_USE_SCCACHE", "auto").strip().lower() or "auto"
    sccache_path = shutil.which("sccache")
    if rustc_wrapper:
        wrapper_name = Path(rustc_wrapper).name
        sccache_ok = wrapper_name == "sccache"
        sccache_detail = f"RUSTC_WRAPPER={rustc_wrapper}"
        sccache_advice = (
            [
                "Use RUSTC_WRAPPER=sccache for compile throughput",
                "or unset RUSTC_WRAPPER and set MOLT_USE_SCCACHE=auto",
            ]
            if not sccache_ok
            else None
        )
    elif sccache_mode in {"0", "false", "no", "off"}:
        sccache_ok = False
        sccache_detail = "disabled via MOLT_USE_SCCACHE"
        sccache_advice = ["Set MOLT_USE_SCCACHE=auto or 1 for faster rebuilds"]
    elif sccache_path is None:
        sccache_ok = False
        sccache_detail = "not found on PATH"
        sccache_advice = [
            "Install sccache and keep MOLT_USE_SCCACHE=auto (or set to 1)"
        ]
    else:
        sccache_ok = True
        sccache_detail = f"{sccache_path} (mode={sccache_mode})"
        sccache_advice = None
    record(
        "sccache",
        sccache_ok,
        sccache_detail,
        level="warning",
        advice=sccache_advice,
    )

    if os.name == "posix":
        daemon_enabled = _backend_daemon_enabled()
        daemon_raw = os.environ.get("MOLT_BACKEND_DAEMON", "1").strip() or "1"
        record(
            "backend-daemon",
            daemon_enabled,
            f"MOLT_BACKEND_DAEMON={daemon_raw}",
            level="warning",
            advice=["Set MOLT_BACKEND_DAEMON=1 for faster native compile loops"]
            if not daemon_enabled
            else None,
        )
    else:
        record("backend-daemon", True, "unsupported on non-posix hosts")

    cargo_target_dir = _resolved_env_dir_from_root(root, "CARGO_TARGET_DIR")
    if cargo_target_dir is None:
        record(
            "cargo-target-dir",
            False,
            f"defaulting to {root / 'target'}",
            level="warning",
            advice=[
                "export CARGO_TARGET_DIR=<external>/target",
                "export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR",
            ],
        )
    else:
        record("cargo-target-dir", True, str(cargo_target_dir))

    molt_cache_dir = _resolved_env_dir_from_root(root, "MOLT_CACHE")
    if molt_cache_dir is None:
        record(
            "molt-cache-dir",
            False,
            f"defaulting to {_default_molt_cache()}",
            level="warning",
            advice=["export MOLT_CACHE=<external>/molt_cache"],
        )
    else:
        record("molt-cache-dir", True, str(molt_cache_dir))

    diff_target_dir = _resolved_env_dir_from_root(root, "MOLT_DIFF_CARGO_TARGET_DIR")
    if diff_target_dir is None:
        record(
            "molt-diff-target-dir",
            False,
            "not set",
            level="warning",
            advice=["export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"],
        )
    elif cargo_target_dir is not None and diff_target_dir != cargo_target_dir:
        record(
            "molt-diff-target-dir",
            False,
            f"{diff_target_dir} (CARGO_TARGET_DIR={cargo_target_dir})",
            level="warning",
            advice=["Set MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"],
        )
    else:
        record("molt-diff-target-dir", True, str(diff_target_dir))

    configured_ext_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    ext_root = (
        Path(configured_ext_root).expanduser().resolve()
        if configured_ext_root
        else None
    )
    if ext_root is not None and ext_root.is_dir():
        routed_paths: list[Path] = []
        if cargo_target_dir is not None:
            routed_paths.append(cargo_target_dir)
        if molt_cache_dir is not None:
            routed_paths.append(molt_cache_dir)
        ext_ok = bool(routed_paths) and all(
            _is_path_within(path, ext_root) for path in routed_paths
        )
        detail = (
            "CARGO_TARGET_DIR and MOLT_CACHE routed to configured artifact root"
            if ext_ok
            else "Set CARGO_TARGET_DIR and MOLT_CACHE under the configured artifact root"
        )
        record(
            "artifact-root-routing",
            ext_ok,
            detail,
            level="warning",
            advice=[
                "export MOLT_EXT_ROOT=<artifact-root>",
                "export CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target",
                "export MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache",
            ]
            if not ext_ok
            else None,
        )
    else:
        record(
            "artifact-root",
            True,
            "Using repo-local canonical artifact roots",
            advice=[
                "Set MOLT_EXT_ROOT=<external-root> if you want shared external artifacts",
                "or keep repo-local target/tmp/log/cache roots for local development",
            ],
        )

    pyproject = root / "pyproject.toml"
    lock_path = root / "uv.lock"
    if pyproject.exists():
        record(
            "uv.lock",
            lock_path.exists(),
            str(lock_path),
            level="warning",
            advice=["uv sync", "or: uv lock"] if not lock_path.exists() else None,
        )
        if lock_path.exists():
            try:
                if lock_path.stat().st_mtime < pyproject.stat().st_mtime:
                    record(
                        "uv.lock_fresh",
                        False,
                        "uv.lock older than pyproject.toml",
                        level="warning",
                        advice=["uv lock", "or: uv sync"],
                    )
            except OSError:
                record(
                    "uv.lock_fresh",
                    False,
                    "unable to stat uv.lock",
                    level="warning",
                    advice=["Ensure uv.lock exists and is readable"],
                )

    runtime_lib = _runtime_lib_path(root, "release", None)
    runtime_exists = runtime_lib.exists()
    if runtime_exists:
        runtime_detail = str(runtime_lib)
        try:
            lib_mtime = runtime_lib.stat().st_mtime
            runtime_cargo = root / "runtime" / "molt-runtime" / "Cargo.toml"
            if runtime_cargo.exists() and runtime_cargo.stat().st_mtime > lib_mtime:
                runtime_detail += " (stale — runtime Cargo.toml is newer)"
                runtime_exists = False
        except OSError:
            pass
    else:
        runtime_detail = f"not found: {runtime_lib}"
    record(
        "molt-runtime",
        runtime_exists,
        runtime_detail,
        level="warning",
        advice=[
            "Run: molt run examples/hello.py to auto-build and materialize runtime aliases",
            "Raw cargo builds only refresh the platform staticlib scratch artifact; Molt publishes profile-qualified aliases",
        ]
        if not runtime_exists
        else None,
    )

    wasm_target_ok = False
    if rustup_path:
        try:
            result = _run_completed_command(
                ["rustup", "target", "list", "--installed"],
                capture_output=True,
                env=None,
                cwd=root,
                memory_guard_prefix="MOLT_BUILD",
            )
        except OSError as exc:
            record("rustup-targets", False, f"failed to query: {exc}")
        else:
            targets = result.stdout.split()
            wasm_target_ok = any(
                target in targets
                for target in ("wasm32-wasip1", "wasm32-unknown-unknown")
            )
            record(
                "wasm-target",
                wasm_target_ok,
                "wasm32-wasip1 or wasm32-unknown-unknown",
                level="warning",
                advice=["rustup target add wasm32-wasip1"]
                if not wasm_target_ok
                else None,
            )

    environment = _canonical_env_defaults(root)
    backends = {
        "native": bool(cargo_path and cc_path and cmake_path and ninja_path),
        "llvm": bool(cargo_path and cc_path and llvm_toolchain),
        "wasm": bool(cargo_path and wasm_target_ok),
        "linked-wasm": bool(
            cargo_path and wasm_target_ok and (zig_path or wasm_ld_path) and wasm_tools_path
        ),
        "luau": bool(luau_runner_path),
    }
    profiles = {
        "dev": bool(cargo_path),
        "release": bool(cargo_path),
    }
    actions = _collect_setup_actions(checks)
    return _ToolchainReport(
        checks=checks,
        warnings=warnings,
        errors=errors,
        environment=environment,
        actions=actions,
        backends=backends,
        profiles=profiles,
    )


def _planned_update_steps(
    root: Path,
    *,
    include_toolchains: bool,
    include_locks: bool,
    include_manifests: bool,
) -> tuple[list[_MaintenanceStep], list[str]]:
    steps: list[_MaintenanceStep] = []
    warnings: list[str] = []

    if include_toolchains:
        if shutil.which("rustup"):
            steps.extend(
                [
                    _MaintenanceStep(
                        "rustup-update-stable",
                        ["rustup", "update", "stable"],
                        root,
                        "toolchain",
                    ),
                    _MaintenanceStep(
                        "rustup-target-add-wasm32-unknown-unknown",
                        ["rustup", "target", "add", "wasm32-unknown-unknown"],
                        root,
                        "toolchain",
                    ),
                    _MaintenanceStep(
                        "rustup-target-add-wasm32-wasip1",
                        ["rustup", "target", "add", "wasm32-wasip1"],
                        root,
                        "toolchain",
                    ),
                ]
            )
        else:
            warnings.append(
                "rustup is not installed; skipping Rust toolchain refresh steps"
            )
        if shutil.which("cargo"):
            cargo_tool_steps: list[tuple[str, str, str]] = [
                ("wasm-tools", "wasm-tools", "wasm-tools"),
                ("wasm-pack", "wasm-pack", "wasm-pack"),
            ]
            for tool_name, crate_name, command_name in cargo_tool_steps:
                if shutil.which(command_name):
                    continue
                steps.append(
                    _MaintenanceStep(
                        f"cargo-install-{tool_name}",
                        ["cargo", "install", crate_name, "--locked"],
                        root,
                        "toolchain",
                    )
                )
            llvm_major, llvm_toolchain = _detect_llvm_backend_toolchain(root)
            if llvm_major is not None and llvm_toolchain is None:
                release = _default_llvm_release_for_major(llvm_major)
                warnings.append(
                    "LLVM backend toolchain is missing; run "
                    f"python tools/bootstrap_llvm.py --version {release} "
                    f"--prefix target/toolchains/llvm-{release} and set "
                    f"{_llvm_sys_prefix_env_var(llvm_major)} to that prefix"
                )
        else:
            warnings.append(
                "cargo is not installed; skipping cargo-installable toolchain helpers"
            )

    if include_locks:
        steps.extend(
            [
                _MaintenanceStep(
                    "cargo-update-root",
                    ["cargo", "update", "--manifest-path", "Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "cargo-update-runtime",
                    ["cargo", "update", "--manifest-path", "runtime/Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "cargo-update-fuzz",
                    ["cargo", "update", "--manifest-path", "fuzz/Cargo.toml"],
                    root,
                    "lock",
                ),
                _MaintenanceStep(
                    "uv-lock-upgrade",
                    ["uv", "lock", "-U"],
                    root,
                    "lock",
                ),
            ]
        )

    if include_manifests:
        if shutil.which("cargo-upgrade") is None:
            steps.append(
                _MaintenanceStep(
                    "cargo-edit-bootstrap",
                    ["cargo", "install", "cargo-edit", "--locked"],
                    root,
                    "manifest",
                )
            )
        steps.extend(
            [
                _MaintenanceStep(
                    "cargo-upgrade-root",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
                _MaintenanceStep(
                    "cargo-upgrade-runtime",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "runtime/Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
                _MaintenanceStep(
                    "cargo-upgrade-fuzz",
                    [
                        "cargo",
                        "upgrade",
                        "--incompatible",
                        "--manifest-path",
                        "fuzz/Cargo.toml",
                    ],
                    root,
                    "manifest",
                ),
            ]
        )

    return steps, warnings


def update_repo(
    *,
    json_output: bool = False,
    verbose: bool = False,
    check_only: bool = False,
    include_toolchains: bool = True,
    include_locks: bool = True,
    include_manifests: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "update")
    if root_error is not None:
        return root_error

    steps, warnings = _planned_update_steps(
        root,
        include_toolchains=include_toolchains,
        include_locks=include_locks,
        include_manifests=include_manifests,
    )
    step_rows = [
        {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
        }
        for step in steps
    ]

    if check_only:
        payload = _json_payload(
            "update",
            "ok",
            data={
                "root": str(root),
                "check_only": True,
                "steps": step_rows,
            },
            warnings=warnings,
        )
        if json_output:
            _emit_json(payload, json_output=True)
        else:
            print(f"Update plan for {root}:")
            for row in step_rows:
                print(f"- [{row['category']}] {row['name']}: {shlex.join(row['cmd'])}")
            for warning in warnings:
                print(f"warning: {warning}", file=sys.stderr)
        return 0

    results: list[dict[str, Any]] = []
    for step in steps:
        if verbose and not json_output:
            print(f"[molt update] {step.name}: {shlex.join(step.cmd)}", file=sys.stderr)
        proc = _run_completed_command(
            step.cmd,
            cwd=step.cwd,
            capture_output=True,
            env=None,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        entry: dict[str, Any] = {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "returncode": proc.returncode,
        }
        if proc.stdout:
            entry["stdout"] = proc.stdout
        if proc.stderr:
            entry["stderr"] = proc.stderr
        results.append(entry)
        if proc.returncode != 0:
            payload = _json_payload(
                "update",
                "error",
                data={
                    "root": str(root),
                    "check_only": False,
                    "steps": step_rows,
                    "results": results,
                },
                warnings=warnings,
                errors=[
                    f"{step.name} failed with exit code {proc.returncode}",
                ],
            )
            if json_output:
                _emit_json(payload, json_output=True)
            else:
                print(
                    f"molt update failed at {step.name}: {shlex.join(step.cmd)}",
                    file=sys.stderr,
                )
                if proc.stderr:
                    print(proc.stderr, file=sys.stderr, end="")
            return proc.returncode or 1

    payload = _json_payload(
        "update",
        "ok",
        data={
            "root": str(root),
            "check_only": False,
            "steps": step_rows,
            "results": results,
        },
        warnings=warnings,
    )
    if json_output:
        _emit_json(payload, json_output=True)
    else:
        print(f"Updated toolchains/dependencies for {root}")
        for result in results:
            print(
                f"- [{result['category']}] {result['name']} (rc={result['returncode']})"
            )
    return 0


def setup(
    json_output: bool = False,
    verbose: bool = False,
    strict: bool = False,
) -> int:
    del verbose
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "setup")
    if root_error is not None:
        return root_error
    report = _build_toolchain_report(root)
    status = "ok" if not report.errors else "error"
    data = {
        "checks": report.checks,
        "environment": report.environment,
        "actions": report.actions,
        "backends": report.backends,
        "profiles": report.profiles,
    }
    if json_output:
        payload = _json_payload(
            "setup",
            status,
            data=data,
            warnings=report.warnings,
            errors=report.errors,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Molt setup plan for {root}:")
        print("Backends:")
        for name, ready in sorted(report.backends.items()):
            print(f"- {name}: {'ready' if ready else 'missing requirements'}")
        print("Profiles:")
        for name, ready in sorted(report.profiles.items()):
            print(f"- {name}: {'ready' if ready else 'missing requirements'}")
        print("Environment:")
        for key, value in report.environment.items():
            print(f"export {key}={shlex.quote(value)}")
        if report.actions:
            print("Actions:")
            for action in report.actions:
                print(f"- [{action['check']}] {action['command']}")
        else:
            print("No setup actions required.")
    if strict and report.errors:
        return 1
    return 0


def doctor(
    json_output: bool = False,
    verbose: bool = False,
    strict: bool = False,
) -> int:
    del verbose
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "doctor")
    if root_error is not None:
        return root_error
    report = _build_toolchain_report(root)
    status = "ok" if not report.errors else "error"
    data = {
        "checks": report.checks,
        "environment": report.environment,
        "actions": report.actions,
        "backends": report.backends,
        "profiles": report.profiles,
    }
    if json_output:
        payload = _json_payload(
            "doctor",
            status,
            data=data,
            warnings=report.warnings,
            errors=report.errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for check in report.checks:
            if check["ok"]:
                print(f"OK: {check['name']} ({check['detail']})")
                continue
            level = check.get("level", "error").upper()
            print(f"{level}: {check['name']} ({check['detail']})")
            for hint in check.get("advice", []):
                print(f"  -> {hint}")
        if report.actions:
            print("Suggested actions:")
            for action in report.actions:
                print(f"- [{action['check']}] {action['command']}")
    if strict and any(not check["ok"] for check in report.checks):
        return 1
    return 0


def _planned_validate_steps(
    root: Path,
    *,
    suite: Literal[
        "full",
        "smoke",
        "commands",
        "conformance",
        "bench",
        "custody-proof",
    ],
    backend: Literal["all", "native", "llvm", "wasm", "luau"],
    profile: Literal["all", "dev", "release"],
) -> list[_ValidationStep]:
    python = sys.executable
    bench_profile = "release" if profile == "all" else profile
    build_profile = "release" if profile == "all" else profile
    steps: list[_ValidationStep] = [
        _ValidationStep(
            "cli-run-json",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/cli/test_cli_smoke.py",
                "-k",
                "test_cli_run_json",
            ],
            root,
            "command",
            ("native",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "cli-command-json",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/cli/test_cli_smoke.py",
                "-k",
                (
                    "test_cli_build_json_binary_executes_for_native_profiles "
                    "or test_cli_compare_json "
                    "or test_cli_run_exec_eval_raise_runtime_error"
                ),
            ],
            root,
            "command",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "subprocess-guard-audit",
            [
                python,
                "tools/check_subprocess_guard_coverage.py",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "memory-guard-wiring-audit",
            [
                python,
                "tools/check_memory_guard_wiring.py",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "custody-proof",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_memory_guard_wiring.py",
                "tests/tools/test_memory_guard_windows_sampling.py",
                "tests/tools/test_process_sentinel.py",
                "tests/cli/test_cli_smoke.py::test_cli_hash_seed_windows_handoff_waits_for_restarted_process",
                "tests/cli/test_cli_smoke.py::test_cli_hash_seed_reexec_argv_uses_active_python_executable",
            ],
            root,
            "command",
            ("native", "llvm", "wasm", "luau"),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "native-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_native_lir_loop_join_semantics.py",
                "-k",
                "not llvm",
            ],
            root,
            "correctness",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "llvm-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_native_lir_loop_join_semantics.py",
                "-k",
                "llvm_simple_exception_catch or llvm_exception_loop",
            ],
            root,
            "correctness",
            ("llvm",),
            ("release",),
            "smoke",
        ),
        _ValidationStep(
            "wasm-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_wasm_control_flow.py",
                "tests/test_wasm_class_smoke.py",
                "-k",
                "preserves_type_name or wasm_module_try_exception_loop_parity",
            ],
            root,
            "correctness",
            ("wasm",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-support-matrix",
            [
                python,
                "tools/gen_luau_support_matrix.py",
                "--check",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-compile-smoke",
            [
                python,
                "-m",
                "molt.cli",
                "build",
                "examples/hello.py",
                "--target",
                "luau",
                "--profile",
                build_profile,
                "--output",
                str(root / "tmp" / "validate" / "luau-smoke" / "hello.luau"),
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-runner-available",
            [
                python,
                "-c",
                (
                    "import shutil, sys; "
                    "runner = shutil.which('luau') or shutil.which('lune'); "
                    "print(runner) if runner else sys.exit("
                    "'luau or lune is required for Luau validation')"
                ),
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-ord-at-parity",
            [
                python,
                "-m",
                "pytest",
                "-q",
                "tests/test_ord_at_native.py",
                "-k",
                "luau",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev",),
            "smoke",
        ),
        _ValidationStep(
            "native-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "native-backend",
                "--test",
                "entry_block_param_shadow",
                "--test",
                "lir_loop_and_join_regressions",
                "--test",
                "native_extern_linkage",
                "--test",
                "ir_contract_validation",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "wasm-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "wasm-backend",
                "--test",
                "lir_wasm_repr_regressions",
                "--test",
                "wasm_lir_fast_path_integration",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("wasm",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-rust-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "luau-backend",
                "--lib",
                "luau::tests::",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "luau-lowering-regressions",
            [
                "cargo",
                "test",
                "-p",
                "molt-backend",
                "--features",
                "luau-backend",
                "--lib",
                "luau_lower::tests::",
                "--",
                "--nocapture",
            ],
            root,
            "correctness",
            ("luau",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "conformance-smoke",
            [
                python,
                "tests/harness/run_molt_conformance.py",
                "--suite",
                "smoke",
                "--json-out",
                str(root / "logs" / "validate-conformance-smoke.json"),
            ],
            root,
            "conformance",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "conformance-full",
            [
                python,
                "tests/harness/run_molt_conformance.py",
                "--suite",
                "full",
                "--json-out",
                str(root / "logs" / "validate-conformance-full.json"),
            ],
            root,
            "conformance",
            ("native",),
            ("dev", "release"),
            "full",
        ),
        _ValidationStep(
            "bench-smoke",
            [
                python,
                "tools/bench.py",
                "--smoke",
                "--warmup",
                "1",
                "--molt-profile",
                bench_profile,
                "--json-out",
                str(root / "bench" / "results" / "validate-bench-smoke.json"),
            ],
            root,
            "benchmark",
            ("native",),
            ("dev", "release"),
            "smoke",
        ),
        _ValidationStep(
            "bench-full",
            [
                python,
                "tools/bench.py",
                "--molt-profile",
                bench_profile,
                "--json-out",
                str(root / "bench" / "results" / "validate-bench-full.json"),
            ],
            root,
            "benchmark",
            ("native",),
            ("dev", "release"),
            "full",
        ),
    ]

    selected: list[_ValidationStep] = []
    for step in steps:
        if suite == "custody-proof" and step.name != "custody-proof":
            continue
        if suite == "commands" and step.category != "command":
            continue
        if suite == "conformance" and step.category != "conformance":
            continue
        if suite == "bench" and step.category != "benchmark":
            continue
        if suite == "smoke" and step.suite != "smoke":
            continue
        if (
            suite == "full"
            and step.suite == "smoke"
            and step.category
            in {
                "conformance",
                "benchmark",
            }
        ):
            continue
        if backend != "all" and backend not in step.backends:
            continue
        if profile != "all" and profile not in step.profiles:
            continue
        selected.append(step)
    return selected


def _validate_guard_prefix(step: _ValidationStep) -> str:
    if step.category == "benchmark":
        return "MOLT_BENCH"
    if step.category == "conformance":
        return "MOLT_CONFORMANCE"
    return "MOLT_TEST_SUITE"


def _validation_guard_summary(
    root: Path,
    env: Mapping[str, str],
    steps: Sequence[_ValidationStep],
) -> dict[str, Any]:
    harness_memory_guard = _load_cli_harness_memory_guard(root)
    prefixes = sorted({_validate_guard_prefix(step) for step in steps})
    summary: dict[str, Any] = {}
    for prefix in prefixes:
        limits = harness_memory_guard.limits_from_env(prefix, env)
        summary[prefix] = harness_memory_guard.limits_summary(limits)
    return summary


def _format_validate_guard_summary(prefix: str, limits: Mapping[str, Any]) -> str:
    def gb(name: str) -> str:
        value = limits[name]
        return f"{value:.2f}" if isinstance(value, float) else str(value)

    return (
        f"- {prefix}: process={gb('max_process_rss_gb')}GB "
        f"tree={gb('max_total_rss_gb')}GB "
        f"global={gb('max_global_rss_gb')}GB "
        f"child_rlimit={gb('child_rlimit_gb')}GB"
    )


def _default_validate_summary_path(
    root: Path,
    *,
    suite: str,
    backend: str,
    profile: str,
) -> Path:
    return root / "logs" / f"validate-{suite}-{backend}-{profile}.json"


def _resolve_validate_summary_path(root: Path, summary_out: str | None) -> Path:
    if summary_out is None:
        raise ValueError("summary_out must not be None")
    path = Path(summary_out).expanduser()
    if not path.is_absolute():
        path = root / path
    return path


def _persist_validate_summary(
    payload: dict[str, Any],
    *,
    summary_path: Path | None,
) -> str | None:
    if summary_path is None:
        return None
    payload["data"]["summary_path"] = str(summary_path)
    try:
        _write_json_sidecar(summary_path, payload)
    except OSError as exc:
        return f"Failed to write validate summary {summary_path}: {exc}"
    return None


def _validate_proof_bypass_errors(env: Mapping[str, str]) -> list[str]:
    errors: list[str] = []
    for key in sorted(_VALIDATE_PROOF_BYPASS_ENV):
        value = env.get(key)
        if value is None or value.strip().lower() in _FALSY_ENV_VALUES:
            continue
        errors.append(
            f"{key}={value} disables a validation proof gate; unset it before running molt validate."
        )
    return errors


def validate(
    *,
    suite: Literal[
        "full",
        "smoke",
        "commands",
        "conformance",
        "bench",
        "custody-proof",
    ] = "full",
    backend: Literal["all", "native", "llvm", "wasm", "luau"] = "all",
    profile: Literal["all", "dev", "release"] = "all",
    json_output: bool = False,
    verbose: bool = False,
    check_only: bool = False,
    summary_out: str | None = None,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "validate")
    if root_error is not None:
        return root_error
    bypass_errors = _validate_proof_bypass_errors(os.environ)
    if bypass_errors:
        return _fail(" ".join(bypass_errors), json_output, command="validate")
    steps = _planned_validate_steps(
        root,
        suite=suite,
        backend=backend,
        profile=profile,
    )
    if not steps:
        return _fail(
            "No validation steps matched the requested filters.",
            json_output,
            command="validate",
        )
    step_rows = [
        {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "memory_guard_prefix": _validate_guard_prefix(step),
            "backends": list(step.backends),
            "profiles": list(step.profiles),
            "suite": step.suite,
        }
        for step in steps
    ]
    env = _base_env(root, molt_root=root)
    for key, value in _canonical_env_defaults(root).items():
        env.setdefault(key, value)
    env.setdefault("MOLT_SESSION_ID", "validate")
    guard_summary = _validation_guard_summary(root, env, steps)
    summary_path = (
        _resolve_validate_summary_path(root, summary_out)
        if summary_out is not None
        else (
            None
            if check_only
            else _default_validate_summary_path(
                root,
                suite=suite,
                backend=backend,
                profile=profile,
            )
        )
    )
    started_at = dt.datetime.now(dt.timezone.utc).isoformat()
    validate_started = time.perf_counter()
    if check_only:
        payload = _json_payload(
            "validate",
            "ok",
            data={
                "check_only": True,
                "started_at": started_at,
                "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                "elapsed_s": round(time.perf_counter() - validate_started, 6),
                "suite": suite,
                "backend": backend,
                "profile": profile,
                "steps": step_rows,
                "memory_guard": guard_summary,
            },
        )
        summary_error = _persist_validate_summary(payload, summary_path=summary_path)
        if summary_error is not None:
            payload["status"] = "error"
            payload["errors"].append(summary_error)
        if json_output:
            _emit_json(payload, json_output=True)
        else:
            print("Validation plan:")
            for row in step_rows:
                print(f"- [{row['category']}] {row['name']}: {shlex.join(row['cmd'])}")
            print("Memory guard:")
            for prefix, limits in guard_summary.items():
                print(_format_validate_guard_summary(prefix, limits))
            if summary_path is not None:
                print(f"Summary: {summary_path}")
        return 1 if summary_error is not None else 0

    results: list[dict[str, Any]] = []
    for step in steps:
        if verbose and not json_output:
            print(
                f"[molt validate] {step.name}: {shlex.join(step.cmd)}",
                file=sys.stderr,
            )
        guard_prefix = _validate_guard_prefix(step)
        start = time.perf_counter()
        proc = _run_completed_command(
            [str(part) for part in step.cmd],
            cwd=step.cwd,
            env=env,
            capture_output=True,
            memory_guard_prefix=guard_prefix,
        )
        duration_s = round(time.perf_counter() - start, 6)
        entry: dict[str, Any] = {
            "name": step.name,
            "category": step.category,
            "cwd": str(step.cwd),
            "cmd": step.cmd,
            "returncode": proc.returncode,
            "duration_s": duration_s,
        }
        if proc.stdout:
            entry["stdout"] = proc.stdout
        if proc.stderr:
            entry["stderr"] = proc.stderr
        results.append(entry)
        if verbose and not json_output:
            print(
                f"[molt validate] {step.name} finished "
                f"(rc={proc.returncode}, {duration_s:.2f}s)",
                file=sys.stderr,
            )
        if proc.returncode != 0:
            finished_at = dt.datetime.now(dt.timezone.utc).isoformat()
            payload = _json_payload(
                "validate",
                "error",
                data={
                    "check_only": False,
                    "started_at": started_at,
                    "finished_at": finished_at,
                    "elapsed_s": round(time.perf_counter() - validate_started, 6),
                    "suite": suite,
                    "backend": backend,
                    "profile": profile,
                    "steps": step_rows,
                    "results": results,
                    "memory_guard": guard_summary,
                },
                errors=[f"{step.name} failed with exit code {proc.returncode}"],
            )
            summary_error = _persist_validate_summary(
                payload,
                summary_path=summary_path,
            )
            if summary_error is not None:
                payload["errors"].append(summary_error)
            if json_output:
                _emit_json(payload, json_output=True)
            else:
                print(
                    f"molt validate failed at {step.name}: {shlex.join(step.cmd)}",
                    file=sys.stderr,
                )
                if proc.stderr:
                    print(proc.stderr, file=sys.stderr, end="")
                if summary_path is not None:
                    print(f"Summary: {summary_path}", file=sys.stderr)
            return proc.returncode or 1

    finished_at = dt.datetime.now(dt.timezone.utc).isoformat()
    payload = _json_payload(
        "validate",
        "ok",
        data={
            "check_only": False,
            "started_at": started_at,
            "finished_at": finished_at,
            "elapsed_s": round(time.perf_counter() - validate_started, 6),
            "suite": suite,
            "backend": backend,
            "profile": profile,
            "steps": step_rows,
            "results": results,
            "memory_guard": guard_summary,
        },
    )
    summary_error = _persist_validate_summary(payload, summary_path=summary_path)
    if summary_error is not None:
        payload["status"] = "error"
        payload["errors"].append(summary_error)
    if json_output:
        _emit_json(payload, json_output=True)
    else:
        print("Validation succeeded:")
        for result in results:
            print(
                f"- [{result['category']}] {result['name']} "
                f"(rc={result['returncode']}, {result['duration_s']:.2f}s)"
            )
        print("Memory guard:")
        for prefix, limits in guard_summary.items():
            print(_format_validate_guard_summary(prefix, limits))
        if summary_path is not None:
            print(f"Summary: {summary_path}")
    return 1 if summary_error is not None else 0
