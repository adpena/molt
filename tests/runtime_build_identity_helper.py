"""Small deterministic fixtures using production runtime-family construction."""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs

import hashlib
import json
import os
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
    RuntimePublicationAction,
    RuntimePublicationTransform,
    RuntimeToolchainContentManifest,
    runtime_build_fingerprint,
    runtime_publication_executor_paths,
)
from molt.exact_json import canonical_json_sha256
from molt.rust_toolchain import cargo_configuration_paths
from molt.cli.cargo_resource_custody import (
    CargoExecutableCustody,
    CargoResourceCustody,
    CargoResourceRoot,
)
from molt.cli.cargo_search_path import cargo_search_path, derives_cargo_search_path
from molt.cli.runtime_cargo_plan import (
    RuntimeCargoPlan,
    _cargo_build_directory_base,
    _resolve_rust_flag_resources,
    _resolve_c_build_resources,
    _select_host_c_tools,
)
from molt.cli.runtime_wasm_build_support import runtime_reloc_publication_action
from molt.cli.runtime_wasm_build_spec import (
    _RuntimeWasmBuildSpec,
    _resolve_runtime_wasm_cargo_specs,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.wasi_sdk_identity import WasiCAbiProjection
from tests.executable_test_support import native_executable_donor


RUNTIME_FIXTURE_INPUT = "MOLT_TEST_RUNTIME_INPUT"


def runtime_cargo_environment_inputs(environment=None) -> CargoEnvironmentInputs:
    """One modeled Cargo unit's explicit ambient dependency observation."""
    return CargoEnvironmentInputs.capture(
        ProcessEnvironment(os.environ if environment is None else environment),
        (RUNTIME_FIXTURE_INPUT,),
        local_unit_count=1,
    )


def native_runtime_build_inputs(
    build_identity: RuntimeBuildIdentity, *, environment=None
):
    """The source fixture's plan and the frozen environment it models."""
    from molt.cli.runtime_native_build import NativeRuntimeBuildInputs

    return NativeRuntimeBuildInputs(
        build_identity,
        ProcessEnvironment(os.environ if environment is None else environment),
    )


def _copy_native_image(source: Path, destination: Path) -> None:
    """Copy independent fixture bytes using the host's native copy policy."""
    if sys.platform != "darwin":
        shutil.copy2(source, destination)
        return

    import ctypes

    # copyfile(3), unlike fcopyfile(3), supports COPYFILE_CLONE. Apple's
    # single operation selects CoW or a full copy, including across volumes;
    # its returned errors are terminal, never retried by this fixture.
    library = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    if not hasattr(library, "clonefile"):
        # COPYFILE_CLONE and clonefile both arrived in macOS 10.12.
        shutil.copy2(source, destination)
        return
    copyfile = library.copyfile
    copyfile.argtypes = [
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_void_p,
        ctypes.c_uint32,
    ]
    copyfile.restype = ctypes.c_int
    # <copyfile.h>: COPYFILE_CLONE implies EXCL, STAT, XATTR and DATA.
    if copyfile(os.fsencode(source), os.fsencode(destination), None, 1 << 24) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), str(source), None, str(destination))
    shutil.copystat(source, destination)


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

        A small real executable of this host supplies the bytes. It does not
        become a Cargo, linker, compiler or wrapper implementation;
        consumers testing tool behavior must provide real tools instead.
        Existing files are retained so deliberate custody mutations stay visible.
        """
        return self.native_executables((relative,))[0]

    def native_executables(self, relatives: Sequence[str]) -> tuple[Path, ...]:
        """Provision a batch before exposing its independent images to tests.

        Only the first image created by this call can supply later copies.
        Existing images may have deliberate mutations; never use them as a
        donor. No template or donor survives in state between calls.
        """
        paths = []
        for relative in relatives:
            relative_path = Path(relative)
            if relative_path.anchor or ".." in relative_path.parts:
                raise ValueError(
                    "native executable fixture requires a confined relative path"
                )
            path = (self.path / relative_path).resolve()
            if path == self.path or not path.is_relative_to(self.path):
                raise ValueError(
                    "native executable fixture escaped its pytest-owned root"
                )
            paths.append(path)
        donor = None
        for path in paths:
            if not path.exists():
                path.parent.mkdir(parents=True, exist_ok=True)
                source = native_executable_donor() if donor is None else donor
                _copy_native_image(source, path)
                if donor is None:
                    donor = path
        return tuple(paths)


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
        "schema": "molt.runtime-build-script-environment.v4",
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
            # A recorded request always carries one default-feature marker;
            # the WASM fixture pair is a no-default (micro-like) runtime.
            "runtime_features": ["no-default-features"] if wasm else [],
            "producer_artifact_selection": artifact_selection.source_identity,
            "cargo_command": ["cargo", "rustc"],
            "environment": {},
            "build_script_environment": build_script,
        },
        members=members,
    )


def publication_executor(
    transform: RuntimePublicationTransform,
) -> dict[str, object] | None:
    paths = runtime_publication_executor_paths(transform)
    if not paths:
        return None
    return {
        "digest": canonical_json_sha256(paths),
        "file_count": len(paths),
        "total_size": len(paths),
        "roots": list(paths),
        "missing": [],
    }


def runtime_build_identity(
    kind: str,
    family_seed: str = "family",
    *,
    compile_seed: str | None = None,
    member_link_args: dict[str, tuple[str, ...]] | None = None,
    member_publications: dict[str, RuntimePublicationAction] | None = None,
) -> RuntimeBuildIdentity:
    if kind not in {"shared", "reloc"}:
        raise ValueError("WASM fixture member must be shared or reloc")
    members = tuple(
        RuntimeBuildMemberPlan(
            name,
            (),
            (member_link_args or {}).get(name, (name,)),
            (
                member_publications[name]
                if member_publications is not None
                else RuntimePublicationAction(
                    RuntimePublicationTransform.SHARED,
                    False,
                    publication_executor(RuntimePublicationTransform.SHARED),
                    export_renames=(("fixture", family_seed),),
                )
                if name == "shared"
                else runtime_reloc_publication_action(
                    ("-C", f"link-arg=--export={family_seed}"),
                    module_name="molt_runtime_reloc.wasm",
                    executor=publication_executor(RuntimePublicationTransform.RELOC),
                )
            ),
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
                RuntimePublicationAction(
                    RuntimePublicationTransform.WASM_STATICLIB
                    if target_triple and target_triple.startswith("wasm32-")
                    else RuntimePublicationTransform.NATIVE_STATICLIB,
                    False,
                    publication_executor(
                        RuntimePublicationTransform.WASM_STATICLIB
                        if target_triple and target_triple.startswith("wasm32-")
                        else RuntimePublicationTransform.NATIVE_STATICLIB
                    ),
                ),
            ),
        ),
    )[0]


def fingerprint_for_identity(
    identity: RuntimeBuildIdentity, *, scope: str = "member"
) -> dict[str, object]:
    return runtime_build_fingerprint(identity, scope=scope)


def clear_wasm_toolchain_selection(monkeypatch) -> None:
    """Drop every WASM selector an ambient projection set, and the SDK pins.

    A test that selects its own SDK must not inherit the real SDK's projection
    from a DX shell or ``tools/toolchain_env.py``: ``MOLT_LLVM_NM`` alone makes
    admission refuse the selected SDK (HF-231).
    """
    from molt import llvm_toolchain

    for key in (
        *llvm_toolchain.wasm_toolchain_environment_keys(),
        "WASI_SDK_PREFIX",
        "MOLT_WASM_CC",
        "MOLT_CROSS_CC",
    ):
        monkeypatch.delenv(key, raising=False)


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
        fixture_root.native_executables(
            tuple(
                str(
                    (sdk / "bin" / executable_filename(name, asset.id)).relative_to(
                        root
                    )
                )
                for name in (
                    "clang",
                    "clang++",
                    "llvm-ar",
                    "llvm-ranlib",
                    "llvm-nm",
                    "wasm-ld",
                    "llvm-strip",
                )
            )
        )
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
    member_link_args = {"shared": shared.link_flags, "reloc": reloc.link_flags}
    from molt._wasm_runtime_exports import wasm_split_runtime_export_rename_map

    assert shared.cargo_plan is not None
    member_publications = {
        "shared": RuntimePublicationAction(
            RuntimePublicationTransform.SHARED,
            shared.cargo_plan.preserve_debug_for_profile(shared.cargo_profile),
            publication_executor(RuntimePublicationTransform.SHARED),
            export_renames=tuple(
                sorted(wasm_split_runtime_export_rename_map(None).items())
            ),
        ),
        "reloc": runtime_reloc_publication_action(
            reloc.runtime_exports,
            module_name=reloc.publication_name,
            executor=publication_executor(RuntimePublicationTransform.RELOC),
        ),
    }
    shared_identity = runtime_build_identity(
        "shared",
        family_seed,
        compile_seed=compile_seed,
        member_link_args=member_link_args,
        member_publications=member_publications,
    )
    reloc_identity = runtime_build_identity(
        "reloc",
        family_seed,
        compile_seed=compile_seed,
        member_link_args=member_link_args,
        member_publications=member_publications,
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
    environment_transform=None,
    rustflags_transform=None,
    **_kwargs: object,
) -> RuntimeCargoPlan:
    """Live plan with immutable source input and pytest-owned generated tools."""
    root = project_root.resolve()
    fixtures = _fixture_path(fixture_root)
    environment = ProcessEnvironmentBuilder(env)
    environment["CARGO_HOME"] = str(fixtures / "test-cargo-home")
    if environment_transform is not None:
        environment = ProcessEnvironmentBuilder(
            environment_transform(MappingProxyType(environment)), rule=environment.rule
        )
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
    if derives_cargo_search_path():
        # The real plan derives Cargo's PATH from its final tools (HF-385).
        environment["PATH"] = cargo_search_path(
            environment, (*tool_paths.values(), *wrappers.values())
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
    command = flag_plan.command
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
        ProcessEnvironment(environment),
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
        _cargo_build_directory_base(root, environment),
    ).with_build_directory()


def write_runtime_crate(project_root: Path, source: bytes = b"before") -> Path:
    """A molt-runtime crate whose source closure the runtime identity resolves."""
    crate = project_root / "runtime" / "molt-runtime"
    path = crate / "src" / "lib.rs"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(source)
    (crate / "Cargo.toml").write_text(
        '[package]\nname = "molt-runtime"\nversion = "0.1.0"\n'
        'edition = "2024"\n[lib]\ncrate-type = ["staticlib"]\n',
        encoding="utf-8",
    )
    return path


def runtime_cargo_unit_stdout(project_root: Path, scratch: Path) -> str:
    """A complete successful Cargo invocation for the unit writing ``scratch``.

    Cargo's per-unit output owns its dep-info; the uplifted scratch archive
    remains the generation publisher's input, so the real coverage reader and
    the canonical runtime source roots both run.
    """
    crate = project_root / "runtime" / "molt-runtime"
    unit = scratch.parent / "deps" / "libmolt_runtime-0123456789abcdef.a"
    unit.parent.mkdir(parents=True, exist_ok=True)
    unit.write_bytes(scratch.read_bytes() if scratch.exists() else b"")
    (unit.parent / "molt_runtime-0123456789abcdef.d").write_text(
        f"{unit.name}: runtime/molt-runtime/src/lib.rs\n", encoding="utf-8"
    )
    artifact = {
        "reason": "compiler-artifact",
        "package_id": f"path+{crate.resolve().as_uri()}#0.1.0",
        "manifest_path": os.fspath(crate / "Cargo.toml"),
        "target": {
            "kind": ["staticlib"],
            "crate_types": ["staticlib"],
            "name": "molt_runtime",
            "src_path": os.fspath(crate / "src" / "lib.rs"),
        },
        "filenames": [os.fspath(unit)],
        "executable": None,
        "fresh": False,
    }
    return (
        json.dumps(artifact)
        + "\n"
        + json.dumps({"reason": "build-finished", "success": True})
        + "\n"
    )


def runtime_wasm_codegen_binding_fixture(
    root: Path, *, seed: str = "pair", pages: int = 2
):
    """An admitted pair with real memory/table facts for compiler consumers."""
    from molt._wasm_abi_generated import (
        WASM_RESERVED_RUNTIME_CALLABLE_BASE,
        WASM_RESERVED_RUNTIME_CALLABLES,
    )
    from molt.cli.runtime_wasm_generation import (
        bind_runtime_wasm_codegen,
        publish_runtime_wasm_generation,
    )
    from tests.wasm_callable_table_fixtures import _wasm_string, _wasm_u32

    def _runtime_module(table_min: int, pages: int) -> bytes:
        """An actual module with an imported table/memory and a complete ABI prefix."""
        prefix = WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * len(
            WASM_RESERVED_RUNTIME_CALLABLES
        )
        imports = (
            b"\x02"
            + _wasm_string("env")
            + _wasm_string("__indirect_function_table")
            + b"\x01\x70\x00"
            + _wasm_u32(table_min)
            + _wasm_string("env")
            + _wasm_string("memory")
            + b"\x02\x00"
            + _wasm_u32(pages)
        )
        sections = (
            (1, b"\x01\x60\x00\x00"),
            (2, imports),
            (3, b"\x01\x00"),
            (9, b"\x01\x00\x41\x01\x0b" + _wasm_u32(prefix) + bytes(prefix)),
            (10, b"\x01\x02\x00\x0b"),
        )
        return b"\x00asm\x01\x00\x00\x00" + b"".join(
            bytes([section]) + _wasm_u32(len(payload)) + payload
            for section, payload in sections
        )

    source = root / seed
    source.mkdir()
    shared = source / "shared.wasm"
    reloc = source / "reloc.wasm"
    shared.write_bytes(_runtime_module(256, pages))
    reloc.write_bytes(_runtime_module(128, pages + 1))
    pair = publish_runtime_wasm_generation(
        root / "molt_runtime.wasm",
        root / "molt_runtime_reloc.wasm",
        shared_identity=runtime_build_identity("shared", seed),
        reloc_identity=runtime_build_identity("reloc", seed),
        source_shared=shared,
        source_reloc=reloc,
        cargo_environment=runtime_cargo_environment_inputs(),
    )
    return bind_runtime_wasm_codegen(pair, None)


def runtime_cargo_stdout(root, outputs, environment, *, package="molt-runtime"):
    """Cargo artifacts and rustc dep-info for one synthetic local library unit.

    The unit reads an existing fixture source and MOLT_TEST_RUNTIME_INPUT;
    every reported output and its dep-info output rule names the same bytes.
    No source file is created or changed by this process-boundary fixture.
    """
    import json
    import os
    from pathlib import Path
    import shutil

    root = Path(root).resolve()
    folder = "molt-cpython-abi" if package == "molt-lang-cpython-abi" else package
    package_root = root / "runtime" / folder
    source = package_root / "src" / "lib.rs"
    if not source.is_file():
        raise ValueError(f"synthetic Cargo unit requires its existing source: {source}")
    crate = "molt_cpython_abi" if package == "molt-lang-cpython-abi" else "molt_runtime"
    paths = tuple(Path(path) for path in outputs)
    if not paths:
        raise ValueError("synthetic Cargo unit requires actual output bytes")
    reported = []
    for output in paths:
        if output.parent.name == "deps":
            reported.append(output)
        else:
            deps = output.parent / "deps"
            deps.mkdir(parents=True, exist_ok=True)
            unit = deps / (
                f"lib{crate}-hf302.a"
                if output.suffix == ".a"
                else f"{crate}-hf302{output.suffix}"
            )
            shutil.copyfile(output, unit)
            reported.append(unit)
    reported.extend(path for path in paths if path not in reported)

    def escape(value):
        return (
            str(value)
            .replace("\\", "\\\\")
            .replace(" ", "\\ ")
            .replace("#", "\\#")
            .replace(":", "\\:")
        )

    name = "MOLT_TEST_RUNTIME_INPUT"
    value = environment.get(name)
    variable = "# env-dep:" + name
    if value is not None:
        variable += "=" + value.replace("\\", "\\\\").replace("\n", "\\n").replace(
            "\r", "\\r"
        )
    rules = " ".join(escape(path) for path in reported) + ": " + escape(source)
    text = rules + "\n\n" + escape(source) + ":\n\n" + variable + "\n"
    for output in reported:
        stem = output.stem.removeprefix("lib")
        output.with_name(stem + ".d").write_text(text, encoding="utf-8")
    return (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": f"path+{package_root.as_uri()}#{package}@0.1.0",
                "manifest_path": str(package_root / "Cargo.toml"),
                "target": {"kind": ["lib"], "name": crate, "src_path": str(source)},
                "filenames": [os.fspath(path) for path in reported],
                "fresh": False,
            }
        )
        + "\n"
        + json.dumps({"reason": "build-finished", "success": True})
        + "\n"
    )
