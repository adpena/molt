"""Queue-native Pact and version-parity named-lane specifications."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
from typing import Mapping, Sequence

from molt.cli.extension_manifest import _default_molt_c_api_version
from molt.cli.file_hashing import _sha256_file
from molt.cli.source_extension_toolchain import (
    MOLT_PKGCONF_REQUIREMENT,
)
from molt.cli.source_package_seal import (
    SourcePackageSealVerificationError,
    verify_source_package_seal,
)
from molt.dx import select_external_artifact_root
from molt.scientific_stack_versions import (
    CONFIG_ENV as SCIENTIFIC_STACK_CONFIG_ENV,
)
from molt.scientific_stack_versions import (
    ScientificExtensionSet,
    ScientificStackVersion,
    attest_numpy_witness_seal,
    numpy_witness_seal_root,
    resolve_scientific_stack,
    scientific_extension_set,
    scipy_witness_seal_root,
)
from tools.proof_queue_pkg import policy, runner, state


def _pact_manifest_source_plan_roots(
    manifest: Mapping[str, object],
) -> tuple[Path, ...]:
    source_plan = manifest.get("source_plan")
    if not isinstance(source_plan, Mapping):
        return ()
    roots: list[Path] = []
    for field_name in ("source_root", "build_root"):
        value = source_plan.get(field_name)
        if isinstance(value, str) and value.strip():
            roots.append(Path(value).expanduser())
    return tuple(roots)



def _pact_manifest_object_sources_resolve(
    manifest: Mapping[str, object],
) -> bool:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return True
    objects = object_closure.get("objects")
    if not isinstance(objects, list):
        return True
    roots = _pact_manifest_source_plan_roots(manifest)
    for item in objects:
        if not isinstance(item, Mapping):
            continue
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            continue
        source_path = Path(source).expanduser()
        if source_path.is_absolute():
            return False
        if roots and not any((root / source_path).is_file() for root in roots):
            return False
    return True



def _pact_numpy_multiarray_seal_root_is_current(root: Path) -> bool:
    try:
        attest_numpy_witness_seal(root)
    except ValueError:
        return False
    manifest = policy._load_json_mapping(root / "extension_manifest.json")
    if manifest is None:
        return False
    runtime_imports = manifest.get("runtime_python_import_modules")
    if not (
        isinstance(runtime_imports, list)
        and any(isinstance(item, str) and item.strip() for item in runtime_imports)
    ):
        return False
    return _pact_manifest_object_sources_resolve(manifest)



def _scientific_extension_manifest_path(root: Path, module: str, target: str) -> Path:
    return root.joinpath(
        *module.split(".")[:-1], f"{target}.molt.wasm.extension_manifest.json"
    )



def _pact_object_closure_digest(object_closure: Mapping[str, object]) -> str | None:
    objects = object_closure.get("objects")
    runtime_symbols = object_closure.get("runtime_symbols")
    if not isinstance(objects, list) or not objects:
        return None
    if not isinstance(runtime_symbols, list) or not all(
        isinstance(item, str) for item in runtime_symbols
    ):
        return None
    digest_objects: list[dict[str, object]] = []
    for item in objects:
        if not isinstance(item, Mapping):
            return None
        digest_item = {
            key: item.get(key)
            for key in (
                "source",
                "object",
                "source_sha256",
                "object_sha256",
                "defined_symbols",
                "undefined_symbols",
            )
        }
        if not all(
            isinstance(digest_item[key], str) and digest_item[key]
            for key in ("source", "object", "source_sha256", "object_sha256")
        ):
            return None
        if not all(
            isinstance(digest_item[key], list)
            and all(isinstance(value, str) for value in digest_item[key])
            for key in ("defined_symbols", "undefined_symbols")
        ):
            return None
        if "compile_command" in item:
            compile_command = item.get("compile_command")
            if not isinstance(compile_command, list) or not compile_command or not all(
                isinstance(value, str) and value for value in compile_command
            ):
                return None
            digest_item["compile_command"] = compile_command
        if "symbol_command" in item:
            symbol_command = item.get("symbol_command")
            if not isinstance(symbol_command, list) or not symbol_command or not all(
                isinstance(value, str) and value for value in symbol_command
            ):
                return None
            digest_item["symbol_command"] = symbol_command
        if "dependencies" in item:
            dependencies = item.get("dependencies")
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, Mapping)
                and set(dependency) == {"path", "sha256"}
                and isinstance(dependency.get("path"), str)
                and bool(dependency.get("path"))
                and isinstance(dependency.get("sha256"), str)
                and bool(dependency.get("sha256"))
                for dependency in dependencies
            ):
                return None
            digest_item["dependencies"] = [dict(dependency) for dependency in dependencies]
        digest_objects.append(digest_item)
    payload = {
        "schema_version": 1,
        "root_symbol": object_closure.get("root_symbol"),
        "objects": digest_objects,
        "runtime_symbols": runtime_symbols,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()



def _pact_scipy_extension_set_manifest_problems(
    root: Path,
    extension_set: ScientificExtensionSet,
    *,
    stack: ScientificStackVersion,
    sidecars: Mapping[str, Mapping[str, object]],
) -> list[str]:
    path = root / "extension_set_manifest.json"
    manifest = policy._load_json_mapping(path)
    if manifest is None:
        return [f"missing or unreadable extension-set manifest {path}"]
    problems: list[str] = []
    expected_scalars: dict[str, object] = {
        "schema_version": 1,
        "kind": "molt-source-extension-set",
        "package": extension_set.package,
        "name": extension_set.name,
        "seal_name": extension_set.seal_name,
        "source_head": stack.scipy_repo_ref,
        "target": "wasm",
        "target_triple": "wasm32-wasip1",
        "abi_tier": "cpython-abi",
    }
    for field, expected in expected_scalars.items():
        if manifest.get(field) != expected:
            problems.append(f"extension-set manifest {field} must be {expected!r}")
    target_metadata = manifest.get("target_metadata")
    if not isinstance(target_metadata, Mapping):
        problems.append("extension-set manifest target_metadata is missing")
    else:
        target_identity = dict(target_metadata)
        target_digest = target_identity.pop("digest", None)
        computed_target_digest = hashlib.sha256(
            json.dumps(
                target_identity,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest()
        if target_digest != computed_target_digest:
            problems.append("extension-set target_metadata identity checksum mismatch")
        target_sidecar = policy._load_json_mapping(
            root
            / "provenance"
            / "metadata"
            / "target"
            / "source-extension-target-metadata.json"
        )
        if target_sidecar != target_metadata:
            problems.append(
                "extension-set target_metadata differs from canonical sidecar"
            )
        target_digests = target_metadata.get("digests")
        target_files = {
            "python_pc_sha256": (
                root / "provenance/metadata/target/pkgconfig/python3.pc"
            ),
            "meson_cross_sha256": (
                root / "provenance/metadata/target/meson.cross"
            ),
        }
        if not isinstance(target_digests, Mapping):
            problems.append("extension-set target_metadata digests are missing")
        else:
            for digest_name, target_file in target_files.items():
                if (
                    not target_file.is_file()
                    or target_digests.get(digest_name) != _sha256_file(target_file)
                ):
                    problems.append(
                        f"extension-set target_metadata {digest_name} mismatch"
                    )
        toolchain = target_metadata.get("toolchain")
        tools = toolchain.get("tools") if isinstance(toolchain, Mapping) else None
        raw_target_commands = (
            toolchain.get("commands") if isinstance(toolchain, Mapping) else None
        )
        tool_roles = {
            "ar": "ar",
            "c": "cc",
            "cpp": "cxx",
            "ld": "wasm_ld",
            "nm": "nm",
            "ranlib": "ranlib",
            "strip": "strip",
        }
        if not isinstance(tools, Mapping) or set(tools) != set(tool_roles.values()):
            problems.append("extension-set target tool identity family is incomplete")
        if not isinstance(raw_target_commands, Mapping) or set(
            raw_target_commands
        ) != set(tool_roles):
            problems.append("extension-set target command family is incomplete")
        elif isinstance(tools, Mapping):
            for command_role, tool_role in tool_roles.items():
                identity = tools.get(tool_role)
                command = raw_target_commands.get(command_role)
                if not (
                    isinstance(identity, Mapping)
                    and isinstance(identity.get("command"), list)
                    and identity.get("command")
                    and isinstance(identity.get("path"), str)
                    and isinstance(identity.get("sha256"), str)
                    and len(str(identity.get("sha256"))) == 64
                    and isinstance(command, list)
                    and command
                    and command[0] == identity["command"][0]
                ):
                    problems.append(
                        f"extension-set target {command_role} identity is invalid"
                    )
    build_environment = manifest.get("build_environment")
    if not isinstance(build_environment, Mapping) or set(build_environment) != {
        "python_executable",
        "requirements",
        "resolved",
    }:
        problems.append("extension-set manifest build_environment shape is invalid")
    else:
        python_executable = build_environment.get("python_executable")
        requirements = build_environment.get("requirements")
        resolved = build_environment.get("resolved")
        if not isinstance(python_executable, str) or not python_executable:
            problems.append("extension-set manifest build Python executable is empty")
        if (
            not isinstance(requirements, list)
            or not requirements
            or not all(isinstance(item, str) and item for item in requirements)
        ):
            problems.append("extension-set manifest build requirements are invalid")
        if not isinstance(resolved, list) or not resolved:
            problems.append("extension-set manifest resolved requirements are empty")
        elif isinstance(requirements, list):
            requirement_positions = {
                requirement: index
                for index, requirement in enumerate(requirements)
                if isinstance(requirement, str)
            }
            resolved_positions: list[int] = []
            resolved_requirements: list[str] = []
            seen_requirements: set[str] = set()
            for item in resolved:
                if not isinstance(item, Mapping) or set(item) != {
                    "requirement",
                    "distribution",
                    "version",
                }:
                    problems.append(
                        "extension-set manifest resolved requirement shape is invalid"
                    )
                    continue
                if not all(
                    isinstance(item.get(field), str) and item.get(field)
                    for field in ("requirement", "distribution", "version")
                ):
                    problems.append(
                        "extension-set manifest resolved requirement values are invalid"
                    )
                    continue
                requirement = str(item["requirement"])
                position = requirement_positions.get(requirement)
                if position is None or requirement in seen_requirements:
                    problems.append(
                        "extension-set manifest resolved requirements do not match "
                        "the original requirement authority"
                    )
                    continue
                seen_requirements.add(requirement)
                resolved_positions.append(position)
                resolved_requirements.append(requirement)
            if resolved_positions != sorted(resolved_positions):
                problems.append(
                    "extension-set manifest resolved requirements are out of source order"
                )
            if resolved_requirements != requirements:
                problems.append(
                    "extension-set manifest resolved requirements do not exactly "
                    "cover the source requirement authority"
                )
    meson = manifest.get("meson")
    expected_meson_fields = {
        "build_root",
        "setup_args",
        "intro_targets_sha256",
        "compile_commands_sha256",
        "intro_installed_sha256",
        "config_tool_cross_sha256",
        "config_tools",
        "pkg_config_requirement",
    }
    if not isinstance(meson, Mapping) or set(meson) != expected_meson_fields:
        problems.append("extension-set manifest meson metadata is missing")
    else:
        build_root = meson.get("build_root")
        if not isinstance(build_root, str) or not build_root:
            problems.append("extension-set manifest meson.build_root is empty")
        if tuple(meson.get("setup_args") or ()) != extension_set.meson_setup_args:
            problems.append("extension-set manifest Meson setup_args drift")
        for field in (
            "intro_targets_sha256",
            "compile_commands_sha256",
            "intro_installed_sha256",
            "config_tool_cross_sha256",
        ):
            value = meson.get(field)
            if (
                not isinstance(value, str)
                or len(value) != 64
                or any(character not in "0123456789abcdef" for character in value)
            ):
                problems.append(
                    f"extension-set manifest meson.{field} is not a SHA-256 digest"
                )
        if meson.get("pkg_config_requirement") != MOLT_PKGCONF_REQUIREMENT:
            problems.append("extension-set manifest Meson pkg-config requirement drift")
        config_tools = meson.get("config_tools")
        expected_config_tool_distributions = {
            "numpy-config": "numpy",
            "pkg-config": "pkgconf",
            "pybind11-config": "pybind11",
            "pythran-config": "pythran",
        }
        if not isinstance(config_tools, list) or not all(
            isinstance(item, Mapping) for item in config_tools
        ):
            problems.append("extension-set manifest Meson config_tools are invalid")
        else:
            actual_names = tuple(item.get("name") for item in config_tools)
            expected_names = tuple(sorted(expected_config_tool_distributions))
            if actual_names != expected_names:
                problems.append(
                    "extension-set manifest Meson config tool set/order drift"
                )
            for item in config_tools:
                if set(item) != {
                    "name",
                    "path",
                    "distribution",
                    "version",
                    "sha256",
                } or not all(
                    isinstance(item.get(field), str) and item.get(field)
                    for field in (
                        "name",
                        "path",
                        "distribution",
                        "version",
                        "sha256",
                    )
                ):
                    problems.append(
                        "extension-set manifest Meson config tool shape is invalid"
                    )
                    continue
                sha256 = str(item["sha256"])
                if len(sha256) != 64 or any(
                    character not in "0123456789abcdef" for character in sha256
                ):
                    problems.append(
                        "extension-set manifest Meson config tool sha256 is invalid"
                    )
                name = str(item["name"])
                expected_distribution = expected_config_tool_distributions.get(name)
                if (
                    expected_distribution is not None
                    and str(item["distribution"]).casefold()
                    != expected_distribution.casefold()
                ):
                    problems.append(
                        f"extension-set manifest Meson {name} distribution drift"
                    )
                if (
                    name == "pkg-config"
                    and item.get("version")
                    != (MOLT_PKGCONF_REQUIREMENT.split("==", 1)[1])
                ):
                    problems.append(
                        "extension-set manifest Meson pkg-config version drift"
                    )
    installed_files = manifest.get("installed_python_files")
    required_installed = {
        "scipy/__init__.py",
        "scipy/version.py",
        "scipy/__config__.py",
    }
    if not isinstance(installed_files, list) or not all(
        isinstance(item, str) and item for item in installed_files
    ):
        problems.append("extension-set manifest installed_python_files is invalid")
    else:
        missing_installed = sorted(required_installed - set(installed_files))
        if missing_installed:
            problems.append(
                "extension-set manifest missing installed Python files: "
                + ", ".join(missing_installed)
            )
        missing_on_disk = sorted(
            item for item in required_installed if not (root / item).is_file()
        )
        if missing_on_disk:
            problems.append(
                "extension-set installed Python files absent on disk: "
                + ", ".join(missing_on_disk)
            )
    raw_extensions = manifest.get("extensions")
    expected_contracts = tuple(
        (extension.module, extension.target, extension.capabilities)
        for extension in extension_set.extensions
    )
    if not isinstance(raw_extensions, list) or not all(
        isinstance(item, Mapping) for item in raw_extensions
    ):
        problems.append("extension-set manifest extensions is invalid")
        return problems
    actual_contracts: list[tuple[object, object, tuple[str, ...]]] = []
    for item in raw_extensions:
        capabilities = item.get("capabilities")
        if not isinstance(capabilities, list) or not all(
            isinstance(value, str) for value in capabilities
        ):
            problems.append("extension-set manifest extension capabilities are invalid")
            normalized_capabilities: tuple[str, ...] = ()
        else:
            normalized_capabilities = tuple(capabilities)
        actual_contracts.append(
            (item.get("module"), item.get("target"), normalized_capabilities)
        )
    if tuple(actual_contracts) != expected_contracts:
        problems.append(
            "extension-set manifest ordered module/target/capability set drift"
        )
    for raw_extension in raw_extensions:
        module = raw_extension.get("module")
        if not isinstance(module, str):
            continue
        sidecar = sidecars.get(module)
        if sidecar is None:
            continue
        object_closure = sidecar.get("object_closure")
        closure_sha256 = (
            object_closure.get("closure_sha256")
            if isinstance(object_closure, Mapping)
            else None
        )
        checksum_pairs = (
            ("artifact_sha256", sidecar.get("extension_sha256")),
            ("wheel_sha256", sidecar.get("wheel_sha256")),
            ("object_closure_sha256", closure_sha256),
        )
        for field, expected in checksum_pairs:
            value = raw_extension.get(field)
            if not isinstance(value, str) or not value or value != expected:
                problems.append(
                    f"extension-set manifest {module} {field} differs from sidecar"
                )
    return problems



def _pact_scipy_witness_seal_problems(
    root: Path,
    extension_set: ScientificExtensionSet,
    stack: ScientificStackVersion | None = None,
) -> list[str]:
    selected = resolve_scientific_stack() if stack is None else stack
    problems: list[str] = []
    try:
        verified_seal = verify_source_package_seal(root)
    except (OSError, SourcePackageSealVerificationError) as exc:
        problems.append(f"source-package seal verification failed: {exc}")
        payload_root = root / "files"
        if not payload_root.is_dir():
            return problems
    else:
        payload_root = verified_seal.payload_root
    expected_manifests = {
        _scientific_extension_manifest_path(
            payload_root, extension.module, extension.target
        ).resolve(): extension.module
        for extension in extension_set.extensions
    }
    actual_manifests = {
        manifest.resolve()
        for manifest in payload_root.glob("**/*.molt.wasm.extension_manifest.json")
        if manifest.is_file()
    }
    for missing in sorted(expected_manifests.keys() - actual_manifests):
        problems.append(
            "missing "
            f"{expected_manifests[missing]} manifest "
            f"{missing.relative_to(payload_root.resolve())}"
        )
    for unexpected in sorted(actual_manifests - expected_manifests.keys()):
        problems.append(
            "unexpected SciPy extension manifest "
            f"{unexpected.relative_to(payload_root.resolve())}"
        )
    current_abi = _default_molt_c_api_version(state.ROOT)
    current_abi_tag = f"molt_abi{current_abi.split('.', 1)[0]}"
    set_manifest = policy._load_json_mapping(payload_root / "extension_set_manifest.json")
    set_target_metadata = (
        set_manifest.get("target_metadata")
        if isinstance(set_manifest, Mapping)
        else None
    )
    set_toolchain = (
        set_target_metadata.get("toolchain")
        if isinstance(set_target_metadata, Mapping)
        else None
    )
    raw_target_commands = (
        set_toolchain.get("commands") if isinstance(set_toolchain, Mapping) else None
    )
    target_commands = (
        raw_target_commands if isinstance(raw_target_commands, Mapping) else None
    )
    sidecars: dict[str, Mapping[str, object]] = {}
    for extension in extension_set.extensions:
        manifest_path = _scientific_extension_manifest_path(
            payload_root, extension.module, extension.target
        )
        manifest = policy._load_json_mapping(manifest_path)
        if manifest is None:
            if manifest_path.resolve() in actual_manifests:
                problems.append(f"{extension.module}: unreadable extension manifest")
            continue
        sidecars[extension.module] = manifest
        if manifest.get("module") != extension.module:
            problems.append(f"{extension.module}: manifest module mismatch")
        if manifest.get("molt_c_api_version") != current_abi:
            problems.append(f"{extension.module}: stale molt_c_api_version")
        if manifest.get("abi_tag") != current_abi_tag:
            problems.append(f"{extension.module}: stale abi_tag")
        if manifest.get("loader_kind") != "libmolt_source":
            problems.append(f"{extension.module}: loader_kind must be libmolt_source")
        if manifest.get("target_triple") != "wasm32-wasip1":
            problems.append(f"{extension.module}: target_triple must be wasm32-wasip1")
        if manifest.get("runtime_linkage") != "static_link":
            problems.append(f"{extension.module}: runtime_linkage must be static_link")
        if manifest.get("artifact_kind") != "wasm_relocatable_object":
            problems.append(
                f"{extension.module}: artifact_kind must be wasm_relocatable_object"
            )
        if manifest.get("deterministic") is not True:
            problems.append(f"{extension.module}: deterministic must be true")
        if manifest.get("capabilities") != list(extension.capabilities):
            problems.append(f"{extension.module}: capabilities drift")
        expected_init_symbol = f"PyInit_{extension.module.rsplit('.', 1)[-1]}"
        if manifest.get("init_symbol") != expected_init_symbol:
            problems.append(f"{extension.module}: init_symbol mismatch")
        source_plan = manifest.get("source_plan")
        if (
            not isinstance(source_plan, Mapping)
            or source_plan.get("target_selector") != extension.target
        ):
            problems.append(f"{extension.module}: Meson source_plan target mismatch")
        raw_exports = manifest.get("python_exports")
        if not isinstance(raw_exports, list) or not all(
            isinstance(item, str) and item for item in raw_exports
        ):
            problems.append(f"{extension.module}: invalid python_exports")
        elif tuple(raw_exports) != extension.python_exports:
            problems.append(f"{extension.module}: python_exports drift")
        artifact = manifest.get("extension")
        expected_artifact = f"{extension.target}.molt.wasm"
        if (
            not isinstance(artifact, str)
            or artifact != expected_artifact
            or not (manifest_path.parent / artifact).is_file()
        ):
            problems.append(f"{extension.module}: extension artifact is missing")
        else:
            artifact_path = manifest_path.parent / artifact
            artifact_sha256 = manifest.get("extension_sha256")
            try:
                actual_sha256 = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
            except OSError:
                actual_sha256 = ""
            if not isinstance(artifact_sha256, str) or actual_sha256 != artifact_sha256:
                problems.append(f"{extension.module}: extension_sha256 mismatch")
        object_closure = manifest.get("object_closure")
        if not isinstance(object_closure, Mapping):
            problems.append(f"{extension.module}: object_closure is missing")
        else:
            if object_closure.get("root_symbol") != expected_init_symbol:
                problems.append(
                    f"{extension.module}: object_closure root_symbol mismatch"
                )
            owner = object_closure.get("init_symbol_owner")
            if not isinstance(owner, str) or not owner:
                problems.append(
                    f"{extension.module}: object_closure init_symbol_owner is empty"
                )
            objects = object_closure.get("objects")
            if not isinstance(objects, list) or not objects:
                problems.append(f"{extension.module}: object_closure is empty")
            elif all(isinstance(item, Mapping) for item in objects):
                for object_index, item in enumerate(objects):
                    compile_command = item.get("compile_command")
                    if not isinstance(compile_command, list) or not compile_command or not all(
                        isinstance(value, str) and value for value in compile_command
                    ):
                        problems.append(
                            f"{extension.module}: object_closure compile command is invalid"
                        )
                    symbol_command = item.get("symbol_command")
                    if not isinstance(symbol_command, list) or not symbol_command or not all(
                        isinstance(value, str) and value for value in symbol_command
                    ):
                        problems.append(
                            f"{extension.module}: object_closure symbol command is invalid"
                        )
                    if target_commands is not None:
                        source_value = item.get("source")
                        compiler_role = (
                            "cpp"
                            if isinstance(source_value, str)
                            and Path(source_value).suffix.lower()
                            in {".cc", ".cpp", ".cxx", ".c++"}
                            else "c"
                        )
                        expected_compiler = target_commands.get(compiler_role)
                        expected_nm = target_commands.get("nm")
                        if not (
                            isinstance(expected_compiler, list)
                            and isinstance(compile_command, list)
                            and compile_command[: len(expected_compiler)]
                            == expected_compiler
                            and isinstance(expected_nm, list)
                            and symbol_command == expected_nm
                        ):
                            problems.append(
                                f"{extension.module}: object_closure did not consume "
                                "the target compiler/nm commands"
                            )
                    source = item.get("source")
                    source_sha256 = item.get("source_sha256")
                    if not isinstance(source, str) or not isinstance(
                        source_sha256, str
                    ):
                        problems.append(
                            f"{extension.module}: object_closure source custody is invalid"
                        )
                        continue
                    source_path = (manifest_path.parent / source).resolve()
                    if (
                        Path(source).is_absolute()
                        or not source_path.is_relative_to(payload_root.resolve())
                        or not source_path.is_file()
                        or _sha256_file(source_path) != source_sha256
                    ):
                        problems.append(
                            f"{extension.module}: object_closure source[{object_index}] "
                            "is not sealed or checksummed"
                        )
                    dependencies = item.get("dependencies", [])
                    if not isinstance(dependencies, list):
                        problems.append(
                            f"{extension.module}: object_closure dependencies are invalid"
                        )
                        continue
                    for dependency_index, dependency in enumerate(dependencies):
                        if not isinstance(dependency, Mapping):
                            problems.append(
                                f"{extension.module}: object_closure dependency is invalid"
                            )
                            continue
                        raw_path = dependency.get("path")
                        expected_sha256 = dependency.get("sha256")
                        if not isinstance(raw_path, str) or not isinstance(
                            expected_sha256, str
                        ):
                            problems.append(
                                f"{extension.module}: object_closure dependency is invalid"
                            )
                            continue
                        dependency_path = (manifest_path.parent / raw_path).resolve()
                        if (
                            Path(raw_path).is_absolute()
                            or not dependency_path.is_relative_to(payload_root.resolve())
                            or not dependency_path.is_file()
                            or _sha256_file(dependency_path) != expected_sha256
                        ):
                            problems.append(
                                f"{extension.module}: object_closure dependency"
                                f"[{object_index}][{dependency_index}] is not sealed "
                                "or checksummed"
                            )
            closure_sha256 = object_closure.get("closure_sha256")
            computed_closure_sha256 = _pact_object_closure_digest(object_closure)
            if (
                not isinstance(closure_sha256, str)
                or not closure_sha256
                or closure_sha256 != computed_closure_sha256
            ):
                problems.append(f"{extension.module}: object_closure checksum mismatch")
    problems.extend(
        _pact_scipy_extension_set_manifest_problems(
            payload_root,
            extension_set,
            stack=selected,
            sidecars=sidecars,
        )
    )
    return problems



def _pact_scipy_witness_seal_root_is_complete(
    root: Path, extension_set: ScientificExtensionSet
) -> bool:
    return not _pact_scipy_witness_seal_problems(root, extension_set)



def _pact_witness_native_roots(repo_root: Path = state.ROOT) -> list[Path]:
    del repo_root
    stack = resolve_scientific_stack()
    durable_numpy_root = numpy_witness_seal_root(stack=stack)
    if not (
        durable_numpy_root.exists()
        and _pact_numpy_multiarray_seal_root_is_current(durable_numpy_root)
    ):
        raise ValueError(
            "no attested NumPy witness seal matches configured version "
            f"{stack.numpy}; expected durable root {durable_numpy_root}. "
            "Run tools/provision_numpy_witness_seal.py with a genuine matching seal."
        )
    scipy_set = scientific_extension_set("scipy", "pact-witness", stack=stack)
    durable_scipy_root = scipy_witness_seal_root(stack=stack)
    scipy_problems = (
        _pact_scipy_witness_seal_problems(durable_scipy_root, scipy_set, stack)
        if durable_scipy_root.exists()
        else ["canonical root does not exist"]
    )
    if scipy_problems:
        raise ValueError(
            "canonical SciPy witness seal is absent or incomplete; expected "
            f"{durable_scipy_root} with exactly the configured extension set: "
            + "; ".join(scipy_problems)
        )
    verified_scipy = verify_source_package_seal(durable_scipy_root)
    return [durable_numpy_root.resolve(), verified_scipy.payload_root.resolve()]



def _pact_witness_env_overrides(repo_root: Path = state.ROOT) -> dict[str, str]:
    # Force UTF-8 across the ENTIRE witness process tree (the parent tool + every
    # spawned build/gate subprocess). On Windows the default cp1252 stdio codec
    # raises UnicodeEncodeError on any non-cp1252 char in a relayed subprocess
    # capture (e.g. a gate's em-dash decoded to U+FFFD), which once aborted an
    # otherwise-SUCCESSFUL witness build after ~20 min. PYTHONUTF8=1 makes stdio
    # and the default file encoding UTF-8 tree-wide — the single-primitive fix for
    # this recurring encoding bug class. Set unconditionally (independent of the
    # native-root delta below) so the guarantee holds on every witness path.
    env: dict[str, str] = {
        "PYTHONUTF8": "1",
        "PYTHONIOENCODING": "utf-8",
        "MOLT_MODULE_ROOTS": "",
        "MOLT_EXTERNAL_STATIC_PACKAGES": "",
    }
    roots = _pact_witness_native_roots(repo_root)
    if roots:
        env["MOLT_MODULE_ROOTS"] = os.pathsep.join(str(root) for root in roots)
        env["MOLT_EXTERNAL_STATIC_PACKAGES"] = "numpy scipy"
    return env



_PACT_WITNESS_ACCEPTANCE_LOGICAL_ID = "pact-witness-acceptance"

_PACT_WITNESS_ACCEPTANCE_LOCKED_ENV = (
    "MOLT_MODULE_ROOTS",
    "MOLT_EXTERNAL_STATIC_PACKAGES",
    "MOLT_WITNESS_EXPECTED_REPO_ROOT",
    "MOLT_WITNESS_EXPECTED_GIT_HEAD",
    SCIENTIFIC_STACK_CONFIG_ENV,
    "MOLT_EXT_ROOT",
    "MOLT_EXTERNAL_ARTIFACT_ROOTS",
    "PYTHONUTF8",
    "PYTHONIOENCODING",
)



def _pact_canonical_input_environment(repo_root: Path) -> dict[str, str]:
    """Resolve named Pact input custody without consulting ambient overrides."""
    root = repo_root.resolve()
    config_path = root / "config" / "scientific_stack_versions.toml"
    if not config_path.is_file():
        raise SystemExit(
            f"named Pact proof is missing canonical stack config {config_path}"
        )
    selection_env = {"MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1"} if os.name == "nt" else {}
    artifact_root = select_external_artifact_root(
        root,
        selection_env,
        create_dirs=False,
        prefer_external=True,
    )
    if artifact_root is None:
        # This fallback is a stable repo-derived identity for hosts without a
        # configured external volume. Seal verification still fails closed if
        # the durable package roots are absent; ambient variables cannot choose
        # a different tree.
        artifact_root = root / "tmp" / "pact-artifacts"
    canonical_artifact_root = str(artifact_root.resolve())
    return {
        SCIENTIFIC_STACK_CONFIG_ENV: str(config_path.resolve()),
        "MOLT_EXT_ROOT": canonical_artifact_root,
        "MOLT_EXTERNAL_ARTIFACT_ROOTS": canonical_artifact_root,
    }



@contextlib.contextmanager
def _temporary_environment(overrides: Mapping[str, str]):
    previous = {name: os.environ.get(name) for name in overrides}
    try:
        os.environ.update(overrides)
        yield
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value



def _pact_witness_acceptance_spec(
    timeout: float | None = None, repo_root: Path = state.ROOT
) -> dict[str, object]:
    canonical_inputs = _pact_canonical_input_environment(repo_root)
    with _temporary_environment(canonical_inputs):
        stack = resolve_scientific_stack()
        git_snapshot = state._git_snapshot(repo_root)
        expected_head = git_snapshot.get("head")
        if not isinstance(expected_head, str) or not expected_head:
            raise SystemExit(
                "pact-witness-acceptance requires a git worktree with a resolvable HEAD"
            )
        env_overrides = _pact_witness_env_overrides(repo_root)
    env_overrides.update(canonical_inputs)
    env_overrides.update(
        {
            "MOLT_WITNESS_EXPECTED_REPO_ROOT": str(repo_root.resolve()),
            "MOLT_WITNESS_EXPECTED_GIT_HEAD": expected_head,
        }
    )
    return {
        "logical_id": _PACT_WITNESS_ACCEPTANCE_LOGICAL_ID,
        "reason": (
            "Run the Pact Kernel A browser/WASM witness acceptance aperture "
            "through queue custody."
        ),
        "command": policy._uv_active_python_command(
            "tools/pact_witness_acceptance.py",
            "--out-dir",
            "tmp/pact_witness_acceptance_queue",
            with_packages=[stack.numpy_requirement, stack.scipy_requirement],
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "wasm/browser_embed.js",
            "wasm/browser_host.js",
            "wasm/run_wasm.js",
            "tools/pact_witness_acceptance.py",
            "config/scientific_stack_versions.toml",
        ],
        "env_overrides": env_overrides,
        "locked_env": _PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
        "notes": [
            "Named Pact acceptance requires the version-keyed durable NumPy "
            "and canonical four-extension SciPy seals, builds field_solve.py, "
            "regenerates the fixture/reference oracle in the run directory, "
            "runs the WASM artifact to produce candidate_outputs.npz, and "
            "executes check_parity.py; --env remains available for diagnostics "
            "but cannot override the named lane's input and identity custody."
        ],
        "timeout": timeout if timeout is not None else 1800.0,
    }



def _pact_witness_oracle_spec(timeout: float | None = None) -> dict[str, object]:
    stack = resolve_scientific_stack()
    return {
        "logical_id": "pact-witness-oracle-parity",
        "reason": (
            "Regenerate the Pact Kernel A fixture/reference pair and prove the "
            "check_parity.py oracle under queue custody."
        ),
        "command": policy._uv_active_python_command(
            "tools/pact_witness_oracle.py",
            with_packages=[stack.numpy_requirement, stack.scipy_requirement],
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "tools/pact_witness_oracle.py",
        ],
        "env_overrides": {},
        "timeout": timeout if timeout is not None else 900.0,
    }



_R6_TARGET_VERSION_PARITY_FILES = (
    "tests/differential/stdlib/sys_metadata_intrinsics.py",
    "tests/differential/stdlib/sys_stat_version_gate.py",
    "tests/differential/stdlib/stat_api_surface_versioned.py",
    "tests/differential/stdlib/queue_shutdown_version_gate.py",
    "tests/differential/stdlib/removed_stdlib_modules_version_gate.py",
)



def _normalize_r6_target_version_fixtures(
    requested: Sequence[str] | None,
) -> list[str]:
    if not requested:
        return list(_R6_TARGET_VERSION_PARITY_FILES)
    by_alias: dict[str, str] = {}
    for fixture in _R6_TARGET_VERSION_PARITY_FILES:
        path = Path(fixture)
        aliases = {
            fixture,
            fixture.replace("\\", "/"),
            path.name,
            path.stem,
        }
        for alias in aliases:
            by_alias[alias.lower()] = fixture
    selected: list[str] = []
    for raw in requested:
        normalized = raw.replace("\\", "/").lower()
        fixture = by_alias.get(normalized)
        if fixture is None:
            allowed = ", ".join(
                Path(item).name for item in _R6_TARGET_VERSION_PARITY_FILES
            )
            raise SystemExit(
                f"unknown R6 target-version fixture {raw!r}; choose one of: {allowed}"
            )
        if fixture not in selected:
            selected.append(fixture)
    return selected



def _r6_target_version_fixture_suffix(fixtures: Sequence[str]) -> str:
    if tuple(fixtures) == _R6_TARGET_VERSION_PARITY_FILES:
        return ""
    stems = [state._slug(Path(fixture).stem) for fixture in fixtures]
    suffix = "-".join(stems)
    if len(suffix) <= 96:
        return suffix
    digest = hashlib.sha256("|".join(fixtures).encode("utf-8")).hexdigest()[:10]
    return f"{stems[0]}-plus-{len(stems) - 1}-{digest}"



def _r6_target_version_parity_spec(
    python_version: str,
    timeout: float | None = None,
    fixtures: Sequence[str] | None = None,
) -> dict[str, object]:
    normalized_version = python_version.strip()
    if not normalized_version:
        raise SystemExit("--python-version must not be empty")
    target_tag = "py" + "".join(normalized_version.split(".")[:2])
    selected_fixtures = _normalize_r6_target_version_fixtures(fixtures)
    fixture_suffix = _r6_target_version_fixture_suffix(selected_fixtures)
    logical_id = f"r6-target-version-parity-{target_tag}"
    if fixture_suffix:
        logical_id = f"{logical_id}-{fixture_suffix}"
    return {
        "logical_id": logical_id,
        "reason": (
            "Run the R6 target-version parity shard through queue custody with "
            "the differential harness and TargetPythonVersion command authority."
        ),
        "command": policy._uv_active_python_command(
            "tests/molt_diff.py",
            "--jobs",
            "1",
            "--python-version",
            normalized_version,
            "--build-profile",
            "dev",
            "--fail-fast",
            *selected_fixtures,
        ),
        "resource_family": "python",
        "contention_key": f"python:r6-target-version-{target_tag}",
        "scopes": [
            "tools/target_python_runtime.py",
            "tests/molt_diff.py",
            "src/molt/cli/target_python.py",
            "src/molt/stdlib/sys.py",
            "src/molt/stdlib/stat.py",
            "src/molt/stdlib/queue.py",
            *selected_fixtures,
        ],
        "env_overrides": {},
        "notes": [
            "Named R6 parity lane runs sys metadata plus stdlib version-gated "
            "stat, queue shutdown, and PEP 594 removed-module fixtures with "
            "serial fail-fast differential custody; missing target interpreters "
            "fail closed through tools/target_python_runtime.py.",
            "Selected R6 fixtures: " + ", ".join(selected_fixtures),
        ],
        "timeout": timeout if timeout is not None else 900.0,
    }



def _native_molt_run_spec(
    entry: str,
    *,
    script_args: Sequence[str] | None = None,
    timeout: float | None = None,
    repo_root: Path = state.ROOT,
) -> dict[str, object]:
    root = repo_root.resolve()
    entry_path = Path(entry)
    if not entry_path.is_absolute():
        entry_path = root / entry_path
    entry_path = entry_path.resolve()
    try:
        rel_entry = entry_path.relative_to(root)
    except ValueError as exc:
        raise SystemExit(
            f"native Molt run entry must live under repo root {root}: {entry_path}"
        ) from exc
    if not entry_path.is_file():
        raise SystemExit(f"native Molt run entry does not exist: {entry_path}")
    entry_scope = rel_entry.as_posix()
    arg_list = list(script_args or [])
    if arg_list[:1] == ["--"]:
        arg_list = arg_list[1:]
    entry_slug = state._slug(entry_scope)
    digest = hashlib.sha256(entry_scope.encode("utf-8")).hexdigest()[:10]
    return {
        "logical_id": f"native-molt-run-{entry_slug}-{digest}",
        "reason": (
            "Run a native Molt entrypoint through proof-queue custody instead "
            "of a foreground Codex shell compile."
        ),
        "command": policy._uv_active_python_command(
            "-m",
            "molt.cli",
            "run",
            entry_scope,
            *arg_list,
            no_sync=True,
        ),
        "resource_family": "python-native",
        "contention_key": f"python:native-molt-run:{entry_slug}",
        "scopes": [entry_scope],
        "env_overrides": {},
        "notes": [
            "Named native Molt run lane prevents compile-heavy `molt run` probes "
            "from occupying the foreground Codex control plane; use --detach "
            "and `proof_queue.py run --jobs N --detach` for cross-platform "
            "bounded worker fanout.",
            "Native Molt entry: " + entry_scope,
        ],
        "timeout": timeout if timeout is not None else 900.0,
    }



def _run_named_spec(args: argparse.Namespace, spec: dict[str, object]) -> int:
    env_overrides = policy._named_spec_env_overrides(spec, args.env)
    initial_notes = state._notes_from_raw(spec.get("note"))
    initial_notes.extend(state._notes_from_raw(spec.get("notes")))
    initial_notes.extend(getattr(args, "note", []) or [])
    runnable = {
        **spec,
        "env_overrides": env_overrides,
    }
    if args.print_spec:
        print(json.dumps(runnable, indent=2, sort_keys=True))
        return 0
    if getattr(args, "queue_only", False):
        rc, _run_id = runner._queue_one(
            args,
            logical_id=str(runnable["logical_id"]),
            reason=str(runnable["reason"]),
            command=list(runnable["command"]),
            resource_family=str(runnable["resource_family"]),
            contention_key=str(runnable["contention_key"]),
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        return rc
    if getattr(args, "detach", False):
        rc, run_id = runner._queue_one(
            args,
            logical_id=str(runnable["logical_id"]),
            reason=str(runnable["reason"]),
            command=list(runnable["command"]),
            resource_family=str(runnable["resource_family"]),
            contention_key=str(runnable["contention_key"]),
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        if rc != 0 or run_id is None:
            return rc
        conn = state._connect(state._db_path(args))
        dispatch = runner._dispatch_detached_runner(
            args,
            conn,
            run_id=run_id,
            timeout=float(runnable["timeout"]),
        )
        if dispatch is None:
            return 0
        pid, runner_log = dispatch
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return runner._run_one(
        args,
        logical_id=str(runnable["logical_id"]),
        reason=str(runnable["reason"]),
        command=list(runnable["command"]),
        resource_family=str(runnable["resource_family"]),
        contention_key=str(runnable["contention_key"]),
        scopes=list(runnable["scopes"]),
        env_overrides=dict(runnable["env_overrides"]),
        timeout=float(runnable["timeout"]),
        initial_notes=initial_notes,
        depends_on=getattr(args, "depends_on", []) or [],
        edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
        edge_note=getattr(args, "edge_note", None),
    )



def _cmd_pact_witness_acceptance(args: argparse.Namespace) -> int:
    # Admission must precede seal/config resolution so a forbidden override
    # cannot redirect spec construction or turn a policy refusal into a producer
    # traceback. _run_named_spec revalidates the completed spec before use.
    policy._named_spec_user_env_overrides(
        _PACT_WITNESS_ACCEPTANCE_LOGICAL_ID,
        _PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
        args.env,
    )
    return _run_named_spec(
        args, _pact_witness_acceptance_spec(args.timeout, state._repo_root(args))
    )



def _cmd_pact_witness_oracle(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_oracle_spec(args.timeout))



def _cmd_r6_target_version_parity(args: argparse.Namespace) -> int:
    return _run_named_spec(
        args,
        _r6_target_version_parity_spec(
            args.python_version,
            args.timeout,
            args.fixture,
        ),
    )



def _cmd_native_molt_run(args: argparse.Namespace) -> int:
    return _run_named_spec(
        args,
        _native_molt_run_spec(
            args.entry,
            script_args=args.script_args,
            timeout=args.timeout,
            repo_root=state._repo_root(args),
        ),
    )
