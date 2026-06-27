from __future__ import annotations

import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Literal, Mapping, Sequence

from molt.dx import DX_ENV_KEYS, DxProject
from molt.cli.backend_daemon_config import _backend_daemon_enabled
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.models import _ToolchainReport
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_molt_root, _require_molt_root
from molt.cli.runtime_paths import _runtime_lib_path


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
            str(prefix / "bin" / name) for name in _llvm_config_names(major)
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


def _clang_llvm_version_detail(major: int) -> str | None:
    clang = shutil.which("clang") or shutil.which("clang.exe")
    if not clang:
        return None
    try:
        result = subprocess.run(
            [clang, "--version"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    first_line = (result.stdout or result.stderr).splitlines()[0:1]
    version_text = first_line[0] if first_line else ""
    match = re.search(r"\b(?:clang|LLVM)\s+version\s+(\d+)(?:\.|$)", version_text)
    if match is None or int(match.group(1)) != major:
        return None
    return (
        f"LLVM {major} clang is present at {clang}, but matching llvm-config was "
        "not found; llvm-sys requires llvm-config, and the Windows LLVM installer "
        "often omits it"
    )


def _llvm_backend_unavailable_message(root: Path) -> str | None:
    major, llvm_toolchain = _detect_llvm_backend_toolchain(root)
    if major is None or llvm_toolchain is not None:
        return None
    env_var = _llvm_sys_prefix_env_var(major)
    advice = "\n".join(f"  - {item}" for item in _llvm_backend_advice(major))
    detail = _clang_llvm_version_detail(major) or "No matching llvm-config was found."
    return (
        f"LLVM backend requires LLVM {major}.1 with llvm-config. "
        f"{detail}\n"
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
        return [
            "winget install BytecodeAlliance.Wasmtime",
            "or: cargo install wasmtime-cli --locked",
        ]
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
        candidate = Path(root) / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
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
    first = next(
        (line.strip() for line in proc.stdout.splitlines() if line.strip()), ""
    )
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
    env = DxProject(root).dx_env(os.environ, create_dirs=False)
    return {key: env[key] for key in DX_ENV_KEYS if key in env}


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
        llvm_detail = (
            f"LLVM {llvm_major} via {llvm_toolchain}"
            if llvm_toolchain is not None
            else _clang_llvm_version_detail(llvm_major)
            or f"LLVM {llvm_major} toolchain not found"
        )
        record(
            "llvm-backend-toolchain",
            llvm_toolchain is not None,
            llvm_detail,
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
                "Maintainer/agent DX: export CARGO_TARGET_DIR=<external>/target",
                "Maintainer/agent DX: export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR",
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
            advice=["Maintainer/agent DX: export MOLT_CACHE=<external>/molt_cache"],
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
            advice=[
                "Maintainer/agent DX: export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
            ],
        )
    elif cargo_target_dir is not None and diff_target_dir != cargo_target_dir:
        record(
            "molt-diff-target-dir",
            False,
            f"{diff_target_dir} (CARGO_TARGET_DIR={cargo_target_dir})",
            level="warning",
            advice=[
                "Maintainer/agent DX: set MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
            ],
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
                "Maintainer/agent DX: export MOLT_EXT_ROOT=<artifact-root>",
                "Maintainer/agent DX: export CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target",
                "Maintainer/agent DX: export MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache",
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
            cargo_path
            and wasm_target_ok
            and (zig_path or wasm_ld_path)
            and wasm_tools_path
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


def _toolchain_report_status(report: _ToolchainReport) -> str:
    return "ok" if not report.errors else "error"


def _toolchain_report_data(report: _ToolchainReport) -> dict[str, Any]:
    return {
        "checks": report.checks,
        "environment": report.environment,
        "actions": report.actions,
        "backends": report.backends,
        "profiles": report.profiles,
    }


def _toolchain_report_json_payload(
    command: str, report: _ToolchainReport
) -> dict[str, Any]:
    return _json_payload(
        command,
        _toolchain_report_status(report),
        data=_toolchain_report_data(report),
        warnings=report.warnings,
        errors=report.errors,
    )


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
    if json_output:
        _emit_json(_toolchain_report_json_payload("setup", report), json_output=True)
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
    if json_output:
        _emit_json(_toolchain_report_json_payload("doctor", report), json_output=True)
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
    if strict and report.errors:
        return 1
    return 0
