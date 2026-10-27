"""Select platform SDKs and their real native tools before build admission."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import platform
import re
import subprocess
from typing import Literal, get_args

from molt import process_guard
from molt.exact_json import loads_exact
from molt.llvm_toolchain import llvm_host_architecture
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from molt.toolchain_facts import (
    DARWIN_DEVELOPER_SELECTION_LINK,
    DARWIN_SELECTOR_ENVIRONMENT,
    FileInput,
    ProbeAnswer,
    ProbeKey,
    capture_probe_tool,
    completed_probe,
    environment_inputs,
    file_input,
    host_toolchain_facts,
)
from molt.toolchain_identity import (
    find_executable,
    resolve_executable,
    open_stable_regular_file,
)

SDK_ENVIRONMENT_NAMES = frozenset(
    {
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "SDKROOT",
        "DEVELOPER_DIR",
        "MACOSX_DEPLOYMENT_TARGET",
        "MOLT_MACOSX_DEPLOYMENT_TARGET",
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


def _completed_query(
    argv: list[str], env: Mapping[str, str], *, timeout: int = 30
) -> subprocess.CompletedProcess[str]:
    try:
        return process_guard.run_completed_command(
            argv,
            env=ProcessEnvironment(env),
            capture_output=True,
            text=True,
            check=True,
            timeout=timeout,
            memory_guard_prefix="MOLT_PLATFORM_TOOLCHAIN",
        )
    except (OSError, subprocess.SubprocessError) as exc:
        detail = str(exc)
        if isinstance(exc, subprocess.CalledProcessError):
            detail = str(exc.stderr or exc.stdout or exc)
        raise ValueError(
            f"Platform toolchain selector {argv[0]} failed: {detail[:2048]}"
        ) from exc


def _query(argv: list[str], env: Mapping[str, str], *, timeout: int = 30) -> str:
    return _completed_query(argv, env, timeout=timeout).stdout.strip()


def _darwin_selection(
    kind: str,
    selector: Path,
    argv: tuple[str, ...],
    env: Mapping[str, str],
    *,
    label: str,
    inputs: tuple[FileInput, ...],
    accept: Callable[[str], bool],
) -> str:
    """One xcode-select/xcrun answer from the toolchain facts.

    The key names the selector binary, the developer-directory link the
    ``/usr/bin`` shims dispatch on, the selector environment and the
    directories the question searches. A recorded answer is served only while
    ``accept`` holds for it; otherwise the selector runs again.
    """
    tool = capture_probe_tool(selector, label=label)
    key = ProbeKey(
        kind,
        (tool,),
        argv,
        None,
        environment_inputs(env, DARWIN_SELECTOR_ENVIRONMENT),
        (file_input(DARWIN_DEVELOPER_SELECTION_LINK), *inputs),
    )

    def run() -> subprocess.CompletedProcess[str]:
        with tool.fenced(label=label):
            return _completed_query([str(selector), *argv], env)

    def current(answer: ProbeAnswer) -> bool:
        return accept(answer.stdout.strip())

    result, _cached = completed_probe(host_toolchain_facts(), key, run, accept=current)
    return result.stdout.strip()


def _one_line(value: str) -> bool:
    return bool(value) and "\n" not in value


def _developer_inputs(developer_dir: Path, *relative: str) -> tuple[FileInput, ...]:
    """The real xcrun under the developer directory, then each searched path."""
    return (
        file_input(developer_dir / "usr" / "bin" / "xcrun"),
        *(file_input(developer_dir / path) for path in relative),
    )


def _darwin_toolchain_inputs(
    developer_dir: Path, env: Mapping[str, str]
) -> tuple[FileInput, ...]:
    """Every ``usr/bin`` an ``xcrun --find`` can select a tool from."""
    inputs = list(_developer_inputs(developer_dir, "usr/bin"))
    home = ProcessEnvironment(env).get("HOME")
    roots = [developer_dir / "Toolchains"]
    if home:
        roots.append(Path(home) / "Library" / "Developer" / "Toolchains")
    for root in roots:
        inputs.append(file_input(root))
        try:
            with os.scandir(root) as iterator:
                names = sorted(entry.name for entry in iterator)
        except OSError:
            names = []
        inputs.extend(file_input(root / name / "usr" / "bin") for name in names)
    return tuple(inputs)


def visual_studio_installation(component: str, env: Mapping[str, str]) -> Path | None:
    """One bounded vswhere selection shared by setup advice and real builds."""
    env = ProcessEnvironment(env)
    for name in ("PROGRAMFILES(X86)", "PROGRAMFILES"):
        if not env.get(name):
            continue
        vswhere = Path(env[name]) / "Microsoft Visual Studio/Installer/vswhere.exe"
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
) -> ProcessEnvironment:
    """Activate one architecture-selected installation; ATL is a caller policy."""
    if platform.system() != "Windows":
        return ProcessEnvironment(base)
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
    env = ProcessEnvironmentBuilder(base)
    for name in tuple(env):
        key = env.rule.key(name)
        if key in SDK_ENVIRONMENT_NAMES or key.startswith(("VSCMD_", "__VSCMD_")):
            del env[name]
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
    observed = _query(
        [str(comspec), "/d", "/s", "/c", command],
        ProcessEnvironment(env),
        timeout=120,
    )
    for line in observed.splitlines():
        if "=" in line and not line.startswith("="):
            key, value = line.split("=", 1)
            env[key] = value
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
    if find_executable("cl", environment=env) is None:
        raise ValueError("Visual Studio activation did not select cl.exe")
    return ProcessEnvironment(env)


@dataclass(frozen=True)
class DarwinSDK:
    developer_dir: Path | None
    sdk_root: Path
    sdk_version: str
    sdk_settings_sha256: str

    def environment(self) -> dict[str, str]:
        environment = {"SDKROOT": str(self.sdk_root)}
        if self.developer_dir is not None:
            environment["DEVELOPER_DIR"] = str(self.developer_dir)
        return environment


@dataclass(frozen=True)
class DarwinToolchain(DarwinSDK):
    developer_dir: Path
    deployment_target: str
    tools: dict[str, Path]

    def environment(self) -> dict[str, str]:
        return {
            **super().environment(),
            "MACOSX_DEPLOYMENT_TARGET": self.deployment_target,
        }

    def versions(self) -> dict[str, str]:
        return {
            "sdk": self.sdk_version,
            "deployment_target": self.deployment_target,
            "sdk_settings_sha256": self.sdk_settings_sha256,
        }


def darwin_toolchain_selector_paths(
    env: Mapping[str, str], *, needs_developer_selection: bool | None = None
) -> dict[str, Path]:
    """Exact prerequisite executables used by the Darwin selection transaction."""
    env = ProcessEnvironment(env)
    if needs_developer_selection is None:
        needs_developer_selection = not env.get("DEVELOPER_DIR")
    tools = {}
    if needs_developer_selection:
        tools["xcode-select"] = resolve_executable(
            "xcode-select", environment=env, label="Darwin developer selector"
        )
    tools["xcrun"] = resolve_executable(
        "xcrun", environment=env, label="Darwin SDK selector"
    )
    return tools


def normalize_darwin_version(value: object, *, label: str) -> str:
    if (
        not isinstance(value, str)
        or re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,2}", value) is None
    ):
        raise ValueError(f"{label} is invalid: {value!r}")
    components = [int(component) for component in value.split(".")]
    if len(components) == 1:
        components.append(0)
    while len(components) > 2 and components[-1] == 0:
        components.pop()
    return ".".join(str(component) for component in components)


def darwin_deployment_target(
    env: Mapping[str, str], *, sdk_version: str, arch: str | None = None
) -> str:
    """Explicit build policy, otherwise the native arch or release SDK floor."""
    env = ProcessEnvironment(env)
    explicit = env.get("MOLT_MACOSX_DEPLOYMENT_TARGET") or env.get(
        "MACOSX_DEPLOYMENT_TARGET"
    )
    return normalize_darwin_version(
        explicit or ("10.13" if arch in {"x86_64", "amd64"} else sdk_version),
        label="Darwin deployment target",
    )


def _darwin_developer_directory(
    env: Mapping[str, str], *, developer_dir: Path | None
) -> Path:
    env = ProcessEnvironment(env)
    if developer_dir is None and env.get("DEVELOPER_DIR"):
        developer_dir = Path(env["DEVELOPER_DIR"])
    if developer_dir is None:
        selector = resolve_executable(
            "xcode-select", environment=env, label="Darwin developer selector"
        )
        selected = _darwin_selection(
            "xcode-select-path",
            selector,
            ("--print-path",),
            env,
            label="Darwin developer selector",
            inputs=(),
            accept=lambda value: _one_line(value) and Path(value).is_dir(),
        )
        if not selected or "\n" in selected:
            raise ValueError("xcode-select must select exactly one developer directory")
        developer_dir = Path(selected)
    if not developer_dir.is_absolute() or not developer_dir.is_dir():
        raise ValueError(f"Darwin developer directory is unavailable: {developer_dir}")
    return developer_dir.resolve(strict=True)


# An Apple SDK's settings name its version and content; Clang reads the same
# file, and each SDK version is installed whole.
DARWIN_SDK_SETTINGS = "SDKSettings.json"


def select_darwin_sdk(
    env: Mapping[str, str], *, developer_dir: Path | None = None
) -> DarwinSDK:
    """Select only the SDK inputs a compiler needs; full tool capture is separate.

    Explicit paths need no selector process. Read version and digest from the
    same stable SDKSettings bytes, as Clang does, instead of querying xcrun for
    a second, independently selected version. Each operation can project the
    returned environment for reuse; no global memo hides external SDK changes.
    """
    env = ProcessEnvironment(env)
    selected_env = ProcessEnvironmentBuilder(env)
    if developer_dir is not None or env.get("DEVELOPER_DIR") or not env.get("SDKROOT"):
        developer_dir = _darwin_developer_directory(env, developer_dir=developer_dir)
        selected_env["DEVELOPER_DIR"] = str(developer_dir)
    explicit_sdk = env.get("SDKROOT")
    if explicit_sdk:
        sdk = Path(explicit_sdk)
    else:
        xcrun = resolve_executable(
            "xcrun", environment=selected_env, label="Darwin SDK selector"
        )
        assert developer_dir is not None
        selected = _darwin_selection(
            "xcrun-sdk-path",
            xcrun,
            ("--sdk", "macosx", "--show-sdk-path"),
            selected_env,
            label="Darwin SDK selector",
            inputs=_developer_inputs(
                developer_dir,
                "Platforms/MacOSX.platform/Developer/SDKs",
                "Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk",
                "SDKs",
                "SDKs/MacOSX.sdk",
            ),
            accept=lambda value: _one_line(value) and Path(value).is_dir(),
        )
        if not selected or "\n" in selected:
            raise ValueError("xcrun must select exactly one SDK directory")
        sdk = Path(selected)
    if not sdk.is_absolute() or not sdk.is_dir():
        raise ValueError(f"Darwin SDK directory is unavailable: {sdk}")
    sdk = sdk.resolve(strict=True)
    with open_stable_regular_file(
        sdk / DARWIN_SDK_SETTINGS, label="Darwin SDK settings"
    ) as opened:
        if opened.stat.st_size > 1024 * 1024:
            raise ValueError("Darwin SDK settings exceed the 1 MiB metadata budget")
        data = opened.stream.read(opened.stat.st_size + 1)
        if len(data) != opened.stat.st_size:
            raise ValueError("Darwin SDK settings changed while reading")
    try:
        settings = loads_exact(data.decode("utf-8", errors="strict"))
    except (ValueError, UnicodeDecodeError) as exc:
        raise ValueError("Darwin SDK settings must be valid JSON") from exc
    version = normalize_darwin_version(
        settings.get("Version") if isinstance(settings, dict) else None,
        label="Darwin SDK version",
    )
    return DarwinSDK(developer_dir, sdk, version, hashlib.sha256(data).hexdigest())


DarwinToolName = Literal["clang", "clang++", "ar", "ranlib", "ld"]
_DARWIN_TOOL_NAMES = get_args(DarwinToolName)


def select_darwin_toolchain(
    env: Mapping[str, str],
    *,
    developer_dir: Path | None = None,
    names: tuple[DarwinToolName, ...] = _DARWIN_TOOL_NAMES,
) -> DarwinToolchain:
    """Capture requested native tools under one fresh SDK selection.

    Full C/C++ clients retain the complete set. Consumers with narrower command
    requirements select their tools before any discovery or capture.
    """
    if (
        not names
        or len(set(names)) != len(names)
        or set(names) - set(_DARWIN_TOOL_NAMES)
    ):
        raise ValueError("Darwin tools must be a nonempty unique declared selection")
    developer_dir = _darwin_developer_directory(env, developer_dir=developer_dir)
    sdk = select_darwin_sdk(env, developer_dir=developer_dir)
    env = ProcessEnvironment(env)
    selected_env = env.updated(sdk.environment())
    xcrun = resolve_executable(
        "xcrun", environment=selected_env, label="Darwin SDK selector"
    )
    tools = {}
    search_inputs = (
        *_darwin_toolchain_inputs(developer_dir, selected_env),
        file_input(sdk.sdk_root / "usr" / "bin"),
    )
    for name in names:
        selected = _darwin_selection(
            "xcrun-find",
            xcrun,
            ("--sdk", str(sdk.sdk_root), "--find", name),
            selected_env,
            label="Darwin SDK selector",
            inputs=search_inputs,
            accept=lambda value: _one_line(value) and Path(value).is_file(),
        )
        if not selected or "\n" in selected:
            raise ValueError(f"xcrun must select one value for {name}")
        path = resolve_executable(
            selected, environment=selected_env, label=f"Darwin {name}"
        )
        if not path.resolve(strict=True).is_relative_to(developer_dir):
            raise ValueError(
                f"xcrun selected {name} outside the pinned developer directory: {path}"
            )
        tools[name] = path
    return DarwinToolchain(
        developer_dir=developer_dir,
        sdk_root=sdk.sdk_root,
        sdk_version=sdk.sdk_version,
        sdk_settings_sha256=sdk.sdk_settings_sha256,
        deployment_target=darwin_deployment_target(env, sdk_version=sdk.sdk_version),
        tools=tools,
    )
