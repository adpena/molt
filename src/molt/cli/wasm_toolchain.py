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
from molt.toolchain_identity import stable_executable_probe


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


def resolve_wasm_linker(
    *, env: Mapping[str, str] | None = None, cwd: Path | None = None
) -> WasmLinkerIdentity:
    """Attest the one wasm-ld the WebAssembly toolchain authority selects.

    ``molt.llvm_toolchain.resolve_wasi_sdk_tool`` owns the selection: an
    explicit ``MOLT_WASM_LD``, the SDK named by ``WASI_SDK_PATH``, or the
    provisioned wasi-sdk. There is no ambient PATH search and Molt never
    installs a linker; an unavailable one fails with the exact provisioning
    command or selector to set.
    """
    from molt.llvm_toolchain import (
        LlvmToolchainConfigError,
        resolve_wasi_sdk_tool,
        selected_wasi_sdk_installation,
    )
    from molt.source_root import compiler_source_root

    environment = dict(os.environ if env is None else env)
    try:
        linker = resolve_wasi_sdk_tool(
            compiler_source_root(), "wasm-ld", environ=environment, cwd=cwd
        )
    except LlvmToolchainConfigError as exc:
        raise WasmLinkerContractError(str(exc)) from exc
    try:
        installation = selected_wasi_sdk_installation(
            compiler_source_root(), environ=environment, cwd=cwd
        )
        if installation is None:
            raise ValueError("the selected WASI SDK is not provisioned")
    except (OSError, ValueError) as exc:
        raise WasmLinkerContractError(f"WASI SDK refused: {exc}") from exc
    fact = installation.tool_fact("wasm-ld")
    version, sha256 = (
        (installation.asset.llvm_version, str(fact["sha256"]))
        if linker.parent.resolve(strict=True) / linker.name
        == installation.sdk / fact["path"]
        else _wasm_linker_binary_identity(
            linker, env=environment, cwd=cwd or Path.cwd()
        )
    )
    expected = installation.asset.llvm_version
    if version != expected:
        raise WasmLinkerContractError(
            f"WASM linker reports {version}, but selected SDK requires LLVM {expected}"
        )
    return WasmLinkerIdentity(linker, version, expected, sha256)
