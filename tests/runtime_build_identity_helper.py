"""Small deterministic fixtures using production runtime-family construction."""

from __future__ import annotations

import hashlib
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Mapping, Sequence

from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.cli.runtime_build_identity import _resolve_runtime_build_family_identities
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    RuntimeBuildMemberPlan,
    RuntimeToolchainContentManifest,
    runtime_build_fingerprint,
)
from molt.exact_json import canonical_json_sha256
from molt.rust_toolchain import cargo_configuration_paths
from tests.python_environment_test_support import runtime_identity_manifest
from molt.cli.runtime_cargo_plan import (
    CargoExecutableCustody,
    CargoResourceCustody,
    CargoResourceRoot,
    RuntimeCargoPlan,
    _CargoEnvironment,
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
from molt.wasi_sdk_identity import WasiCAbiProjection


@dataclass(frozen=True)
class RuntimeFixtureRoot:
    """Writable runtime fixtures owned by pytest's temporary-directory lifetime.

    Obtain this value through the shared ``runtime_fixture_root`` fixture, not
    from a compiler source path. Source roots remain separate read-only inputs.
    """

    path: Path

    def __post_init__(self) -> None:
        path = self.path.resolve(strict=True)
        source = _compiler_root().resolve(strict=True)
        if (
            not path.is_dir()
            or source.is_relative_to(path)
            or path.is_relative_to(source)
        ):
            raise ValueError("runtime fixture root cannot own the compiler source root")
        object.__setattr__(self, "path", path)

    def native_executable(self, relative: str) -> Path:
        """Own real native image bytes for custody-only, never-executed tools.

        The running interpreter supplies a valid executable for this host. It
        does not become a Cargo, linker, compiler or wrapper implementation;
        consumers testing tool behavior must provide real tools instead.
        Existing files are retained so deliberate custody mutations stay visible.
        """
        relative_path = Path(relative)
        if relative_path.anchor or ".." in relative_path.parts:
            raise ValueError(
                "native executable fixture requires a confined relative path"
            )
        path = (self.path / relative_path).resolve()
        if path == self.path or not path.is_relative_to(self.path):
            raise ValueError("native executable fixture escaped its pytest-owned root")
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(Path(sys.executable).resolve(strict=True), path)
        return path


def mock_wasm_optimizer_cache_fact(root: RuntimeFixtureRoot) -> dict[str, str]:
    """Synthetic child protocol identity, never optimizer execution evidence."""
    image = root.native_executable("mock-tools/wasm-opt")
    return {
        "tool": "wasm-opt",
        "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
        "binaryen_version": "wasm-opt version 130 (version_130)",
    }


def mock_wasm_optimizer_publications(
    command: Sequence[str],
    artifacts: Mapping[str, tuple[Path, bytes]],
    cache_fact: Mapping[str, str],
) -> dict[str, tuple[Path, bytes]]:
    """Complete mocked-child sidecars with independently bound output bytes.

    No optimizer runs. These are synthetic protocol fixtures for outer routing,
    custody and publication tests; actual optimizer execution has its own tests.
    Preserve production validation and default optimization, rather than hiding
    a missing child output by disabling optimization in routing tests.
    """
    from molt.wasm_optimization import wasm_link_policy
    from molt.wasm_optimizer_identity import (
        build_wasm_optimizer_attestation,
        encode_wasm_optimizer_attestation,
        wasm_optimizer_attestation_path,
    )

    if "--optimize" not in command:
        return {}
    level = command[command.index("--optimize-level") + 1]
    preserve_debug = "--preserve-debug-sections" in command
    policy = wasm_link_policy(level, preserve_debug=preserve_debug)
    outputs = {}
    for artifact_role, sidecar_role in (
        ("linked", "optimizer"),
        ("app", "app_optimizer"),
    ):
        if artifact_role not in artifacts:
            continue
        path, content = artifacts[artifact_role]
        digest = hashlib.sha256(content).hexdigest()
        payload = build_wasm_optimizer_attestation(
            {
                "ok": True,
                "wasm_opt_sha256": cache_fact["sha256"],
                "binaryen_version": cache_fact["binaryen_version"],
                "optimization_level": policy.level,
                "optimization_converge": policy.converge,
                "optimization_apply_level": policy.apply_level,
                "optimization_preserve_debug": preserve_debug,
                "optimization_extra_passes": list(policy.extra_passes),
                "pipeline": list(policy.pipeline),
                "optimizer_input_sha256": digest,
                "optimizer_output_sha256": digest,
            },
            published_output=content,
        )
        assert payload["published_output_sha256"] == digest
        outputs[sidecar_role] = (
            wasm_optimizer_attestation_path(path),
            encode_wasm_optimizer_attestation(payload).encode("utf-8"),
        )
    return outputs


def _fixture_path(root: RuntimeFixtureRoot) -> Path:
    if not isinstance(root, RuntimeFixtureRoot):
        raise TypeError(
            "runtime fixtures require the pytest-owned runtime_fixture_root"
        )
    return root.path


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
            "sysroots": {
                "wasi": {
                    "schema": "molt.wasi-c-abi-identity.v1",
                    "target": "wasm32-wasip1",
                    "variant": "single",
                    "sdk_version": "34.0",
                    "llvm_version": "23.1.0",
                    "tree_sha256": canonical_json_sha256(f"wasi:{seed}"),
                    "members": {
                        role: {
                            "size": len(seed),
                            "sha256": canonical_json_sha256(f"{role}:{seed}"),
                        }
                        for role in (
                            "libc",
                            "long_double",
                            "compiler_rt",
                            "crt_command",
                            "crt_reactor",
                        )
                    },
                }
            }
            if wasm
            else {},
            "archives": [
                {
                    "logical_name": name,
                    "size": len(seed),
                    "sha256": canonical_json_sha256(f"{name}:{seed}"),
                }
                for name in (
                    (
                        "wasi-libc",
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
        "schema": "molt.runtime-build-script-environment.v3",
        "build_python": {
            "selected_by": "platform-default",
            "selectors": {"MOLT_BUILD_PYTHON": "unset", "PYTHON": "unset"},
            "content_digest": canonical_json_sha256(build_python_identity_fixture()),
        },
        "python_import_policy": "isolated-no-site-v1",
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": [] if wasm else "ignored-for-target",
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": [] if wasm else "ignored-for-target",
        "MOLT_WASI_C_ABI_PLAN": (
            {
                "state": "selected",
                "content": {
                    "schema": "molt.wasi-c-abi-identity.v1",
                    "target": "wasm32-wasip1",
                    "variant": "single",
                    "sdk_version": "34.0",
                    "llvm_version": "23.0.0",
                    "tree_sha256": "a" * 64,
                    "members": {
                        name: {
                            "size": 8,
                            "sha256": hashlib.sha256(
                                (name + compile_seed).encode()
                            ).hexdigest(),
                        }
                        for name in (
                            "libc",
                            "long_double",
                            "compiler_rt",
                            "crt_command",
                            "crt_reactor",
                        )
                    },
                },
            }
            if target in {"wasm32-wasip1", "wasm32-unknown-unknown"}
            else {"state": "ignored-for-target"}
        ),
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


def provisioned_wasi_sdk_fixture(fixture_root: RuntimeFixtureRoot):
    """Create a complete synthetic install using real, never-executed tool images.

    Its manifest/receipt are real production admission inputs; this fixture does
    not claim the copied host images implement Clang or the archives implement C.
    """
    from dataclasses import asdict
    from molt import llvm_toolchain
    from molt.wasi_sdk_identity import (
        executable_filename,
        render_wasi_sdk_install_receipt,
        wasi_sdk_tree_identity,
    )

    root = _fixture_path(fixture_root)
    asset = llvm_toolchain.wasi_sdk_host_asset(_compiler_root())
    prefix = llvm_toolchain.wasi_sdk_install_prefix(root / "toolchains", asset)
    sdk = prefix / "sdk"
    receipt = prefix / ".molt-wasi-sdk.json"
    if not receipt.exists():
        for name in (
            "clang",
            "clang++",
            "llvm-ar",
            "llvm-ranlib",
            "llvm-nm",
            "wasm-ld",
            "llvm-strip",
        ):
            relative = str(
                (sdk / "bin" / executable_filename(name, asset.id)).relative_to(root)
            )
            fixture_root.native_executable(relative)
        (sdk / "VERSION").write_text(
            f"{asset.sdk_version}\nwasi-libc: fixture\nllvm-version: {asset.llvm_version}\n",
            encoding="utf-8",
        )
        for relative, content in (
            (
                "share/wasi-sysroot/include/wasm32-wasip1/errno.h",
                b"#define EINVAL 28\n",
            ),
            (
                "share/wasi-sysroot/include/wasm32-wasip1/stddef.h",
                b"typedef int size_t;\n",
            ),
            *(
                ("share/wasi-sysroot/lib/wasm32-wasip1/" + name, b"!<arch>\n")
                for name in (
                    "libc.a",
                    "libc-printscan-long-double.a",
                    "crt1-command.o",
                    "crt1-reactor.o",
                )
            ),
            (
                f"lib/clang/{asset.llvm_version.split('.')[0]}/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a",
                b"!<arch>\n",
            ),
        ):
            path = sdk / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        receipt.write_text(
            render_wasi_sdk_install_receipt(asdict(asset), wasi_sdk_tree_identity(sdk)),
            encoding="utf-8",
            newline="",
        )
    return llvm_toolchain.load_wasi_sdk_installation(
        _compiler_root(), prefix, verify_tree=False
    )


def runtime_wasi_c_abi_plan(fixture_root: RuntimeFixtureRoot) -> WasiCAbiProjection:
    """Admit the finite projection from the fixture's real provision receipt."""
    from molt.llvm_toolchain import wasi_c_abi_plan

    return wasi_c_abi_plan(provisioned_wasi_sdk_fixture(fixture_root))


def bind_runtime_wasm_specs(
    shared: _RuntimeWasmBuildSpec,
    reloc: _RuntimeWasmBuildSpec,
    *,
    root: Path,
    family_seed: str = "family",
    compile_seed: str | None = None,
) -> tuple[_RuntimeWasmBuildSpec, _RuntimeWasmBuildSpec]:
    shared, reloc = _resolve_runtime_wasm_cargo_specs(
        root,
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
    fixture_root: RuntimeFixtureRoot,
    env: Mapping[str, str],
    cargo_command: Sequence[str],
    requested_target: str | None = None,
    rustflags_transform=None,
    **_kwargs: object,
) -> RuntimeCargoPlan:
    """Live plan with immutable source input and pytest-owned generated tools."""
    root = project_root.resolve()
    fixtures = _fixture_path(fixture_root)
    environment = _CargoEnvironment(env)
    environment["CARGO_HOME"] = str(fixtures / "test-cargo-home")
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
    environment["RUSTC"] = str(executable)
    wrappers = {}
    for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        selected = environment.get(name, "")
        if selected:
            path = fixture_root.native_executable(f"test-tools/{Path(selected).name}")
            wrappers[name] = path
            environment[name] = str(path)
        else:
            environment[name] = ""
    wasi = None
    installation = None
    tool_paths = {"cargo": executable, "rustc": executable}
    if requested_target in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        from molt.llvm_toolchain import (
            selected_wasi_sdk_installation,
            wasi_c_abi_plan,
            project_wasm_toolchain_environment,
        )

        installation = (
            selected_wasi_sdk_installation(_compiler_root(), environ=environment)
            if "WASI_SDK_PATH" in environment or "WASI_SDK_PREFIX" in environment
            else provisioned_wasi_sdk_fixture(fixture_root)
        )
        assert installation is not None
        wasi = wasi_c_abi_plan(installation)
        environment.update(
            project_wasm_toolchain_environment(
                installation, environ={}, rust_target=requested_target
            )
        )
        for role, name in (
            ("cc", "clang"),
            ("cxx", "clang++"),
            ("ar", "llvm-ar"),
            ("ranlib", "llvm-ranlib"),
        ):
            path = installation.sdk / installation.tool_fact(name)["path"]
            tool_paths[role] = path
            environment[f"{role.upper()}_{requested_target}"] = str(path)
        tool_paths["linker"] = wasi.linker
        environment[
            "CARGO_TARGET_" + requested_target.upper().replace("-", "_") + "_LINKER"
        ] = str(wasi.linker)
    flag_plan = _resolve_rust_flag_resources(
        flags,
        cargo_command,
        root=root,
        env=environment,
        wasi_plan=wasi,
    )
    flags = flag_plan.flags
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
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
            if installation is None
            or role not in {"cc", "cxx", "ar", "ranlib", "linker", "final_linker"}
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
    target_libdir = fixtures / "test-rustlib" / (requested_target or host) / "lib"
    target_libdir.mkdir(parents=True, exist_ok=True)
    builtins = target_libdir / "libcompiler_builtins-fixture.rlib"
    if not builtins.exists():
        builtins.write_bytes(b"!<arch>\n")
    libc = target_libdir / "self-contained" / "libc.a"
    libc.parent.mkdir(exist_ok=True)
    if not libc.exists():
        libc.write_bytes(b"!<arch>\n")
    rust_roots = (CargoResourceRoot("rust/target-libdir/0", target_libdir),)
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
        cargo_configuration_paths(root, environment),
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
        installation,
    )
