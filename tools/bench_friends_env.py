import datetime as dt
import os
import sys
from collections.abc import Mapping
from pathlib import Path

from molt.process_environment import (
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from tools.bench_friends_context import REPO_ROOT

from tools import harness_memory_guard


def _emit_progress(message: str) -> None:
    stamp = dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")
    # Windows PowerShell surfaces native stderr as noisy error records even when
    # the child exits successfully. Keep heartbeat lines calm there.
    stream = sys.stdout if os.name == "nt" else sys.stderr
    print(f"bench_friends: {stamp} {message}", file=stream, flush=True)


_PASSTHROUGH_ENV_KEYS = {
    "CC",
    "COMSPEC",
    "CFLAGS",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "CXX",
    "CXXFLAGS",
    "DevEnvDir",
    "ExtensionSdkDir",
    "Framework40Version",
    "FrameworkDir",
    "FrameworkDir64",
    "FrameworkVersion",
    "FrameworkVersion64",
    "HOME",
    "IFCPATH",
    "INCLUDE",
    "LANG",
    "LC_ALL",
    "LD_LIBRARY_PATH",
    "LIB",
    "LIBRARY_PATH",
    "LIBPATH",
    "LOCALAPPDATA",
    "NETFXSDKDir",
    "PATH",
    "PATHEXT",
    "Platform",
    "PROCESSOR_ARCHITECTURE",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "REQUESTS_CA_BUNDLE",
    "SDKROOT",
    "SHELL",
    "SSL_CERT_FILE",
    "SystemRoot",
    "TERM",
    "USER",
    "UCRTVersion",
    "UniversalCRTSdkDir",
    "VCIDEInstallDir",
    "VCINSTALLDIR",
    "VCToolsInstallDir",
    "VCToolsRedistDir",
    "VCToolsVersion",
    "VisualStudioVersion",
    "VSINSTALLDIR",
    "VSCMD_ARG_HOST_ARCH",
    "VSCMD_ARG_TGT_ARCH",
    "VSCMD_VER",
    "windir",
    "WindowsLibPath",
    "WindowsSdkBinPath",
    "WindowsSdkDir",
    "WindowsSdkVerBinPath",
    "WindowsSDKLibVersion",
    "WindowsSDKVersion",
}

_PASSTHROUGH_ENV_PREFIXES = (
    "MOLT_BENCH_",
    "MOLT_MEMORY_GUARD_",
)


def _default_output_root() -> Path:
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return REPO_ROOT / "bench" / "results" / "friends" / timestamp


def _project_python() -> str:
    suffix = "Scripts/python.exe" if os.name == "nt" else "bin/python"
    venv = os.environ.get("VIRTUAL_ENV", "").strip()
    candidates: list[Path] = []
    if venv:
        candidates.append(Path(venv) / suffix)
    candidates.append(REPO_ROOT / ".venv" / suffix)
    if sys.prefix != getattr(sys, "base_prefix", sys.prefix):
        candidates.append(Path(sys.prefix) / suffix)
    for candidate in candidates:
        if candidate.exists():
            return str(candidate)
    return sys.executable


_FILE_ENV_PATH_KEYS = {
    "CACHEDB",
    "MOLT_GUARD_PROFILE_LOG",
}
_FILE_ENV_PATH_SUFFIXES = (
    "_DB",
    "_FILE",
    "_JSON",
    "_LOG",
    "_SQLITE",
    "_SQLITE3",
)
_DIR_ENV_PATH_KEYS = {
    "MOLT_CACHE",
    "TMP",
    "TEMP",
    "TMPDIR",
    "UV_CACHE_DIR",
    "XDG_CACHE_HOME",
}
_DIR_ENV_PATH_SUFFIXES = (
    "_DIR",
    "_DIRS",
    "_HOME",
    "_ROOT",
    "_ROOTS",
)


def _path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError:
        return False
    return True


def _materialize_output_env_paths(env: Mapping[str, str], *, output_root: Path) -> None:
    """Create output-root env path custody before a suite process starts."""
    output_root = output_root.resolve()
    env = ProcessEnvironment(env)
    file_keys = ProcessEnvironment(
        dict.fromkeys(_FILE_ENV_PATH_KEYS, ""), rule=env.rule
    )
    dir_keys = ProcessEnvironment(dict.fromkeys(_DIR_ENV_PATH_KEYS, ""), rule=env.rule)
    for key, value in env.items():
        if not value:
            continue
        normalized_key = env.rule.key(key)
        is_file_path = key in file_keys or normalized_key.endswith(
            _FILE_ENV_PATH_SUFFIXES
        )
        is_dir_path = key in dir_keys or normalized_key.endswith(_DIR_ENV_PATH_SUFFIXES)
        if not is_file_path and not is_dir_path:
            continue
        candidates = (
            [part for part in value.split(os.pathsep) if part]
            if is_dir_path
            else [value]
        )
        for candidate in candidates:
            candidate_path = Path(candidate)
            if not candidate_path.is_absolute():
                candidate_path = (REPO_ROOT / candidate_path).resolve()
            if not _path_is_under(candidate_path, output_root):
                continue
            if is_file_path:
                candidate_path.parent.mkdir(parents=True, exist_ok=True)
            else:
                candidate_path.mkdir(parents=True, exist_ok=True)


def _base_run_env() -> ProcessEnvironment:
    source = ProcessEnvironment(os.environ)
    allowed = ProcessEnvironment(
        dict.fromkeys(
            _PASSTHROUGH_ENV_KEYS | set(harness_memory_guard.CANONICAL_RUN_ENV_KEYS),
            "",
        ),
        rule=source.rule,
    )
    inherited = {
        key: value
        for key, value in source.items()
        if key in allowed
        or any(
            source.rule.key(key).startswith(prefix)
            for prefix in _PASSTHROUGH_ENV_PREFIXES
        )
    }
    env = ProcessEnvironmentBuilder(
        harness_memory_guard.canonical_harness_env(
            inherited,
            repo_root=REPO_ROOT,
        ),
        rule=source.rule,
    )
    env["PYTHONHASHSEED"] = "0"
    env["PYTHONUNBUFFERED"] = "1"
    env["PYTHONNOUSERSITE"] = "1"
    if tmpdir := env.get("TMPDIR"):
        env["TMP"] = tmpdir
        env["TEMP"] = tmpdir
    env.pop("PYTHONPATH", None)
    return ProcessEnvironment(env)
