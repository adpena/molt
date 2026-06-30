#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import tarfile
import urllib.request
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.llvm_toolchain import (  # noqa: E402
    LlvmToolchainConfigError,
    default_llvm_release,
    llvm_sys_prefix_env_var_for_version,
    required_llvm_backend_pin,
)


def _required_llvm_major(root: Path) -> int:
    pin = required_llvm_backend_pin(root)
    if pin is None:
        raise SystemExit(f"Unable to find LLVM backend feature pin under {root}")
    return pin.major


def _default_release_for_major(major: int) -> str:
    return default_llvm_release(major)


def _llvm_sys_prefix_env_var(version: str) -> str:
    return llvm_sys_prefix_env_var_for_version(version)


def _run(cmd: list[str], *, cwd: Path | None, env: dict[str, str]) -> None:
    printable = " ".join(_quote(part) for part in cmd)
    print(f"[bootstrap-llvm] {printable}", flush=True)
    proc = subprocess.run(cmd, cwd=cwd, env=env, check=False)
    if proc.returncode != 0:
        location = f" in {cwd}" if cwd is not None else ""
        raise SystemExit(
            f"Command failed with exit code {proc.returncode}{location}: {printable}"
        )


def _quote(value: str) -> str:
    if not value or any(ch.isspace() for ch in value):
        return '"' + value.replace('"', '\\"') + '"'
    return value


def _which_required(name: str) -> str:
    resolved = shutil.which(name)
    if resolved is None:
        raise SystemExit(f"Required executable not found on PATH: {name}")
    return resolved


def _vswhere_path() -> Path | None:
    candidates = [
        Path(os.environ.get("ProgramFiles(x86)", ""))
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe",
        Path(os.environ.get("ProgramFiles", ""))
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe",
    ]
    return next((path for path in candidates if path.exists()), None)


def _visual_studio_installation() -> Path | None:
    vswhere = _vswhere_path()
    if vswhere is None:
        return None
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
    )
    if proc.returncode != 0:
        return None
    path = proc.stdout.strip().splitlines()
    if not path:
        return None
    install = Path(path[0])
    return install if install.exists() else None


def _windows_msvc_env(base: dict[str, str]) -> dict[str, str]:
    if platform.system() != "Windows" or shutil.which("cl", path=base.get("PATH")):
        return base
    install = _visual_studio_installation()
    if install is None:
        raise SystemExit(
            "MSVC Build Tools were not found. Install Visual Studio Build Tools "
            "with the x64 C++ toolchain before building LLVM for the MSVC Rust target."
        )
    vsdevcmd = install / "Common7" / "Tools" / "VsDevCmd.bat"
    if not vsdevcmd.exists():
        raise SystemExit(f"Visual Studio developer command file not found: {vsdevcmd}")
    command = f'"{vsdevcmd}" -arch=x64 -host_arch=x64 >nul && set'
    proc = subprocess.run(
        ["cmd.exe", "/d", "/s", "/c", command],
        check=False,
        capture_output=True,
        text=True,
        env=base,
    )
    if proc.returncode != 0:
        raise SystemExit(proc.stderr.strip() or "Failed to activate VsDevCmd.bat")
    env = base.copy()
    for line in proc.stdout.splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        env[key] = value
    if shutil.which("cl", path=env.get("PATH")) is None:
        raise SystemExit("VsDevCmd.bat completed, but cl.exe is still not on PATH")
    return env


def _download(url: str, archive: Path) -> None:
    archive.parent.mkdir(parents=True, exist_ok=True)
    if archive.exists():
        print(f"[bootstrap-llvm] using cached archive {archive}", flush=True)
        return
    tmp = archive.with_suffix(archive.suffix + ".partial")
    print(f"[bootstrap-llvm] downloading {url}", flush=True)
    with urllib.request.urlopen(url) as response, tmp.open("wb") as fh:
        shutil.copyfileobj(response, fh)
    tmp.replace(archive)


def _safe_extract_tar_xz(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        print(f"[bootstrap-llvm] using existing source tree {destination}", flush=True)
        return
    with tarfile.open(archive, "r:xz") as tf:
        members = tf.getmembers()
        for member in members:
            target = (destination / member.name).resolve()
            try:
                target.relative_to(destination.resolve())
            except ValueError as exc:
                raise SystemExit(
                    f"Archive contains unsafe path: {member.name}"
                ) from exc
        tf.extractall(destination, members=members)


def _llvm_source_root(extract_root: Path, version: str) -> Path:
    direct = extract_root / f"llvm-project-llvmorg-{version}" / "llvm"
    if direct.exists():
        return direct
    matches = sorted(extract_root.glob("llvm-project-*/llvm"))
    if matches:
        return matches[0]
    raise SystemExit(f"Unable to find extracted LLVM source under {extract_root}")


def _verify_llvm_config(prefix: Path, version: str) -> Path:
    exe = "llvm-config.exe" if platform.system() == "Windows" else "llvm-config"
    llvm_config = prefix / "bin" / exe
    if not llvm_config.exists():
        raise SystemExit(f"LLVM install did not produce {llvm_config}")
    proc = subprocess.run(
        [str(llvm_config), "--version"],
        check=False,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise SystemExit(proc.stderr.strip() or f"{llvm_config} --version failed")
    expected = ".".join(version.split(".")[:2])
    actual = proc.stdout.strip()
    if not actual.startswith(expected + ".") and actual != expected:
        raise SystemExit(f"{llvm_config} reports {actual}; expected LLVM {expected}.x")
    return llvm_config


def main(argv: list[str] | None = None) -> int:
    try:
        major = _required_llvm_major(ROOT)
    except LlvmToolchainConfigError as exc:
        raise SystemExit(str(exc)) from exc
    default_version = _default_release_for_major(major)
    parser = argparse.ArgumentParser(
        description="Build and install a complete LLVM dev prefix for Molt."
    )
    parser.add_argument("--version", default=default_version)
    parser.add_argument(
        "--prefix",
        type=Path,
        default=ROOT / "target" / "toolchains" / f"llvm-{default_version}",
    )
    parser.add_argument(
        "--archive",
        type=Path,
        default=None,
        help="Cached llvm-project tar.xz path.",
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        default=None,
        help="Extraction root containing llvm-project-llvmorg-<version>/llvm.",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        default=None,
        help="CMake build directory.",
    )
    parser.add_argument("--targets", default="X86;WebAssembly")
    parser.add_argument("--build-type", default="Release")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--configure-only", action="store_true")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Only verify an existing prefix and print the required env var.",
    )
    args = parser.parse_args(argv)

    prefix = args.prefix.resolve()
    env_var = _llvm_sys_prefix_env_var(args.version)
    if args.check:
        llvm_config = _verify_llvm_config(prefix, args.version)
        print(f"{env_var}={prefix}")
        print(f"llvm-config={llvm_config}")
        return 0

    _which_required("cmake")
    _which_required("ninja")
    env = _windows_msvc_env(os.environ.copy())

    archive = (
        args.archive.resolve()
        if args.archive is not None
        else ROOT
        / "target"
        / "toolchains"
        / "downloads"
        / f"llvm-project-{args.version}.tar.xz"
    )
    source_root = (
        args.source_root.resolve()
        if args.source_root is not None
        else ROOT / "tmp" / "toolchains" / f"llvm-project-{args.version}"
    )
    build_dir = (
        args.build_dir.resolve()
        if args.build_dir is not None
        else ROOT / "target" / "toolchains" / "build" / f"llvm-{args.version}"
    )
    url = (
        "https://github.com/llvm/llvm-project/releases/download/"
        f"llvmorg-{args.version}/llvm-project-{args.version}.src.tar.xz"
    )
    _download(url, archive)
    _safe_extract_tar_xz(archive, source_root)
    llvm_source = _llvm_source_root(source_root, args.version)
    build_dir.mkdir(parents=True, exist_ok=True)
    prefix.parent.mkdir(parents=True, exist_ok=True)

    cmake_configure = [
        "cmake",
        "-S",
        str(llvm_source),
        "-B",
        str(build_dir),
        "-G",
        "Ninja",
        f"-DCMAKE_BUILD_TYPE={args.build_type}",
        f"-DCMAKE_INSTALL_PREFIX={prefix}",
        f"-DLLVM_TARGETS_TO_BUILD={args.targets}",
        "-DLLVM_ENABLE_ASSERTIONS=ON",
        "-DLLVM_INCLUDE_BENCHMARKS=OFF",
        "-DLLVM_INCLUDE_DOCS=OFF",
        "-DLLVM_INCLUDE_EXAMPLES=OFF",
        "-DLLVM_INCLUDE_TESTS=OFF",
        "-DLLVM_INSTALL_UTILS=ON",
    ]
    _run(cmake_configure, cwd=ROOT, env=env)
    if args.configure_only:
        print(f"[bootstrap-llvm] configured {build_dir}")
        return 0
    _run(
        [
            "cmake",
            "--build",
            str(build_dir),
            "--target",
            "install",
            "--",
            "-j",
            str(args.jobs),
        ],
        cwd=ROOT,
        env=env,
    )
    llvm_config = _verify_llvm_config(prefix, args.version)
    print(f"[bootstrap-llvm] installed {llvm_config}")
    print(f"{env_var}={prefix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
