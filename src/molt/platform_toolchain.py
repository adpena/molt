"""Select platform SDKs and their real native tools before build admission."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
import platform
import re
import shutil

from molt import process_guard
from molt.llvm_toolchain import llvm_host_architecture
from molt.toolchain_identity import (
    resolve_executable,
    stable_regular_file_content_identity,
)

SDK_ENVIRONMENT_NAMES = frozenset(
    {
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "SDKROOT",
        "DEVELOPER_DIR",
        "MACOSX_DEPLOYMENT_TARGET",
        "VCINSTALLDIR",
        "VSINSTALLDIR",
        "VCTOOLSINSTALLDIR",
        "VCTOOLSVERSION",
        "WINDOWSSDKDIR",
        "WINDOWSSDKVERSION",
        "UNIVERSALCRTSDKDIR",
        "UCRTVERSION",
        "VSCMD_ARG_TGT_ARCH",
        "VSCMD_ARG_HOST_ARCH",
    }
)


def _query(argv: list[str], env: Mapping[str, str], *, timeout: int = 30) -> str:
    result = process_guard.run_completed_command(
        argv,
        env=dict(env),
        capture_output=True,
        text=True,
        check=True,
        timeout=timeout,
        memory_guard_prefix="MOLT_PLATFORM_TOOLCHAIN",
    )
    return result.stdout.strip()


def visual_studio_installation(component: str, env: Mapping[str, str]) -> Path | None:
    """One bounded vswhere selection shared by setup advice and real builds."""
    folded = {key.upper(): value for key, value in env.items()}
    for name in ("PROGRAMFILES(X86)", "PROGRAMFILES"):
        if not folded.get(name):
            continue
        vswhere = Path(folded[name]) / "Microsoft Visual Studio/Installer/vswhere.exe"
        if not vswhere.is_file():
            continue
        selected = _query(
            [
                str(vswhere),
                "-latest",
                "-products",
                "*",
                "-requires",
                component,
                "-property",
                "installationPath",
            ],
            env,
        )
        if not selected:
            return None
        if len(selected.splitlines()) != 1:
            raise ValueError(
                "vswhere must select exactly one Visual Studio installation"
            )
        install = Path(selected)
        if not install.is_absolute() or not install.is_dir():
            raise ValueError(f"Visual Studio installation is unavailable: {selected}")
        return install.resolve(strict=True)
    return None


def activate_msvc_environment(
    base: Mapping[str, str], *, repo_root: Path, machine: str | None = None
) -> dict[str, str]:
    """Activate one architecture-selected installation; ATL is a caller policy."""
    if platform.system() != "Windows":
        return dict(base)
    host = llvm_host_architecture(repo_root, machine or platform.machine())
    if host is None:
        raise ValueError(
            "native MSVC architecture is absent from config/llvm_toolchain_arches.toml"
        )
    component = host.windows_component
    target_arch = host.windows_target_arch
    host_arch = host.windows_host_arch
    if not component or not target_arch or not host_arch:
        raise ValueError(
            "native MSVC architecture is absent from config/llvm_toolchain_arches.toml"
        )
    env = {
        key.upper(): value
        for key, value in base.items()
        if key.upper() not in SDK_ENVIRONMENT_NAMES
        and not key.upper().startswith(("VSCMD_", "__VSCMD_"))
    }
    install = visual_studio_installation(component, env)
    if install is None:
        raise ValueError(
            f"MSVC Build Tools require component {component} for {host.id}"
        )
    script = install / "Common7/Tools/VsDevCmd.bat"
    if not script.is_file():
        raise ValueError(
            f"Visual Studio developer command file is unavailable: {script}"
        )
    activation_name = "MOLT_VSDEVCMD_CALL"
    env[activation_name] = f'"{script}"'
    command = f"call %{activation_name}% -arch={target_arch} -host_arch={host_arch} >nul && set"
    comspec = resolve_executable(
        env.get("COMSPEC", "cmd.exe"),
        environment=env,
        label="MSVC environment command host",
    )
    observed = _query([str(comspec), "/d", "/s", "/c", command], env, timeout=120)
    for line in observed.splitlines():
        if "=" in line and not line.startswith("="):
            key, value = line.split("=", 1)
            env[key.upper()] = value
    env.pop(activation_name, None)
    if (
        env.get("VSCMD_ARG_TGT_ARCH", "").lower() != target_arch.lower()
        or env.get("VSCMD_ARG_HOST_ARCH", "").lower() != host_arch.lower()
    ):
        raise ValueError(
            "Visual Studio activation selected a different host or target architecture"
        )
    for key, expected in (("VSINSTALLDIR", install), ("VCINSTALLDIR", install / "VC")):
        if not env.get(key) or Path(env[key]).resolve(strict=True) != expected.resolve(
            strict=True
        ):
            raise ValueError(
                f"Visual Studio activation did not bind {key} to {install}"
            )
    if shutil.which("cl", path=env.get("PATH")) is None:
        raise ValueError("Visual Studio activation did not select cl.exe")
    return env


@dataclass(frozen=True)
class DarwinToolchain:
    developer_dir: Path
    sdk_root: Path
    sdk_version: str
    deployment_target: str
    sdk_settings_sha256: str
    tools: dict[str, Path]

    def environment(self) -> dict[str, str]:
        return {
            "DEVELOPER_DIR": str(self.developer_dir),
            "SDKROOT": str(self.sdk_root),
            "MACOSX_DEPLOYMENT_TARGET": self.deployment_target,
        }

    def versions(self) -> dict[str, str]:
        return {
            "sdk": self.sdk_version,
            "deployment_target": self.deployment_target,
            "sdk_settings_sha256": self.sdk_settings_sha256,
        }


def select_darwin_toolchain(
    env: Mapping[str, str], *, developer_dir: Path | None = None
) -> DarwinToolchain:
    """Pin xcode-select once, then resolve every xcrun shim in that selection."""
    selected_env = dict(env)
    if developer_dir is None:
        selector = resolve_executable(
            "xcode-select", environment=selected_env, label="Darwin developer selector"
        )
        selected = _query([str(selector), "--print-path"], selected_env)
        if not selected or "\n" in selected:
            raise ValueError("xcode-select must select exactly one developer directory")
        developer_dir = Path(selected)
    if not developer_dir.is_absolute() or not developer_dir.is_dir():
        raise ValueError(f"Darwin developer directory is unavailable: {developer_dir}")
    developer_dir = developer_dir.resolve(strict=True)
    selected_env["DEVELOPER_DIR"] = str(developer_dir)
    xcrun = resolve_executable(
        "xcrun", environment=selected_env, label="Darwin SDK selector"
    )

    def query(*args: str) -> str:
        value = _query([str(xcrun), "--sdk", "macosx", *args], selected_env)
        if not value or "\n" in value:
            raise ValueError(f"xcrun must select one value for {args}")
        return value

    sdk = Path(query("--show-sdk-path")).resolve(strict=True)
    if not sdk.is_dir():
        raise ValueError(f"Darwin SDK directory is unavailable: {sdk}")
    version = query("--show-sdk-version")
    if re.fullmatch(r"\d+\.\d+(?:\.\d+)?", version) is None:
        raise ValueError(f"Darwin SDK version is invalid: {version}")
    selected_env["SDKROOT"] = str(sdk)
    tools = {}
    for name in ("clang", "clang++", "ar", "ranlib", "ld"):
        path = resolve_executable(
            query("--find", name), environment=selected_env, label=f"Darwin {name}"
        )
        if not path.resolve(strict=True).is_relative_to(developer_dir):
            raise ValueError(
                f"xcrun selected {name} outside the pinned developer directory: {path}"
            )
        tools[name] = path
    settings = stable_regular_file_content_identity(
        sdk / "SDKSettings.json", label="Darwin SDK settings"
    )
    # Release binaries target the selected SDK version; binary compatibility
    # auditing derives the distributed OS floor from the actual linked images.
    return DarwinToolchain(
        developer_dir, sdk, version, version, str(settings["sha256"]), tools
    )
