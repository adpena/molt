from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import functools
import os
import re
from pathlib import Path
import subprocess
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


def rust_target_readiness_error(target_triple: str, *, root: Path) -> str | None:
    """Inspect the selected compiler's standard library without installing tools.

    Reuse link-input selection, including explicit RUSTC and Rustup proxy
    resolution in the compiler source directory. A different toolchain's target
    directory is not evidence that the selected compiler can build this target.
    Only the printed path is cached; library availability is checked each time.
    """
    try:
        libdir = wasm_link_inputs.rust_target_libdir(target_triple, root=root)
        if libdir is not None and any(
            path.is_file() and path.stat().st_size > 0
            for pattern in ("libstd-*.rlib", "libstd.rlib")
            for path in libdir.glob(pattern)
        ):
            return None
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        return (
            f"Cannot inspect Rust target {target_triple}: {exc}. "
            "No toolchains were installed or changed."
        )
    return (
        rust_target_missing_message(target_triple, root=root, context="Compilation")
        + "\nNo toolchains were installed or changed."
    )


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
