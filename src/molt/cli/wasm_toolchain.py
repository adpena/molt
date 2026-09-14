from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import functools
import os
import re
import shutil
from pathlib import Path
import tomllib

from molt.cli import wasm_link_inputs
from molt.cli.command_runtime import _run_completed_command
from molt.toolchain_identity import (
    resolve_explicit_tool_command,
    stable_executable_probe,
)
from molt.cli.llvm_wasi_tools import (
    llvm_linker_candidates,
)
from molt.llvm_linker_roles import executable_selects_linker_role
from molt.wasi_sysroot import (
    wasi_sysroot_llvm_version,
)


_REQUIRED_WASM_RUST_TARGETS = ("wasm32-wasip1",)


class RustToolchainContractError(ValueError):
    pass


class WasmLinkerContractError(ValueError):
    pass


@dataclass(frozen=True)
class WasmLinkerIdentity:
    path: Path
    version: str
    wasi_sdk_llvm_version: str | None
    sha256: str | None = None

    @property
    def diagnostic(self) -> str:
        expected = self.wasi_sdk_llvm_version or "unattested"
        digest = self.sha256 or "unattested"
        return (
            f"role=wasm-ld path={self.path} version={self.version} "
            f"sha256={digest} wasi-sdk-llvm={expected}"
        )

    @property
    def fingerprint_token(self) -> str:
        return f"wasm-ld:{self.version}:{self.sha256 or 'unattested'}"


@dataclass(frozen=True)
class RustToolchainContract:
    channel: str | None
    components: tuple[str, ...]
    targets: tuple[str, ...]

    @property
    def rustup_toolchain_args(self) -> tuple[str, ...]:
        return () if self.channel is None else ("--toolchain", self.channel)

    @property
    def required_wasm_targets(self) -> tuple[str, ...]:
        targets: list[str] = []
        for target in (*_REQUIRED_WASM_RUST_TARGETS, *self.targets):
            if target.startswith("wasm32") and target not in targets:
                targets.append(target)
        return tuple(targets)


@functools.lru_cache(maxsize=32)
def rust_toolchain_contract(root: Path | str | None = None) -> RustToolchainContract:
    root_path = Path(root).resolve(strict=False) if root is not None else None
    toolchain_path = (
        root_path / "rust-toolchain.toml" if root_path is not None else None
    )
    if toolchain_path is None or not toolchain_path.exists():
        return RustToolchainContract(channel=None, components=(), targets=())
    try:
        data = tomllib.loads(toolchain_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise RustToolchainContractError(
            f"invalid Rust toolchain contract {toolchain_path}: {exc}"
        ) from exc
    toolchain = data.get("toolchain", {})
    if not isinstance(toolchain, dict):
        toolchain = {}
    channel_raw = toolchain.get("channel")
    channel = channel_raw.strip() if isinstance(channel_raw, str) else None
    if not channel:
        channel = None

    def string_tuple(key: str) -> tuple[str, ...]:
        value = toolchain.get(key, ())
        if not isinstance(value, list):
            return ()
        return tuple(
            item.strip() for item in value if isinstance(item, str) and item.strip()
        )

    return RustToolchainContract(
        channel=channel,
        components=string_tuple("components"),
        targets=string_tuple("targets"),
    )


def rustup_toolchain_install_cmd(root: Path) -> list[str]:
    contract = rust_toolchain_contract(root)
    cmd = ["rustup", "toolchain", "install"]
    if contract.channel is not None:
        cmd.append(contract.channel)
    else:
        cmd.append("stable")
    cmd.extend(["--profile", "minimal"])
    for component in contract.components:
        cmd.extend(["--component", component])
    for target in contract.required_wasm_targets:
        cmd.extend(["--target", target])
    return cmd


def rustup_target_add_cmd(target_triple: str, root: Path | None = None) -> list[str]:
    contract = rust_toolchain_contract(root)
    return [
        "rustup",
        "target",
        "add",
        target_triple,
        *contract.rustup_toolchain_args,
    ]


def rustup_installed_targets(root: Path | None = None) -> tuple[str, ...] | None:
    rustup = shutil.which("rustup")
    if rustup is None:
        return None
    contract = rust_toolchain_contract(root)
    try:
        result = _run_completed_command(
            [
                rustup,
                "target",
                "list",
                "--installed",
                *contract.rustup_toolchain_args,
            ],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return tuple(result.stdout.split())


def _rustlib_target_dir_installed(target_triple: str, root: Path | None) -> bool:
    """Lock-free ground truth for an installed rustup target.

    `rustup target list` contends on the rustup lock and has returned empty
    output under concurrent cargo/rustup lanes, producing false "target
    missing" build failures for targets that are installed. The installed
    standard library lives at
    ``$RUSTUP_HOME/toolchains/<channel>-*/lib/rustlib/<triple>`` — a plain
    directory probe that no lock can lie about.
    """
    rustup_home = os.environ.get("RUSTUP_HOME", "").strip()
    home = Path(rustup_home).expanduser() if rustup_home else Path.home() / ".rustup"
    toolchains = home / "toolchains"
    if not toolchains.is_dir():
        return False
    contract = rust_toolchain_contract(root)
    pattern = f"{contract.channel}-*" if contract.channel else "*"
    for toolchain_dir in toolchains.glob(pattern):
        if (toolchain_dir / "lib" / "rustlib" / target_triple).is_dir():
            return True
    return False


def ensure_rustup_target(
    target_triple: str, warnings: list[str], *, root: Path | None = None
) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    # Filesystem ground truth first: the rustup CLI query contends on the
    # rustup lock under concurrent lanes and has returned empty output for
    # installed targets, failing witness builds with a false "target
    # missing". The rustlib directory probe cannot be starved by a lock.
    if _rustlib_target_dir_installed(target_triple, root):
        return True
    try:
        installed = rustup_installed_targets(root)
    except RustToolchainContractError as exc:
        warnings.append(str(exc))
        return False
    if installed is None:
        warnings.append(f"Failed to query rustup targets for {target_triple}")
        return False
    if target_triple in installed:
        return True
    add_command = rustup_target_add_cmd(target_triple, root)
    add_command[0] = rustup_path
    try:
        add = _run_completed_command(
            add_command,
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        warnings.append(f"Failed to install rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    wasm_link_inputs.clear_rust_target_libdir_cache()
    return True


def rust_target_missing_message(
    target_triple: str, *, root: Path | None = None, context: str = "WASM build"
) -> str:
    try:
        cmd = rustup_target_add_cmd(target_triple, root)
    except RustToolchainContractError as exc:
        return f"{context} cannot resolve Rust target setup: {exc}"
    return (
        f"{context} requires Rust target {target_triple}, but the active Rust "
        f"toolchain does not provide it. Run: {' '.join(cmd)}"
    )


def _wasm_linker_version(path: Path, *, env: Mapping[str, str], cwd: Path) -> str:
    result = _run_completed_command(
        [str(path), "--version"],
        capture_output=True,
        env=dict(env),
        cwd=cwd,
        memory_guard_prefix="MOLT_BUILD",
    )
    output = f"{result.stdout}\n{result.stderr}"
    match = re.search(r"\b(?:LLD\s+)?(\d+\.\d+(?:\.\d+)?)\b", output)
    if result.returncode != 0 or match is None:
        detail = output.strip() or f"exit code {result.returncode}"
        raise WasmLinkerContractError(
            f"unable to attest wasm-ld identity for {path}: {detail}"
        )
    return match.group(1)


def _wasm_linker_binary_identity(
    path: Path,
    *,
    env: Mapping[str, str],
    cwd: Path,
) -> tuple[str, str]:
    with stable_executable_probe(path, label="runtime WASM linker") as (
        entrypoint,
        identity,
    ):
        version = _wasm_linker_version(entrypoint, env=env, cwd=cwd)
        return version, identity.sha256


def _llvm_release_line(version: str) -> tuple[int, int]:
    major, minor, *_ = version.split(".")
    return int(major), int(minor)


def resolve_wasm_linker(
    *, env: Mapping[str, str] | None = None, cwd: Path | None = None
) -> WasmLinkerIdentity | None:
    environment = os.environ if env is None else env
    sysroot = wasm_link_inputs.resolve_wasi_sysroot(env=environment)
    explicit_commands: tuple[tuple[str, ...], ...] = ()
    override = environment.get("MOLT_WASM_LD", "").strip()
    if override:
        try:
            explicit = resolve_explicit_tool_command(
                override, label="MOLT_WASM_LD", environment=environment
            )
        except ValueError as exc:
            raise WasmLinkerContractError(str(exc)) from exc
        if len(explicit) != 1:
            raise WasmLinkerContractError(
                "MOLT_WASM_LD must select one linker executable without embedded arguments"
            )
        if not executable_selects_linker_role(Path(explicit[0]), "wasm-ld"):
            raise WasmLinkerContractError(
                "MOLT_WASM_LD must select the wasm-ld entrypoint; generic lld and "
                f"other linker roles are not wasm linkers: {explicit[0]}"
            )
        explicit_commands = (explicit,)
    sibling_directories: tuple[Path, ...] = ()
    if sysroot is not None:
        sdk_root = wasm_link_inputs._wasi_sdk_root_for_sysroot(sysroot)
        if sdk_root is not None:
            sibling_directories = (sdk_root / "bin",)
    candidates = llvm_linker_candidates(
        "wasm-ld",
        explicit_commands=explicit_commands,
        sibling_directories=sibling_directories,
        environment=environment,
    )
    if not candidates:
        return None
    linker = candidates[0]
    version, sha256 = _wasm_linker_binary_identity(
        linker, env=environment, cwd=cwd or Path.cwd()
    )
    expected = None
    if sysroot is not None:
        expected = wasi_sysroot_llvm_version(sysroot)
        if expected is not None and _llvm_release_line(version) != _llvm_release_line(
            expected
        ):
            raise WasmLinkerContractError(
                "wasm linker/toolchain mismatch: "
                f"{linker} reports {version}, but {sysroot / 'VERSION'} requires "
                f"LLVM {expected}; use the matching wasi-sdk bin/wasm-ld or set "
                "MOLT_WASM_LD"
            )
    return WasmLinkerIdentity(linker, version, expected, sha256)
