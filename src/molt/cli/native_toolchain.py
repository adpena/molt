from __future__ import annotations

import importlib
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.compiler_metadata import _compiler_root


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _run_completed_command(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._run_completed_command(*args, **kwargs)


def _emit_json(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._emit_json(*args, **kwargs)


def _json_payload(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._json_payload(*args, **kwargs)


def _coerce_bool(*args: Any, **kwargs: Any) -> bool:
    return _cli_module()._coerce_bool(*args, **kwargs)


def _codesign_binary(binary_path: Path) -> None:
    """Ad-hoc codesign a binary on macOS."""
    if sys.platform != "darwin":
        return
    try:
        _run_completed_command(
            ["codesign", "-f", "-s", "-", str(binary_path)],
            capture_output=True,
            env=None,
            cwd=binary_path.parent,
            memory_guard_prefix="MOLT_BUILD",
            timeout=10,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired, OSError):
        pass


def _run_bolt_post_link(
    *,
    bolt_requested: bool,
    bolt_training_cmd: str | None,
    target: str,
    output: str | None,
    out_dir: str | None,
    build_rc: int,
    json_output: bool,
) -> int:
    """Run BOLT post-link optimization after a successful native build.

    Returns 0 when BOLT was not requested or ran successfully, or a nonzero
    return code on failure.
    """
    if not bolt_requested:
        return 0
    if build_rc != 0:
        return 0  # build already failed — skip BOLT

    # BOLT only applies to native targets.
    is_native = target in {"native"} or (
        target is not None
        and "-" in target
        and "wasm" not in target
        and "luau" not in target
    )
    if not is_native:
        if not json_output:
            print(
                "Warning: --bolt is only supported for native targets; skipping.",
                file=sys.stderr,
            )
        return 0

    # Locate the BOLT wrapper script.
    bolt_script = _compiler_root() / "tools" / "bolt_optimize.sh"
    if not bolt_script.exists():
        msg = f"BOLT script not found: {bolt_script}"
        if json_output:
            _emit_json(
                _json_payload("build", "error", errors=[msg]),
                json_output,
            )
        else:
            print(msg, file=sys.stderr)
        return 1

    # Determine the output binary path.  When --output is given we can
    # resolve it directly; otherwise BOLT requires it explicitly because
    # the default output path lives inside internal build state.
    if output:
        binary_path = Path(output).expanduser()
        if not binary_path.is_absolute():
            base = Path(out_dir) if out_dir else Path.cwd()
            binary_path = base / binary_path
    else:
        if not json_output:
            print(
                "Error: --bolt requires an explicit --output path so BOLT "
                "can locate the binary.",
                file=sys.stderr,
            )
        return 1

    if not binary_path.exists():
        msg = f"BOLT: output binary not found at {binary_path}"
        if not json_output:
            print(msg, file=sys.stderr)
        return 1

    # Build the bolt command.
    bolt_cmd: list[str] = ["bash", str(bolt_script), str(binary_path)]
    if bolt_training_cmd:
        bolt_cmd.append(bolt_training_cmd)

    if not json_output:
        print(
            f"==> Running BOLT post-link optimization on {binary_path}...",
            file=sys.stderr,
        )

    try:
        bolt_proc = _run_completed_command(
            bolt_cmd,
            cwd=binary_path.parent,
            env=None,
            capture_output=not json_output,
            memory_guard_prefix="MOLT_BUILD",
            timeout=300,  # 5 min ceiling
        )
    except FileNotFoundError:
        if not json_output:
            print("BOLT: bash not found", file=sys.stderr)
        return 1
    except subprocess.TimeoutExpired:
        if not json_output:
            print("BOLT: optimization timed out (300s)", file=sys.stderr)
        return 1

    if bolt_proc.returncode != 0:
        if not json_output:
            stderr_text = (
                bolt_proc.stderr
                if isinstance(bolt_proc.stderr, str)
                else (
                    bolt_proc.stderr.decode("utf-8", errors="replace")
                    if bolt_proc.stderr
                    else ""
                )
            )
            if stderr_text:
                print(stderr_text, file=sys.stderr)
            print("BOLT optimization failed", file=sys.stderr)
        return bolt_proc.returncode

    # Replace the original binary with the BOLT-optimized one.
    bolt_binary = Path(f"{binary_path}.bolt")
    if bolt_binary.exists():
        try:
            _atomic_copy_file(bolt_binary, binary_path, codesign=True)
            bolt_binary.unlink()
        except OSError as exc:
            if not json_output:
                print(
                    f"BOLT: failed to publish optimized binary: {exc}",
                    file=sys.stderr,
                )
            return 1
        if not json_output:
            print(
                f"==> BOLT-optimized binary installed: {binary_path}",
                file=sys.stderr,
            )

    return 0


def _strip_arch_flags(args: list[str]) -> list[str]:
    cleaned: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg == "-arch":
            skip_next = True
            continue
        if arg.startswith("-arch="):
            continue
        cleaned.append(arg)
    return cleaned


def _zig_target_query(target_triple: str) -> str:
    triple = target_triple.strip()
    if not triple:
        return target_triple
    parts = [part for part in triple.split("-") if part]
    if len(parts) < 2:
        return target_triple

    arch_aliases = {
        "amd64": "x86_64",
        "x64": "x86_64",
        "arm64": "aarch64",
        "armv7l": "armv7",
        "i386": "x86",
        "i486": "x86",
        "i586": "x86",
        "i686": "x86",
    }
    os_aliases = {
        "darwin": "macos",
        "macosx": "macos",
        "win32": "windows",
        "mingw32": "windows",
        "mingw64": "windows",
        "cygwin": "windows",
    }
    abi_aliases = {
        "sim": "simulator",
        "androideabi": "android",
    }
    abi_tokens = {
        "gnu",
        "gnueabi",
        "gnueabihf",
        "gnuabi64",
        "gnux32",
        "musl",
        "musleabi",
        "musleabihf",
        "msvc",
        "eabi",
        "eabihf",
        "android",
        "simulator",
        "sim",
        "ilp32",
        "uclibc",
        "ohos",
        "macabi",
        "androideabi",
    }
    os_tokens = {
        "linux",
        "windows",
        "darwin",
        "macos",
        "macosx",
        "ios",
        "tvos",
        "watchos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "haiku",
        "hurd",
        "android",
        "wasi",
        "emscripten",
        "fuchsia",
        "uefi",
        "mingw32",
        "mingw64",
        "cygwin",
        "illumos",
        "aix",
    }

    def is_os_token(token: str) -> bool:
        lowered = token.lower()
        return lowered in os_tokens or lowered in os_aliases

    arch = arch_aliases.get(parts[0].lower(), parts[0].lower())
    remainder = [part.lower() for part in parts[1:]]
    abi = None
    if remainder:
        last = remainder[-1]
        if len(remainder) >= 2 and last in abi_tokens and is_os_token(remainder[-2]):
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
        elif last in abi_tokens and last not in os_tokens:
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
    os_part = remainder[-1] if remainder else None
    vendor_parts = remainder[:-1] if len(remainder) > 1 else []
    if os_part is None:
        return f"{arch}-{abi}" if abi else arch
    os_token = os_part.lower()
    match = re.match(r"^(darwin|macosx|macos|ios|tvos|watchos)([0-9].*)$", os_token)
    if match:
        os_token = match.group(1)
    os_name = os_aliases.get(os_token, os_token)
    if os_name in {"unknown", "none"}:
        os_name = "freestanding"
    if os_name == "windows" and abi is None:
        if any(token in {"w64", "mingw32", "mingw64"} for token in vendor_parts):
            abi = "gnu"
    if os_name in {"mingw32", "mingw64"}:
        os_name = "windows"
        if abi is None:
            abi = "gnu"
    if os_name in {"macos", "ios", "tvos", "watchos"}:
        if abi == "sim":
            abi = "simulator"
        elif os_name == "macos":
            abi = None
        elif abi in {
            "gnu",
            "gnueabi",
            "gnueabihf",
            "gnuabi64",
            "gnux32",
            "musl",
            "musleabi",
            "musleabihf",
            "msvc",
            "android",
            "eabi",
            "eabihf",
            "uclibc",
        }:
            abi = None

    if abi:
        return f"{arch}-{os_name}-{abi}"
    return f"{arch}-{os_name}"


def _detect_macos_arch(obj_path: Path) -> str | None:
    try:
        result = _run_completed_command(
            ["lipo", "-archs", str(obj_path)],
            capture_output=True,
            env=None,
            cwd=obj_path.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    archs = result.stdout.strip().split()
    return archs[0] if archs else None


def _detect_macos_deployment_target(arch: str | None = None) -> str | None:
    env_target = os.environ.get("MOLT_MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    env_target = os.environ.get("MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    # Stable per-arch baselines when no environment override is present.
    if arch in ("x86_64", "amd64"):
        return "10.13"
    # arm64, aarch64, and any unknown arch: use the SDK version reported
    # by xcrun, which matches what Rust/C dependencies were compiled
    # against.  Using platform.mac_ver() (OS version) can be lower than
    # the SDK, causing hundreds of linker version-mismatch warnings.
    try:
        result = _run_completed_command(
            ["xcrun", "--show-sdk-version"],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
            timeout=5,
        )
        sdk_ver = result.stdout.strip()
        if sdk_ver:
            return sdk_ver
    except (subprocess.SubprocessError, FileNotFoundError):
        pass
    # Fallback to OS version if xcrun unavailable
    import platform as _platform

    ver = _platform.mac_ver()[0]
    if ver:
        parts = ver.split(".")
        return ".".join(parts[:2]) if len(parts) >= 2 else ver
    return "11.0"


def _append_darwin_runtime_frameworks(
    args: list[str],
    *,
    target_triple: str | None = None,
) -> None:
    """Append macOS framework flags when targeting Darwin.

    For cross-target builds (e.g. building x86_64-apple-darwin from an
    aarch64 host) the linker is invoked without rustc's host-SDK
    auto-discovery, so the framework search path is empty and `-framework
    Security` fails to resolve. Inject `-F <sdk>/System/Library/Frameworks`
    explicitly when we have a target triple in hand.
    """
    is_darwin = False
    if target_triple:
        is_darwin = "apple" in target_triple or "darwin" in target_triple
    else:
        is_darwin = sys.platform == "darwin"
    if is_darwin:
        # Only inject SDK paths when cross-targeting; native builds get
        # the search paths for free from rustc's default SDK probing.
        if target_triple:
            sdk_root = _resolve_macos_sdk_root()
            if sdk_root:
                # -isysroot points the linker at the cross-target SDK so
                # libSystem / libobjc / libc++ resolve from the SDK's
                # usr/lib instead of the host's /usr/lib (which is a
                # different ABI on a non-host arch).
                if "-isysroot" not in args:
                    args.extend(["-isysroot", sdk_root])
                framework_dir = f"{sdk_root}/System/Library/Frameworks"
                if framework_dir not in args:
                    args.extend(["-F", framework_dir])
                lib_dir = f"{sdk_root}/usr/lib"
                if lib_dir not in args:
                    args.extend(["-L", lib_dir])
        args.extend(["-framework", "Security", "-framework", "CoreFoundation"])
        if _coerce_bool(os.environ.get("MOLT_RUNTIME_GPU_METAL"), False):
            args.extend(["-framework", "Metal", "-lobjc"])
        if _coerce_bool(os.environ.get("MOLT_RUNTIME_GPU_WEBGPU"), False):
            args.extend(
                [
                    "-framework",
                    "Metal",
                    "-framework",
                    "Foundation",
                    "-framework",
                    "QuartzCore",
                    "-framework",
                    "AppKit",
                    "-lobjc",
                ]
            )


def _resolve_macos_sdk_root() -> str | None:
    """Return the active macOS SDK root via xcrun, or None if unavailable."""
    try:
        result = _run_completed_command(
            ["xcrun", "--sdk", "macosx", "--show-sdk-path"],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
            timeout=5,
        )
        return result.stdout.strip() or None
    except (subprocess.SubprocessError, FileNotFoundError):
        return None
