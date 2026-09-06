"""Small deterministic fixtures using production runtime-family construction."""

from __future__ import annotations

import sys
from pathlib import Path
from types import MappingProxyType
from typing import Mapping, Sequence

from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.cli.runtime_build_identity import (
    RuntimeBuildIdentity,
    RuntimeBuildMemberPlan,
    RuntimeToolchainContentManifest,
    _resolve_runtime_build_family_identities,
    runtime_build_fingerprint,
)
from molt.exact_json import canonical_json_sha256
from tests.python_environment_test_support import runtime_identity_manifest
from molt.cli.runtime_cargo_plan import (
    CargoExecutableCustody,
    CargoResourceCustody,
    CargoResourceRoot,
    RuntimeCargoPlan,
    _CargoEnvironment,
    _config_paths,
    _pin_cargo_command,
    _resolve_rust_flag_resources,
    _resolve_c_build_resources,
    _select_host_c_tools,
)
from molt.cli.runtime_wasm_build_spec import (
    _RuntimeWasmBuildSpec,
    _resolve_runtime_wasm_cargo_specs,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.runtime_wasm_build_support import RuntimeWasmLinkInputs
from molt.toolchain_identity import stable_regular_file_identity


def _tree_identity(seed: str) -> dict[str, object]:
    return {
        "digest": canonical_json_sha256(seed),
        "file_count": 1,
        "total_size": len(seed),
        "roots": ["test"],
        "missing": [],
    }


def _tool_identity(name: str, seed: str) -> dict[str, object]:
    return {
        "logical_name": name,
        "entrypoint": name,
        "content_filename": name,
        "size": len(seed),
        "sha256": canonical_json_sha256(f"{name}:{seed}"),
    }


def build_python_identity_fixture() -> dict[str, object]:
    material = {
        "schema": "molt.runtime-build-python.v1",
        "logical_name": "build_python",
        "runtime": runtime_identity_manifest(),
        "selected_executable": {
            "entrypoint": "python",
            "content_filename": "python",
            "size": 6,
            "sha256": canonical_json_sha256("python"),
        },
    }
    return {**material, "identity_sha256": canonical_json_sha256(material)}


def runtime_toolchain_content_manifest(
    seed: str = "exact",
    *,
    target_triple: str = "native",
    host_target: str = "x86_64-unknown-linux-gnu",
) -> RuntimeToolchainContentManifest:
    wasm = target_triple.startswith("wasm32-")
    host = host_target
    target = host if target_triple == "native" else target_triple
    tools = {
        name: _tool_identity(name, seed)
        for name in (
            "cargo",
            "rustc",
            "cc",
            "cxx",
            "ar",
            "ranlib",
            *(("wasm_linker",) if wasm else ("linker",)),
        )
    }
    tools["build_python"] = build_python_identity_fixture()
    payload = {
        "target_triple": target_triple,
        "toolchain": {
            "tools": tools,
            "wrappers": {},
            "cargo_configuration": _tree_identity(f"cargo:{seed}"),
            "effective_target": target,
            "rust_resources": {
                "host_triple": host,
                "selected_target": target,
                "content": _tree_identity(f"rust:{seed}"),
            },
            "sysroots": {"wasi": _tree_identity(f"wasi:{seed}")} if wasm else {},
            "archives": [
                {
                    "logical_name": name,
                    "size": len(seed),
                    "sha256": canonical_json_sha256(f"{name}:{seed}"),
                }
                for name in (
                    (
                        "wasi-libc",
                        "rust-compiler-builtins",
                        "wasi-long-double",
                        "clang-rt-builtins",
                    )
                    if wasm
                    else ()
                )
            ],
        },
    }
    return RuntimeToolchainContentManifest(canonical_json_sha256(payload), payload)


def _identity(
    *,
    target: str,
    cargo_profile: str,
    family_seed: str,
    compile_seed: str,
    members: tuple[RuntimeBuildMemberPlan, ...],
    artifact_selection: RuntimeArtifactSelection,
    host_target: str = "x86_64-unknown-linux-gnu",
) -> tuple[RuntimeBuildIdentity, ...]:
    wasm = target.startswith("wasm32-")
    build_script = {
        "schema": "molt.runtime-build-script-environment.v1",
        "build_python": {
            "selected_by": "platform-default",
            "selectors": {"MOLT_BUILD_PYTHON": "unset", "PYTHON": "unset"},
            "content_digest": canonical_json_sha256(build_python_identity_fixture()),
        },
        "PYTHONPATH": {"state": "unset"},
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": [] if wasm else "ignored-for-target",
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": [] if wasm else "ignored-for-target",
        "MOLT_WASM_LONGDOUBLE_ARCHIVE": {
            "state": "unset" if wasm else "ignored-for-target"
        },
        "MOLT_WASM_BUILTINS_ARCHIVE": {
            "state": "unset" if wasm else "ignored-for-target"
        },
    }
    return _resolve_runtime_build_family_identities(
        sources=_tree_identity(compile_seed),
        toolchain_manifest=runtime_toolchain_content_manifest(
            compile_seed, target_triple=target, host_target=host_target
        ),
        target_triple=target,
        common_config={
            "cargo_profile": cargo_profile,
            "target_triple": target,
            "runtime_features": [],
            "producer_artifact_selection": artifact_selection.source_identity,
            "cargo_command": ["cargo", "rustc"],
            "environment": {},
            "build_script_environment": build_script,
        },
        publication_authority={
            "schema": "molt.runtime-build-tooling-authority.v2",
            **_tree_identity(family_seed),
        },
        members=members,
    )


def runtime_build_identity(
    kind: str,
    family_seed: str = "family",
    *,
    compile_seed: str | None = None,
    member_seeds: dict[str, str] | None = None,
) -> RuntimeBuildIdentity:
    if kind not in {"shared", "reloc"}:
        raise ValueError("WASM fixture member must be shared or reloc")
    members = tuple(
        RuntimeBuildMemberPlan(
            name, (), ((member_seeds or {}).get(name, name),), name, False
        )
        for name in ("shared", "reloc")
    )
    identities = _identity(
        target="wasm32-wasip1",
        cargo_profile="release",
        family_seed=family_seed,
        compile_seed=compile_seed or family_seed,
        members=members,
        artifact_selection=RUNTIME_WASM_COMBINED_ARTIFACTS,
    )
    return identities[0 if kind == "shared" else 1]


def native_runtime_staticlib_identity(
    *,
    cargo_profile: str = "release",
    target_triple: str | None = None,
    family_seed: str = "native-staticlib-family",
    artifact_selection: RuntimeArtifactSelection = RUNTIME_STATICLIB_ARTIFACTS,
    host_target: str = "x86_64-unknown-linux-gnu",
) -> RuntimeBuildIdentity:
    return _identity(
        target=target_triple or "native",
        host_target=host_target,
        cargo_profile=cargo_profile,
        family_seed=family_seed,
        compile_seed=family_seed,
        artifact_selection=artifact_selection,
        members=(
            RuntimeBuildMemberPlan(
                "staticlib",
                (),
                ("--print", "native-static-libs"),
                "native-staticlib-and-link-manifest-v1",
                False,
            ),
        ),
    )[0]


def fingerprint_for_identity(
    identity: RuntimeBuildIdentity, *, scope: str = "member"
) -> dict[str, object]:
    return runtime_build_fingerprint(identity, scope=scope)


def runtime_wasm_link_inputs(
    root: Path, *, env: Mapping[str, str] | None = None
) -> RuntimeWasmLinkInputs:
    """Tiny file-backed input custody; never inspects a live WASI toolchain."""
    directory = root / "runtime-link-inputs"
    directory.mkdir(parents=True, exist_ok=True)
    identities = []
    for name in (
        "wasm-ld",
        "libc.a",
        "rust-builtins.a",
        "long-double.a",
        "clang-builtins.a",
    ):
        path = directory / name
        if not path.exists():
            path.write_bytes(b"!<arch>\n" if name.endswith(".a") else b"test-linker")
        identities.append(
            stable_regular_file_identity(path, label=f"test runtime link {name}")
        )
    return RuntimeWasmLinkInputs(
        directory,
        CargoExecutableCustody.capture("runtime WASM linker", identities[0].path),
        *identities[1:],
    )


def bind_runtime_wasm_specs(
    shared: _RuntimeWasmBuildSpec,
    reloc: _RuntimeWasmBuildSpec,
    *,
    root: Path | None = None,
    family_seed: str = "family",
    compile_seed: str | None = None,
) -> tuple[_RuntimeWasmBuildSpec, _RuntimeWasmBuildSpec]:
    shared, reloc = _resolve_runtime_wasm_cargo_specs(
        root or _compiler_root(),
        shared,
        reloc,
        simd_enabled=True,
        freestanding=False,
    )
    member_seeds = {"shared": shared.link_flags, "reloc": reloc.link_flags}
    shared_identity = runtime_build_identity(
        "shared", family_seed, compile_seed=compile_seed, member_seeds=member_seeds
    )
    reloc_identity = runtime_build_identity(
        "reloc", family_seed, compile_seed=compile_seed, member_seeds=member_seeds
    )
    compile_fingerprint = runtime_build_fingerprint(shared_identity, scope="compile")
    return (
        shared._replace(
            fingerprint=runtime_build_fingerprint(
                shared_identity, scope="member-output"
            ),
            staticlib_fingerprint=compile_fingerprint,
        ),
        reloc._replace(
            fingerprint=runtime_build_fingerprint(
                reloc_identity, scope="member-output"
            ),
            staticlib_fingerprint=compile_fingerprint,
        ),
    )


def runtime_cargo_plan(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_command: Sequence[str],
    requested_target: str | None = None,
    rustflags_transform=None,
    capture_inputs=None,
    **_kwargs: object,
) -> RuntimeCargoPlan:
    """Live-plan fixture with no executable probes; consumers remain real."""
    root = project_root.resolve()
    environment = _CargoEnvironment(env)
    environment["CARGO_HOME"] = str(root / "test-cargo-home")
    host = "x86_64-unknown-linux-gnu"
    executable = Path(sys.executable)
    profiles = {
        "profile": {
            name: {"inherits": parent}
            for name, parent in (
                ("release-output", "release"),
                ("release-fast", "release"),
                ("wasm-release", "release"),
                ("dev-fast", "dev"),
            )
        }
    }
    flags = (
        tuple(environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"))
        if environment.get("CARGO_ENCODED_RUSTFLAGS")
        else tuple(environment.get("RUSTFLAGS", "").split())
    )
    if rustflags_transform is not None:
        flags = rustflags_transform(flags)
    flag_plan = _resolve_rust_flag_resources(
        flags, cargo_command, root=root, env=environment
    )
    flags = flag_plan.flags
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    environment["RUSTC"] = str(executable)
    wrappers = {}
    for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        selected = environment.get(name, "")
        if selected:
            path = root / "test-tools" / Path(selected).name
            path.parent.mkdir(exist_ok=True)
            if not path.exists():
                path.write_bytes(b"MZfixture-" + path.name.encode())
            wrappers[name] = path
            environment[name] = str(path)
        else:
            environment[name] = ""
    tool_paths = {"cargo": executable, "rustc": executable}
    if flag_plan.dependency_linker is not None:
        tool_paths["linker"] = flag_plan.dependency_linker
    if flag_plan.final_linker is not None:
        tool_paths["final_linker"] = flag_plan.final_linker
    _select_host_c_tools(
        tool_paths,
        environment,
        root=root,
        target=requested_target or host,
        host_target=host,
    )
    custody = (
        *(
            CargoExecutableCustody.capture("tool/" + role, path)
            for role, path in tool_paths.items()
        ),
        *(
            CargoExecutableCustody.capture("wrapper/" + role, path)
            for role, path in wrappers.items()
        ),
    )
    tools = MappingProxyType(tool_paths)
    command = _pin_cargo_command(
        flag_plan.command, tools, wrappers, requested_target or host
    )
    target_libdir = root / "test-rustlib" / (requested_target or host) / "lib"
    target_libdir.mkdir(parents=True, exist_ok=True)
    builtins = target_libdir / "libcompiler_builtins.fixture.rlib"
    if not builtins.exists():
        builtins.write_bytes(b"!<arch>\n")
    libc = target_libdir / "self-contained" / "libc.a"
    libc.parent.mkdir(exist_ok=True)
    if not libc.exists():
        libc.write_bytes(b"!<arch>\n")
    rust_roots = (CargoResourceRoot("rust/target-libdir/0", target_libdir),)
    if capture_inputs is not None:
        capture_inputs(MappingProxyType(environment), tools, rust_roots)
    c_resources = _resolve_c_build_resources(
        environment, target=requested_target or host, host_target=host
    )
    return RuntimeCargoPlan(
        root,
        command,
        MappingProxyType(environment),
        requested_target or host,
        host,
        executable,
        tools,
        MappingProxyType(wrappers),
        flags,
        (),
        _config_paths(root, environment),
        MappingProxyType({}),
        MappingProxyType(profiles),
        custody,
        CargoResourceCustody.capture(
            (*rust_roots, *flag_plan.roots, *c_resources.roots)
        ),
        (*flag_plan.logical_paths, *c_resources.logical_paths),
        CargoResourceCustody.capture(flag_plan.link_roots),
        (),
        c_resources.environment,
    )
