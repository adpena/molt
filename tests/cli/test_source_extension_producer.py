from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import threading
import zipfile
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from molt import python_environment_identity
from molt.python_file_node_custody import VerifiedTreeFile, _semantic_access
from molt.toolchain_identity import stable_regular_file_identity
from molt.exact_json import canonical_json_sha256
from molt.cli import entrypoint_dispatch, entrypoint_parser
from molt.cli import source_build_environment as build_environment
from molt.cli import source_extension_producer as producer
from molt.cli import source_extension_set_validation as set_validation
from molt.cli import source_extension_publication as publication
from molt.cli.source_extension_invocation import SourceExtensionSetInvocation
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.cli.extension_wheel import _write_extension_wheel
from molt.cli.extension_seal import (
    _resolve_declared_artifact,
    _source_package_root_for_manifest,
)
from molt.cli.source_extension_input_custody import (
    project_source_extension_manifest_inputs,
    resolve_source_extension_manifest_input,
    source_extension_input_custody_path,
    stage_source_extension_manifest_inputs,
    validate_source_extension_manifest_input_custody,
)
from molt.cli.source_extension_publication import (
    _source_extension_publication_custody,
)
from molt.cli.source_extension_object_closure import (
    finalize_source_extension_object_closure,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_manifest_codec import (
    _manifest_dependencies,
    _manifest_sequence,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionSet,
    SourceExtensionSource,
    SourceExtensionSpec,
    SourceExtensionVariant,
)
from molt.cli.source_extension_target import (
    SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION,
    resolve_source_extension_target_plan,
)
from molt.cli.source_extension_toolchain import (
    _meson_cross_text,
    _meson_native_text,
    _source_extension_meson_cross_properties,
)
from molt.target_python import TargetPythonVersion
from tests.cli.test_cli_extension_commands import _wasm_exporting_i64_unary_symbol
from tests.python_environment_test_support import (
    build_environment_manifest as _build_environment_manifest,
    lock_closure_manifest as _lock_closure_manifest,
    realized_environment_manifest as _realized_environment_manifest,
    runtime_identity_manifest as _runtime_identity_manifest,
)


_MODULES = (
    "scipy.ndimage._nd_image",
    "scipy.ndimage._ni_label",
    "scipy.ndimage._rank_filter_1d",
    "scipy._lib._ccallback_c",
)


def _tool_image(path: Path) -> VerifiedTreeFile:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"owned build tool")
    path.chmod(0o755)
    return VerifiedTreeFile(
        path=path,
        content=stable_regular_file_identity(path, label="test build tool"),
    )


@contextmanager
def _held_publication_custody(destination: Path):
    lock_path = destination.parent / f".{destination.name}.producer.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message=f"cannot acquire fixture publication lock {lock_path}",
    )
    try:
        yield _source_extension_publication_custody(destination, handle)
    finally:
        _release_file_lock(handle)


def _write_test_extension_wheel(
    wheel: Path,
    *,
    extension_path: str,
    extension_bytes: bytes,
) -> str:
    dist_info = "scipy-1.0.dist-info"
    embedded_manifest = {
        "module": "scipy.ndimage._nd_image",
        "extension": extension_path,
        "wheel": wheel.name,
    }
    return _write_extension_wheel(
        wheel,
        entries=(
            (extension_path, extension_bytes),
            (
                "extension_manifest.json",
                json.dumps(embedded_manifest, sort_keys=True).encode() + b"\n",
            ),
            (f"{dist_info}/WHEEL", b"Wheel-Version: 1.0\n"),
            (f"{dist_info}/METADATA", b"Metadata-Version: 2.1\n"),
        ),
        record_path=f"{dist_info}/RECORD",
    )


def _write_legacy_distribution_metadata(root: Path, name: str, version: str) -> None:
    metadata = root / f"{name.replace('-', '_')}.egg-info" / "PKG-INFO"
    metadata.parent.mkdir(parents=True)
    metadata.write_text(
        f"Metadata-Version: 1.2\nName: {name}\nVersion: {version}\n",
        encoding="utf-8",
    )


def _write_build_pyproject(root: Path, requirements: tuple[str, ...]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    encoded = ", ".join(json.dumps(item) for item in requirements)
    (root / "pyproject.toml").write_text(
        f"[build-system]\nrequires = [{encoded}]\n",
        encoding="utf-8",
    )


def _write_complete_root(root: Path, *, marker: str) -> None:
    root.mkdir(parents=True)
    (root / "marker.txt").write_text(marker, encoding="utf-8")
    for module in _MODULES:
        path = root.joinpath(*module.split(".")).with_suffix(".molt.wasm")
        path.parent.mkdir(parents=True, exist_ok=True)
        root_symbol = f"PyInit_{module.rsplit('.', 1)[-1]}"
        path.write_bytes(_wasm_exporting_i64_unary_symbol(root_symbol))
        artifact_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
        source_path = root.joinpath(
            "provenance", "compiled-inputs", *module.split("."), "source.c"
        )
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text(f"{path.stem}.c", encoding="utf-8")
        source_reference = os.path.relpath(source_path, path.parent).replace(
            os.sep, "/"
        )
        source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
        wheel_sha256 = hashlib.sha256(f"wheel:{module}".encode()).hexdigest()
        object_closure = {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": root_symbol,
            "init_symbol_owner": "0.o",
            "defined_symbols": [root_symbol],
            "undefined_symbols": [],
            "runtime_symbols": [],
            "required_c_api_symbols": [],
            "required_capsules": [],
            "project_generated_c_api_symbols": [],
            "wasm_imports": [],
            "objects": [
                {
                    "source": source_reference,
                    "object": "0.o",
                    "producer_unit": {
                        "target_id": module,
                        "object": f"{module}.so.p/0.o",
                    },
                    "language": "c",
                    "source_sha256": source_sha256,
                    "object_sha256": artifact_sha256,
                    "defined_symbols": [root_symbol],
                    "undefined_symbols": [],
                    "compile_command": ["clang", "-x", "c", "-c", source_reference],
                    "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                    "dependencies": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                    "project_generated_c_api_symbols": [],
                }
            ],
        }
        wheel_path = root.joinpath(
            "provenance", "wheels", *module.split("."), f"{path.stem}.whl"
        )
        wheel_path.parent.mkdir(parents=True, exist_ok=True)
        wheel_path.write_text(f"wheel:{module}", encoding="utf-8")
        manifest = {
            "name": "scipy",
            "version": "1.18.0",
            "module": module,
            "init_symbol": root_symbol,
            "abi_tier": "cpython-abi",
            "target_python": "py312",
            "python_tag": "py3",
            "target_triple": "wasm32-wasip1",
            "artifact_kind": "wasm_relocatable_object",
            "deterministic": True,
            "source_plan": {"target_selector": module.rsplit(".", 1)[-1]},
            "python_exports": [module],
            "capabilities": [],
            "provided_capsules": [],
            "wheel": os.path.relpath(wheel_path, path.parent).replace(os.sep, "/"),
            "link_requirements": {
                "target_triple": "wasm32-wasip1",
                "items": [],
                "retained_symbols": [],
            },
            "extension_sha256": artifact_sha256,
            "wheel_sha256": wheel_sha256,
            "object_closure": object_closure,
            "build": {},
        }
        finalize_source_extension_object_closure(manifest)
        producer._compact_source_extension_manifest(manifest)
        path.with_name(path.name + ".extension_manifest.json").write_text(
            json.dumps(manifest),
            encoding="utf-8",
        )


def _write_target_metadata(root: Path) -> dict[str, object]:
    target_root = root / "provenance/metadata/target"
    python_pc = target_root / "pkgconfig/python3.pc"
    meson_cross = target_root / "meson.cross"
    meson_native = target_root / "meson.native"
    python_pc.parent.mkdir(parents=True, exist_ok=True)
    python_pc.write_text("prefix=@molt\n", encoding="utf-8")
    tool_names = {
        "cc": "clang",
        "cxx": "clang++",
        "wasm_ld": "wasm-ld",
        "ar": "llvm-ar",
        "ranlib": "llvm-ranlib",
        "nm": "llvm-nm",
        "strip": "llvm-strip",
    }
    tools = {
        role: {
            "command": [name],
            "path": name,
            "version": "test",
            "sha256": "a" * 64,
        }
        for role, name in tool_names.items()
    }
    commands = {
        "c": ["clang"],
        "cpp": ["clang++"],
        "ld": ["wasm-ld"],
        "ar": ["llvm-ar"],
        "ranlib": ["llvm-ranlib"],
        "nm": ["llvm-nm"],
        "strip": ["llvm-strip"],
    }
    build_commands = {role: argv for role, argv in commands.items() if role != "ld"}
    target_plan = resolve_source_extension_target_plan("wasm")
    compiler_builtins = "@toolchain/compiler-builtins.a"
    pkg_config_dir = "@target/pkgconfig"
    include_dirs = ["@molt/include"]
    meson_cross.write_bytes(
        _meson_cross_text(
            target_plan=target_plan,
            pkg_config_dir=pkg_config_dir,
            commands={role: tuple(argv) for role, argv in commands.items()},
            compiler_builtins=compiler_builtins,
            include_dirs=tuple(include_dirs),
        ).encode("utf-8"),
    )
    meson_native.write_bytes(
        _meson_native_text(
            commands={role: tuple(argv) for role, argv in build_commands.items()}
        ).encode("utf-8"),
    )
    metadata: dict[str, object] = {
        "schema_version": SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION,
        "kind": "molt-source-extension-target-metadata",
        "target_triple": "wasm32-wasip1",
        "target": {
            "requested": "wasm",
            "compiler_target_triple": "wasm32-wasip1",
            "artifact_kind": "wasm_relocatable_object",
        },
        "python": {"implementation": "cpython", "version": "3.12"},
        "abi": {
            "tier": "cpython-abi",
            "include_dirs": include_dirs,
            "python_header": "@molt/include/Python.h",
            "python_header_sha256": "b" * 64,
            "include_surface": {"sha256": "c" * 64},
        },
        "toolchain": {
            "tools": tools,
            "commands": commands,
            "link_probe_archives": {
                "compiler_builtins": {"path": compiler_builtins, "sha256": "d" * 64}
            },
        },
        "build_toolchain": {
            "target_triple": "x86_64-unknown-linux-gnu",
            "compiler_kind": "host",
            "tools": tools,
            "commands": build_commands,
        },
        "meson_cross_properties": _source_extension_meson_cross_properties(target_plan),
        "paths": {"pkg_config_dir": pkg_config_dir},
        "env": {},
        "digests": {
            "python_pc_sha256": producer._sha256_file(python_pc),
            "meson_cross_sha256": producer._sha256_file(meson_cross),
            "meson_native_sha256": producer._sha256_file(meson_native),
        },
    }
    encoded = json.dumps(metadata, sort_keys=True, separators=(",", ":")).encode()
    metadata["digest"] = hashlib.sha256(encoded).hexdigest()
    (target_root / "source-extension-target-metadata.json").write_text(
        json.dumps(metadata), encoding="utf-8"
    )
    return metadata


def _write_meson_metadata(
    root: Path, extension_set: SourceExtensionSet
) -> dict[str, object]:
    metadata_root = root / "provenance/metadata/meson"
    metadata_root.mkdir(parents=True, exist_ok=True)
    intro_targets = metadata_root / "intro-targets.json"
    compile_commands = metadata_root / "compile-commands.json"
    intro_installed = metadata_root / "intro-installed.json"
    config_tool_cross = metadata_root / "build-config-tools.cross"
    intro_targets.write_text("[]\n", encoding="utf-8")
    compile_commands.write_text("[]\n", encoding="utf-8")
    intro_installed.write_text("{}\n", encoding="utf-8")
    config_tool_cross.write_text("[binaries]\n", encoding="utf-8")
    return {
        "driver": {
            "kind": "build-environment",
            "module": "mesonbuild.mesonmain",
            "distribution": "meson",
            "version": "1.9.0",
        },
        "backend": {
            "distribution": "ninja",
            "version": "1.13.0",
            "reported_version": "1.13.0.git.kitware.jobserver-pipe-1",
            "path": "ninja.exe",
            "sha256": "a" * 64,
        },
        "build_root": "@build",
        "setup_args": list(extension_set.meson_setup_args),
        "intro_targets_sha256": producer._sha256_file(intro_targets),
        "compile_commands_sha256": producer._sha256_file(compile_commands),
        "intro_installed_sha256": producer._sha256_file(intro_installed),
        "config_tool_cross_sha256": producer._sha256_file(config_tool_cross),
        "config_tools": [
            {
                "name": "pkg-config",
                "path": "pkg-config.exe",
                "distribution": "pkgconf",
                "version": "3.0.1.post0",
                "sha256": "b" * 64,
            }
        ],
        "pkg_config_requirement": "pkgconf==3.0.1.post0",
        "generated_inputs": [],
    }


def test_producer_process_environment_preserves_sdk_but_removes_compiler_overrides(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    observed: dict[str, Any] = {}

    def run(argv: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        observed["argv"] = argv
        observed["environment"] = kwargs["env"]
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(producer.process_guard, "run_completed_command", run)
    selected = {
        "CL": "/DUNBOUND",
        "_cl_": "/link unbound.lib",
        "CCC_OVERRIDE_OPTIONS": "-funbound",
        "CC_LD": "other-ld",
        "CXX_LD_FOR_BUILD": "other-build-ld",
        "CC": "owned-clang",
        "CXX": "owned-clang++",
        "INCLUDE": "C:/SDK/include",
        "LIB": "C:/SDK/lib",
    }
    producer._run_process(["meson"], cwd=tmp_path, env=selected)

    assert observed["argv"] == ["meson"]
    assert observed["environment"] == {
        "CC": "owned-clang",
        "CXX": "owned-clang++",
        "INCLUDE": "C:/SDK/include",
        "LIB": "C:/SDK/lib",
    }


@pytest.mark.parametrize("layout", ["scripts", "embedded", "console-with-payload"])
def test_ninja_driver_executes_attested_distribution_command(
    layout: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    payload = _build_environment_manifest()
    filename = "ninja.exe" if os.name == "nt" else "ninja"
    site_root = payload["custody"]["realized_environment"]["site_roots"][0]
    if layout == "console-with-payload":
        _install_realized_tool(
            payload,
            tmp_path,
            "ninja",
            distribution="ninja",
            relative=f"{site_root}/ninja/data/bin/{filename}",
            console_script=False,
        )
    binary = _install_realized_tool(
        payload,
        tmp_path,
        "ninja",
        distribution="ninja",
        relative=f"{site_root}/ninja/data/bin/{filename}"
        if layout == "embedded"
        else None,
        console_script=layout == "console-with-payload",
    )
    calls = []

    def run(argv, *, cwd):
        calls.append((tuple(argv), cwd))
        return subprocess.CompletedProcess(
            argv, 0, stdout="1.13.0.git.kitware.jobserver-pipe-1\n", stderr=""
        )

    monkeypatch.setattr(producer.sys, "prefix", str(tmp_path))
    monkeypatch.setattr(producer, "_run_process", run)
    driver = producer._source_ninja_driver(tmp_path, _source_environment(payload))
    assert calls == [((str(binary), "--version"), tmp_path)]
    assert driver.command == (str(binary),)
    assert driver.manifest_payload() == {
        "distribution": "ninja",
        "version": "1.13.0",
        "reported_version": "1.13.0.git.kitware.jobserver-pipe-1",
        "path": filename,
        "sha256": producer._sha256_file(binary),
    }


@pytest.mark.parametrize("stage", ["version", "command", "manifest"])
def test_ninja_rejects_image_replacement_after_selection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    stage: str,
) -> None:
    payload = _build_environment_manifest()
    binary = _install_realized_tool(
        payload,
        tmp_path,
        "ninja",
        distribution="ninja",
        console_script=False,
    )
    monkeypatch.setattr(producer.sys, "prefix", str(tmp_path))

    def run(argv, *, cwd):
        if stage == "version":
            binary.write_bytes(b"replaced image")
        return subprocess.CompletedProcess(argv, 0, stdout="1.13.0\n", stderr="")

    monkeypatch.setattr(producer, "_run_process", run)
    with pytest.raises(ValueError, match="changed"):
        driver = producer._source_ninja_driver(tmp_path, _source_environment(payload))
        binary.write_bytes(b"replaced image")
        if stage == "command":
            _ = driver.command
        else:
            driver.manifest_payload()


def _source_environment(
    payload: dict[str, Any] | None = None,
) -> producer._SourceBuildEnvironment:
    payload = _build_environment_manifest() if payload is None else payload
    return producer._SourceBuildEnvironment(
        python_executable=sys.executable,
        requirements=tuple(payload["requirements"]),
        marker_environment=payload["marker_environment"],
        active_requirements=tuple(payload["active_requirements"]),
        resolved=tuple(
            producer._ResolvedBuildRequirement(**row) for row in payload["resolved"]
        ),
        custody=payload["custody"],
        inventory=producer.SourceBuildInventory(payload["custody"], Path(sys.prefix)),
    )


def _install_realized_tool(
    payload: dict[str, Any],
    root: Path,
    name: str,
    *,
    distribution: str,
    relative: str | None = None,
    console_script: bool = True,
) -> Path:
    """Add actual tool bytes and ownership to the shared synthetic environment."""
    realized = payload["custody"]["realized_environment"]
    filename = name + ".exe" if realized["operating_system"] == "windows" else name
    relative = relative or f"{realized['scripts_root']}/{filename}"
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    data = b"realized-tool-fixture"
    path.write_bytes(data)
    path.chmod(0o755)
    tree = realized["tree"]
    known_paths = {entry["path"] for entry in tree["entries"]}
    for parent in path.parents:
        if parent == root:
            break
        parent_relative = parent.relative_to(root).as_posix()
        if parent_relative not in known_paths:
            tree["entries"].append(
                {
                    "path": parent_relative,
                    "kind": "directory",
                    "access": _semantic_access(parent.lstat()),
                }
            )
    for entry in tree["entries"]:
        directory = root / entry["path"]
        if entry["kind"] == "directory" and path.is_relative_to(directory):
            entry["access"] = _semantic_access(directory.lstat())
    node = f"file-node-{len(tree['file_nodes'])}"
    tree["file_nodes"].append(
        {"id": node, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    )
    tree["node_ids"].append(node)
    tree["file_count"] += 1
    tree["entries"].append(
        {
            "path": relative,
            "kind": "file",
            "node": node,
            "access": _semantic_access(path.lstat()),
        }
    )
    tree["entries"].sort(key=lambda row: (row["path"].casefold(), row["path"]))
    tree["manifest_sha256"] = canonical_json_sha256(tree["entries"])
    owner = next(
        row for row in realized["distributions"] if row["name"] == distribution
    )
    owner["installed_files"].append({"path": relative, "node": node, "declared": None})
    owner["installed_files"].sort(key=lambda row: (row["path"].casefold(), row["path"]))
    owner["installed_file_count"] += 1
    owner["file_manifest_sha256"] = canonical_json_sha256(owner["installed_files"])
    if console_script:
        owner["entry_points"].append(
            {"group": "console_scripts", "name": name, "value": f"{distribution}:main"}
        )
        owner["entry_points"].sort(
            key=lambda row: (row["group"], row["name"], row["value"])
        )
        owner["entry_points_sha256"] = canonical_json_sha256(owner["entry_points"])
        owner["console_scripts"][name] = [relative]
        realized["console_scripts"][name] = [relative]
    realized["distribution_inventory_sha256"] = canonical_json_sha256(
        realized["distributions"]
    )
    realized["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in realized.items()
            if key != "environment_closure_sha256"
        }
    )
    python_environment_identity.validate_python_environment_identity(realized)
    return path.resolve()


@pytest.mark.parametrize(
    "defect", ["missing", "ambiguous", "unowned", "tampered", "removed"]
)
def test_inventory_rejects_unattested_executable(
    tmp_path: Path,
    defect: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = _build_environment_manifest()
    filename = "ninja.exe" if os.name == "nt" else "ninja"
    site_root = payload["custody"]["realized_environment"]["site_roots"][0]
    if defect != "missing":
        binary = _install_realized_tool(
            payload,
            tmp_path,
            "ninja",
            distribution="meson" if defect == "unowned" else "ninja",
            console_script=False,
        )
        if defect == "ambiguous":
            _install_realized_tool(
                payload,
                tmp_path,
                "ninja",
                distribution="ninja",
                relative=f"{site_root}/ninja/{filename}",
                console_script=False,
            )
        elif defect == "tampered":
            binary.write_bytes(b"unattested replacement")
        elif defect == "removed":
            binary.unlink()
    monkeypatch.setattr(producer.sys, "prefix", str(tmp_path))
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_a, **_kw: pytest.fail("unattested tool must not execute"),
    )
    message = {
        "missing": "0 executable candidates",
        "ambiguous": "2 executable candidates",
        "unowned": "0 executable candidates",
        "tampered": "content differs",
        "removed": None,
    }[defect]
    with pytest.raises((ValueError, FileNotFoundError), match=message):
        producer._source_ninja_driver(tmp_path, _source_environment(payload))


def test_produce_set_parser_has_no_partial_or_nondeterministic_lane() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "extension",
            "produce-set",
            "--package",
            "scipy",
            "--package-version",
            "1.18.0",
            "--module-set",
            "pact-witness",
            "--python-version",
            "3.12",
            "--source",
            "repos/scipy",
            "--build-root",
            "build/scipy-wasm",
            "--json",
        ]
    )

    assert args.extension_command == "produce-set"
    assert args.package == "scipy"
    assert args.package_version == "1.18.0"
    assert args.module_set == "pact-witness"
    assert args.python_version == "3.12"
    assert args.target == "wasm"
    assert args.abi_tier == "cpython-abi"
    assert args.expected_identity_sha256 is None
    assert args.expected_candidate_identity_sha256 is None
    assert not hasattr(args, "module")
    assert not hasattr(args, "deterministic")

    native = parser.parse_args(
        [
            "extension",
            "produce-set",
            "--package",
            "scipy",
            "--package-version",
            "1.18.0",
            "--module-set",
            "pact-witness",
            "--python-version",
            "3.12",
            "--source",
            "repos/scipy",
            "--build-root",
            "build/scipy-native",
            "--target",
            "aarch64-apple-darwin",
        ]
    )
    assert native.target == "aarch64-apple-darwin"


@pytest.mark.parametrize(
    "missing", ["--package-version", "--python-version", "--source", "--build-root"]
)
def test_produce_set_rejects_implicit_source_or_environment(
    missing: str, capsys: pytest.CaptureFixture[str]
) -> None:
    selectors = {
        "--package": "scipy",
        "--package-version": "1.18.0",
        "--module-set": "pact-witness",
        "--python-version": "3.12",
        "--source": "repos/scipy",
        "--build-root": "build/scipy-wasm",
    }
    argv = ["extension", "produce-set"]
    for option, value in selectors.items():
        if option != missing:
            argv.extend((option, value))
    with pytest.raises(SystemExit) as error:
        entrypoint_parser._build_entrypoint_parser().parse_args(argv)
    assert error.value.code == 2
    assert missing in capsys.readouterr().err


def test_produce_set_dispatches_complete_set(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        entrypoint_dispatch,
        "produce_source_extension_set",
        lambda **kwargs: calls.append(kwargs) or 0,
    )
    args = entrypoint_parser._build_entrypoint_parser().parse_args(
        [
            "extension",
            "produce-set",
            "--package",
            "scipy",
            "--package-version",
            "1.18.0",
            "--module-set",
            "pact-witness",
            "--python-version",
            "3.12",
            "--source",
            "repos/scipy",
            "--build-root",
            "build/scipy-wasm",
        ]
    )

    assert (
        entrypoint_dispatch._dispatch_entrypoint_command(
            args,
            build_fn=lambda **_: 0,
            config_root=tmp_path,
            config={},
            build_cfg={},
            run_cfg={},
            compare_cfg={},
            test_cfg={},
            diff_cfg={},
            extension_cfg={},
            publish_cfg={},
            cfg_capabilities=None,
        )
        == 0
    )
    assert calls == [
        {
            "package": "scipy",
            "package_version": "1.18.0",
            "module_set": "pact-witness",
            "python_version": "3.12",
            "source": "repos/scipy",
            "build_root": "build/scipy-wasm",
            "target": "wasm",
            "abi_tier": "cpython-abi",
            "expected_identity_sha256": None,
            "expected_candidate_identity_sha256": None,
            "json_output": False,
            "prepared": False,
        }
    ]


def test_meson_installed_python_is_complete_package_authority(tmp_path: Path) -> None:
    source = tmp_path / "source"
    build = tmp_path / "build"
    publish = tmp_path / "publish"
    installed: dict[str, str] = {}
    for relative in (
        "scipy/__init__.py",
        "scipy/version.py",
        "scipy/__config__.py",
        "scipy/ndimage/_measurements.py",
    ):
        path = source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        content = (
            f'ROOT = r"{source}"\n'
            if relative == "scipy/__config__.py"
            else f"# {relative}\n"
        )
        path.write_text(content, encoding="utf-8")
        installed[str(path)] = f"C:/prefix/Lib/site-packages/{relative}"
    unrelated = source / "array_api_extra/tests/__init__.py"
    unrelated.parent.mkdir(parents=True)
    unrelated.write_text("# unrelated subproject\n", encoding="utf-8")
    installed[str(unrelated)] = (
        "C:/prefix/Lib/site-packages/array_api_extra/tests/__init__.py"
    )
    generated_header = build / "scipy/_lib/include/scipy/generated_api.h"
    generated_header.parent.mkdir(parents=True)
    generated_header.write_text(f'#define BUILD_ROOT "{build}"\n', encoding="utf-8")
    installed[str(generated_header)] = (
        "C:/prefix/Lib/site-packages/scipy/_lib/include/scipy/generated_api.h"
    )
    intro = build / "meson-info" / "intro-installed.json"
    intro.parent.mkdir(parents=True)
    intro.write_text(json.dumps(installed), encoding="utf-8")

    staged = producer._stage_installed_package_files(
        intro_installed=intro,
        source_root=source,
        build_root=build,
        package="scipy",
        publish_root=publish,
        location_roots=((source, "@source"), (build, "@build")),
        required_installed_files=(
            "scipy/__config__.py",
            "scipy/__init__.py",
            "scipy/version.py",
        ),
    )

    assert len(staged) == 5
    assert (publish / "scipy/version.py").read_text(encoding="utf-8") == (
        "# scipy/version.py\n"
    )
    assert (publish / "scipy/__config__.py").read_text(encoding="utf-8") == (
        'ROOT = r"@source"\n'
    )
    assert (publish / "scipy/_lib/include/scipy/generated_api.h").read_text(
        encoding="utf-8"
    ) == '#define BUILD_ROOT "@build"\n'
    assert not (publish / "array_api_extra").exists()


def test_meson_installed_directory_is_recursively_materialized(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    build = tmp_path / "build"
    publish = tmp_path / "publish"
    package_dir = source / "numpy/_utils"
    (package_dir / "nested").mkdir(parents=True)
    (package_dir / "__init__.py").write_text("\n", encoding="utf-8")
    (package_dir / "_inspect.py").write_text(
        'ROOT = r"' + str(source) + '"\n', encoding="utf-8"
    )
    (package_dir / "py.typed").write_text("\n", encoding="utf-8")
    (package_dir / "nested/data.bin").write_bytes(b"\x00package-data\xff")
    (package_dir / "unmanaged.cp312-win_amd64.pyd").write_bytes(b"native")
    intro = build / "meson-info" / "intro-installed.json"
    intro.parent.mkdir(parents=True)
    intro.write_text(
        json.dumps(
            {
                str(package_dir): ("C:/prefix/Lib/site-packages/numpy/_utils"),
                str(build / "numpy/_core/unmanaged.cp312-win_amd64.pyd"): (
                    "C:/prefix/Lib/site-packages/numpy/_core/"
                    "unmanaged.cp312-win_amd64.pyd"
                ),
            }
        ),
        encoding="utf-8",
    )

    staged = producer._stage_installed_package_files(
        intro_installed=intro,
        source_root=source,
        build_root=build,
        package="numpy",
        publish_root=publish,
        location_roots=((source, "@source"), (build, "@build")),
        required_installed_files=(
            "numpy/_utils/__init__.py",
            "numpy/_utils/_inspect.py",
        ),
    )

    relative = {path.relative_to(publish).as_posix() for path in staged}
    assert relative == {
        "numpy/_utils/__init__.py",
        "numpy/_utils/_inspect.py",
        "numpy/_utils/py.typed",
    }
    assert (publish / "numpy/_utils/_inspect.py").read_text(encoding="utf-8") == (
        'ROOT = r"@source"\n'
    )
    assert not (publish / "numpy/_utils/nested/data.bin").exists()
    assert not (publish / "numpy/_utils/unmanaged.cp312-win_amd64.pyd").exists()


def test_meson_installed_directory_preserves_leaf_collision_authority(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    build = tmp_path / "build"
    publish = tmp_path / "publish"
    package_dir = source / "numpy/_utils"
    package_dir.mkdir(parents=True)
    (package_dir / "_inspect.py").write_text("DIRECTORY = True\n", encoding="utf-8")
    conflicting = source / "conflicting.py"
    conflicting.write_text("DIRECTORY = False\n", encoding="utf-8")
    intro = build / "meson-info" / "intro-installed.json"
    intro.parent.mkdir(parents=True)
    intro.write_text(
        json.dumps(
            {
                str(package_dir): "C:/prefix/Lib/site-packages/numpy/_utils",
                str(conflicting): (
                    "C:/prefix/Lib/site-packages/numpy/_utils/_inspect.py"
                ),
            }
        ),
        encoding="utf-8",
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="different package files to the same path",
    ):
        producer._stage_installed_package_files(
            intro_installed=intro,
            source_root=source,
            build_root=build,
            package="numpy",
            publish_root=publish,
            location_roots=((source, "@source"), (build, "@build")),
            required_installed_files=(),
        )


def test_meson_installed_python_rejects_old_handwritten_config_gap(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    build = tmp_path / "build"
    publish = tmp_path / "publish"
    installed: dict[str, str] = {}
    for relative in ("scipy/__init__.py", "scipy/version.py"):
        path = source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n", encoding="utf-8")
        installed[str(path)] = f"/prefix/lib/python3.12/site-packages/{relative}"
    intro = build / "meson-info" / "intro-installed.json"
    intro.parent.mkdir(parents=True)
    intro.write_text(json.dumps(installed), encoding="utf-8")

    with pytest.raises(
        producer.SourceExtensionProducerError, match=r"scipy/__config__\.py"
    ):
        producer._stage_installed_package_files(
            intro_installed=intro,
            source_root=source,
            build_root=build,
            package="scipy",
            publish_root=publish,
            location_roots=((source, "@source"), (build, "@build")),
            required_installed_files=(
                "scipy/__config__.py",
                "scipy/__init__.py",
                "scipy/version.py",
            ),
        )


def test_build_extension_routes_real_meson_authority_deterministically(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    build = tmp_path / "meson-build"
    intro = build / "meson-info" / "intro-targets.json"
    compile_commands = build / "compile_commands.json"
    output = tmp_path / "transaction" / "module"
    calls: list[dict[str, object]] = []
    expected = object()
    backend = producer._SourceNinjaDriver(
        image=_tool_image(tmp_path / "ninja"),
        manifest={"distribution": "ninja"},
    )
    monkeypatch.setattr(
        producer.extension_commands,
        "extension_build",
        lambda **kwargs: calls.append(kwargs) or 0,
    )
    monkeypatch.setattr(
        producer,
        "_audit_extension_output",
        lambda **_kwargs: expected,
    )

    actual = producer._build_extension(
        source_root=source,
        build_root=build,
        intro_targets=intro,
        compile_commands=compile_commands,
        output_root=output,
        module="scipy.ndimage._nd_image",
        target_name="_nd_image",
        python_exports=("scipy",),
        capabilities=(),
        provided_capsules=(),
        exclude_linked_static_libraries=(),
        target="wasm",
        target_python=TargetPythonVersion(3, 12, 0),
        package_version="1.18.0",
        abi_tier="cpython-abi",
        tool_commands={"ld": ("/tools/wasm-ld",)},
        backend=backend,
    )

    assert actual is expected
    assert len(calls) == 1
    assert calls[0]["deterministic"] is True
    assert calls[0]["python_version"] == "3.12"
    assert calls[0]["source_plan"] == str(intro)
    assert calls[0]["source_plan_build_root"] == str(build)
    assert calls[0]["source_plan_compile_commands"] == str(compile_commands)
    assert calls[0]["source_plan_target"] == "_nd_image"
    assert calls[0]["python_export"] == ["scipy"]
    assert calls[0]["capabilities"] == []
    assert calls[0]["tool_commands"] == {"ld": ("/tools/wasm-ld",)}
    assert calls[0]["source_plan_ninja_command"] == backend.command


def test_transactional_wheel_bytes_must_match_manifest(
    tmp_path: Path,
) -> None:
    wheel = tmp_path / "scipy.whl"
    wheel.write_bytes(b"audited-wheel")
    manifest = {
        "wheel": wheel.name,
        "wheel_sha256": hashlib.sha256(wheel.read_bytes()).hexdigest(),
    }

    assert (
        producer._audit_declared_wheel(
            manifest, output_root=tmp_path, module="scipy.ndimage._nd_image"
        )
        == hashlib.sha256(wheel.read_bytes()).hexdigest()
    )
    manifest["wheel_sha256"] = "0" * 64
    with pytest.raises(
        producer.SourceExtensionProducerError, match="wheel checksum mismatch"
    ):
        producer._audit_declared_wheel(
            manifest, output_root=tmp_path, module="scipy.ndimage._nd_image"
        )


def test_producer_audit_enforces_exact_consumer_contract() -> None:
    current_abi = producer._default_molt_c_api_version(
        Path(__file__).resolve().parents[2]
    )
    manifest = {
        "deterministic": True,
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "target_triple": "wasm32-wasip1",
        "target_python": "py312",
        "python_tag": "py3",
        "version": "1.18.0",
        "abi_tier": "cpython-abi",
        "molt_c_api_version": current_abi,
        "abi_tag": f"molt_abi{current_abi.split('.', 1)[0]}",
    }

    producer._audit_producer_contract(
        manifest,
        module="scipy.ndimage._nd_image",
        expected_target_triple="wasm32-wasip1",
        expected_target_python=TargetPythonVersion(3, 12, 0),
        expected_package_version="1.18.0",
    )
    manifest["deterministic"] = False
    with pytest.raises(producer.SourceExtensionProducerError, match="deterministic"):
        producer._audit_producer_contract(
            manifest,
            module="scipy.ndimage._nd_image",
            expected_target_triple="wasm32-wasip1",
            expected_target_python=TargetPythonVersion(3, 12, 0),
            expected_package_version="1.18.0",
        )


def test_source_build_environment_noop_records_exact_resolutions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    requirements = ("meson>=1.5", "Cython>=3.0", 'ninja; python_version < "3"')
    _write_build_pyproject(tmp_path, requirements)
    custody = _build_environment_manifest(["cython==3.1.2", "meson==1.8.0"])["custody"]
    poison = tmp_path / "ambient"
    _write_legacy_distribution_metadata(poison, "meson", "0.1")
    monkeypatch.syspath_prepend(str(poison))
    monkeypatch.setenv("PYTHONPATH", str(poison))
    inventories = []
    inventory_type = producer.SourceBuildInventory

    def capture_inventory(*args, **kwargs):
        inventory = inventory_type(*args, **kwargs)
        inventories.append(inventory)
        return inventory

    monkeypatch.setattr(producer, "SourceBuildInventory", capture_inventory)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail(
            "satisfied requirements must not install"
        ),
    )

    environment = producer._ensure_source_build_environment(tmp_path, custody=custody)
    assert len(inventories) == 1
    assert environment.inventory is inventories[0]
    producer._source_build_config_tools(environment)
    assert len(inventories) == 1

    assert environment.manifest_payload() == {
        "python": {
            "implementation": producer.sys.implementation.name,
            "version": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
            "executable": Path(sys.executable).name,
        },
        "requirements": list(requirements),
        "marker_environment": producer.canonical_source_marker_environment(),
        "active_requirements": ["meson>=1.5", "Cython>=3.0"],
        "resolved": [
            {"requirement": "meson>=1.5", "distribution": "meson", "version": "1.8.0"},
            {
                "requirement": "Cython>=3.0",
                "distribution": "cython",
                "version": "3.1.2",
            },
        ],
        "custody": custody,
    }


def test_source_build_environment_rejects_incomplete_locked_group(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5", "Cython>=3.0"))
    custody = _build_environment_manifest(["cython==3.1.2", "meson==1.0"])["custody"]
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="configured dependency group or frozen lock is incomplete.*meson>=1.5",
    ):
        producer._ensure_source_build_environment(tmp_path, custody=custody)


def test_source_build_environment_missing_distribution_is_fail_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5",))
    custody = _build_environment_manifest(["ninja==1.13.0"])["custody"]
    # An ambient matching distribution cannot fill a gap in the realized closure.
    poison = tmp_path / "ambient"
    _write_legacy_distribution_metadata(poison, "meson", "99")
    monkeypatch.syspath_prepend(str(poison))
    monkeypatch.setenv("PYTHONPATH", str(poison))
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="configured dependency group or frozen lock is incomplete.*meson>=1.5",
    ):
        producer._ensure_source_build_environment(tmp_path, custody=custody)


def _locked_environment_spec(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[Path, Path, Path, dict[str, object], Path]:
    root = tmp_path / "custody/environment"
    monkeypatch.setattr(
        build_environment, "_source_build_custody_root", lambda _repo: root.parent
    )
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    payload = _build_environment_manifest(
        ["ninja==1.13.0"], dependency_group="source-build-numpy"
    )
    custody = {
        key: value
        for key, value in payload["custody"].items()
        if key != "realized_environment"
    }
    return (
        root,
        python,
        root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
        custody,
        tmp_path / "uv.exe",
    )


def test_source_build_environment_rejects_resealed_float_schema_version() -> None:
    payload = _build_environment_manifest(["ninja==1.13.0"])
    custody = payload["custody"]
    custody["schema_version"] = 5.0
    custody["environment_id"] = canonical_json_sha256(
        {
            key: value
            for key, value in custody.items()
            if key not in {"environment_id", "realized_environment"}
        }
    )
    assert build_environment.source_build_environment_problems(payload) == [
        "extension-set manifest build-environment address digest is invalid"
    ]


def test_source_build_environment_address_is_worktree_neutral(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = tmp_path / "worktrees/first"
    second = tmp_path / "worktrees/second"
    for root in (first, second):
        root.mkdir(parents=True)
        (root / "pyproject.toml").write_text(
            '[dependency-groups]\nsource-build-numpy = ["ninja==1.13.0"]\n',
            encoding="utf-8",
        )
        (root / "uv.lock").write_bytes(b"same complete lock")
    monkeypatch.setattr(
        build_environment,
        "checkout_custody",
        lambda _root, _env: SimpleNamespace(custody_root=tmp_path),
    )
    monkeypatch.setattr(
        build_environment,
        "_python_identity",
        _runtime_identity_manifest,
    )
    lock_closure = _lock_closure_manifest(
        ["ninja==1.13.0"],
        [("ninja", "1.13.0")],
        dependency_group="source-build-numpy",
    )
    monkeypatch.setattr(
        build_environment,
        "selected_uv_lock_group_closure",
        lambda *_args, **_kwargs: lock_closure,
    )
    monkeypatch.setattr(
        build_environment,
        "_uv_identity",
        lambda: (
            tmp_path / "uv.exe",
            {
                "executable": "uv.exe",
                "version": "uv 0.11.24",
                "sha256": "d" * 64,
            },
        ),
    )

    first_spec = build_environment._environment_spec(first, "source-build-numpy")
    second_spec = build_environment._environment_spec(second, "source-build-numpy")

    assert first_spec[:4] == second_spec[:4]
    assert "worktrees" not in str(first_spec[0])
    custody = first_spec[3]
    assert custody["schema_version"] == 5
    address_payload = {key: custody[key] for key in custody if key != "environment_id"}
    assert custody["environment_id"] == canonical_json_sha256(address_payload)
    old_address_payload = dict(address_payload)
    old_address_payload["schema_version"] = 3
    assert custody["environment_id"] != canonical_json_sha256(old_address_payload)


def test_source_build_environment_failed_sync_leaves_only_provisional_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 7),
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="provisioning failed.*returned 7",
    ):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )

    assert not spec[2].exists()
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    assert json.loads(provisioning_path.read_text(encoding="utf-8")) == (
        build_environment._provisioning_record(spec[3])
    )


def test_source_build_environment_recovers_exact_provisional_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    monkeypatch.setenv("VIRTUAL_ENV", str(tmp_path / "unrelated-launcher"))
    calls = 0
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    realized = _realized_environment_manifest(
        spec[3]["python_runtime"], [("ninja", "1.13.0")]
    )
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_identity",
        lambda _python, _root: realized,
    )

    def run(argv, **kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            return subprocess.CompletedProcess(argv, 7)
        environment_root = Path(kwargs["environment"]["UV_PROJECT_ENVIRONMENT"])
        assert "VIRTUAL_ENV" not in kwargs["environment"]
        assert environment_root == spec[0]
        environment_python = environment_root / (
            "Scripts/python.exe" if os.name == "nt" else "bin/python"
        )
        environment_python.parent.mkdir(parents=True, exist_ok=True)
        environment_python.write_bytes(b"python")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(build_environment, "_run_uv_sync", run)

    with pytest.raises(build_environment.SourceBuildEnvironmentError):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )
    result = build_environment.source_build_environment(
        tmp_path, "source-build-numpy", provision=True
    )

    assert calls == 2
    assert result.root == spec[0]
    assert json.loads(spec[2].read_text(encoding="utf-8")) == {
        **spec[3],
        "realized_environment": realized,
    }
    assert not build_environment._provisioning_record_path(spec[0]).exists()


@pytest.mark.parametrize(
    "foreign_payload",
    [None, {}, {"state": "provisioning"}, {"state": "provisioning", "custody": {}}],
)
def test_source_build_environment_rejects_foreign_unattested_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    foreign_payload: object,
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    spec[0].mkdir(parents=True)
    if foreign_payload is not None:
        spec[2].write_text(json.dumps(foreign_payload), encoding="utf-8")
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: pytest.fail("foreign root must never be mutated"),
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="exact attestation or sibling provisioning record",
    ):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )


@pytest.mark.parametrize(
    "foreign_payload",
    [{}, {"state": "provisioning"}, {"state": "provisioning", "custody": {}}],
)
def test_source_build_environment_rejects_foreign_sibling_record(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    foreign_payload: object,
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text(json.dumps(foreign_payload), encoding="utf-8")
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="foreign source-build provisioning record",
    ):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )


def test_source_build_environment_rejects_malformed_sibling_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text("{not-json", encoding="utf-8")
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="malformed source-build provisioning record",
    ):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )


def test_distribution_probe_sanitizes_python_import_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}
    monkeypatch.setenv("PYTHONHOME", "poison-home")
    monkeypatch.setenv("PYTHONPATH", "poison-path")
    realized = _realized_environment_manifest(
        _runtime_identity_manifest(), [("ninja", "1.13.0")]
    )

    def run(argv, **kwargs):
        observed["argv"] = argv
        observed["env"] = kwargs["env"]
        return subprocess.CompletedProcess(
            argv, 0, stdout=json.dumps(realized), stderr=""
        )

    monkeypatch.setattr(
        build_environment.process_guard,
        "run_completed_command",
        run,
    )

    assert (
        build_environment._probe_environment_identity(
            Path(sys.executable), Path(sys.prefix)
        )
        == realized
    )
    expected_argv = [
        str(Path(sys.executable)),
        "-B",
        "-I",
        str(Path(python_environment_identity.__file__).resolve()),
        "--capture-environment",
        str(Path(sys.prefix).resolve()),
        "--admit-virtualenv-bootstrap",
    ]
    assert observed["argv"] == expected_argv
    environment = observed["env"]
    assert isinstance(environment, dict)
    assert "PYTHONHOME" not in environment
    assert "PYTHONPATH" not in environment
    assert environment["PYTHONNOUSERSITE"] == "1"
    # The exact argv above owns bytecode policy: isolated -I ignores PYTHON*.


def test_complete_environment_cleans_exact_stale_sibling_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    realized = _realized_environment_manifest(
        spec[3]["python_runtime"], [("ninja", "1.13.0")]
    )
    spec[1].parent.mkdir(parents=True)
    spec[1].write_bytes(b"python")
    spec[2].write_text(
        json.dumps({**spec[3], "realized_environment": realized}),
        encoding="utf-8",
    )
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text(
        json.dumps(build_environment._provisioning_record(spec[3])),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_identity",
        lambda _python, _root: realized,
    )
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: pytest.fail("complete root must not reprovision"),
    )

    result = build_environment.source_build_environment(
        tmp_path, "source-build-numpy", provision=True
    )

    assert result.root == spec[0]
    assert not provisioning_path.exists()


def test_concurrent_source_build_provision_runs_one_sync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    calls: list[tuple[str, ...]] = []
    calls_lock = threading.Lock()
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    realized = _realized_environment_manifest(
        spec[3]["python_runtime"], [("ninja", "1.13.0")]
    )
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_identity",
        lambda _python, _root: realized,
    )

    def run(argv, **kwargs):
        with calls_lock:
            calls.append(tuple(argv))
        environment_root = Path(kwargs["environment"]["UV_PROJECT_ENVIRONMENT"])
        assert environment_root == spec[0]
        environment_python = environment_root / (
            "Scripts/python.exe" if os.name == "nt" else "bin/python"
        )
        environment_python.parent.mkdir(parents=True, exist_ok=True)
        environment_python.write_bytes(b"python")
        launcher = environment_python.parent / (
            "cython.exe" if os.name == "nt" else "cython"
        )
        launcher.write_text(f"#!{environment_python}\n", encoding="utf-8")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(build_environment, "_run_uv_sync", run)
    errors: list[BaseException] = []

    def provision() -> None:
        try:
            build_environment.source_build_environment(
                tmp_path, "source-build-numpy", provision=True
            )
        except BaseException as exc:  # pragma: no cover - asserted below
            errors.append(exc)

    threads = [threading.Thread(target=provision) for _index in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert not errors
    assert len(calls) == 1
    assert calls[0] == (
        str(spec[4]),
        "sync",
        "--project",
        str(tmp_path.resolve()),
        "--python",
        str(Path(getattr(sys, "_base_executable", None) or sys.executable).resolve()),
        "--frozen",
        "--no-default-groups",
        "--no-dev",
        "--group",
        "source-build-numpy",
        "--no-install-project",
        "--compile-bytecode",
    )
    assert json.loads(spec[2].read_text(encoding="utf-8")) == {
        **spec[3],
        "realized_environment": realized,
    }
    launcher = spec[1].parent / ("cython.exe" if os.name == "nt" else "cython")
    assert launcher.read_text(encoding="utf-8") == f"#!{spec[1]}\n"
    assert ".provision-" not in launcher.read_text(encoding="utf-8")


def test_source_build_provision_rejects_group_resolution_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_identity",
        lambda _python, _root: _realized_environment_manifest(
            spec[3]["python_runtime"], [("packaging", "26.2")]
        ),
    )

    def run(argv, **kwargs):
        environment_root = Path(kwargs["environment"]["UV_PROJECT_ENVIRONMENT"])
        environment_python = environment_root / (
            "Scripts/python.exe" if os.name == "nt" else "bin/python"
        )
        environment_python.parent.mkdir(parents=True, exist_ok=True)
        environment_python.write_bytes(b"python")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(build_environment, "_run_uv_sync", run)

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="differs from its selected lock",
    ):
        build_environment.source_build_environment(
            tmp_path, "source-build-numpy", provision=True
        )

    assert not spec[2].exists()
    assert json.loads(
        build_environment._provisioning_record_path(spec[0]).read_text(encoding="utf-8")
    ) == (build_environment._provisioning_record(spec[3]))


def test_active_source_build_environment_rejects_mutated_ambient_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    spec[0].mkdir(parents=True)
    realized = _realized_environment_manifest(
        spec[3]["python_runtime"], [("ninja", "1.13.0")]
    )
    manifest = {**spec[3], "realized_environment": realized}
    spec[2].write_text(json.dumps(manifest), encoding="utf-8")
    monkeypatch.setattr(
        build_environment, "_environment_spec", lambda *_args, **_kwargs: spec
    )
    monkeypatch.setattr(build_environment.sys, "prefix", str(spec[0]))
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_identity",
        lambda _python, _root: {
            **realized,
            "environment_closure_sha256": "9" * 64,
        },
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="attestation is stale or invalid",
    ):
        build_environment.source_build_environment(tmp_path, "source-build-numpy")


@pytest.fixture
def active_source_namespace_case(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[dict[str, Any], Path, list[tuple[Path, Path]]]:
    """Exercise real address selection with only external identity probes mocked."""
    payload = _build_environment_manifest(
        ["ninja==1.13.0"], dependency_group="source-build-numpy"
    )
    custody = payload["custody"]
    realized = custody["realized_environment"]
    namespace = tmp_path / "build-environments/source-extension"
    active_root = namespace / custody["environment_id"]
    active_root.mkdir(parents=True)
    executable = active_root / realized["selected_executable"]["path"]
    executable.parent.mkdir(parents=True, exist_ok=True)
    executable.write_bytes(b"synthetic-python")
    (active_root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
        json.dumps(custody), encoding="utf-8"
    )
    probes: list[tuple[Path, Path]] = []

    def capture(python: Path, root: Path) -> dict[str, object]:
        probes.append((python, root))
        return realized

    monkeypatch.setattr(build_environment.sys, "prefix", str(active_root))
    monkeypatch.setattr(build_environment.sys, "executable", str(executable))
    monkeypatch.setattr(
        build_environment, "_source_build_custody_root", lambda _repo: namespace
    )
    monkeypatch.setattr(
        build_environment,
        "_declared_dependency_group",
        lambda *_args: tuple(custody["dependency_group_requirements"]),
    )
    monkeypatch.setattr(
        build_environment,
        "selected_uv_lock_group_closure",
        lambda *_args, **_kwargs: custody["lock_closure"],
    )
    monkeypatch.setattr(
        build_environment, "_uv_identity", lambda: (tmp_path / "uv.exe", custody["uv"])
    )
    monkeypatch.setattr(build_environment, "_probe_environment_identity", capture)
    monkeypatch.setattr(
        build_environment,
        "_python_identity",
        lambda: pytest.fail(
            "active realized capture must be the sole runtime identity source"
        ),
    )
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: pytest.fail("active lookup must not provision"),
    )
    return payload, active_root, probes


@pytest.mark.parametrize("provision", [False, True])
def test_active_namespace_lookup_captures_once_and_derives_one_recipe(
    tmp_path: Path,
    active_source_namespace_case: tuple[dict[str, Any], Path, list[tuple[Path, Path]]],
    provision: bool,
) -> None:
    payload, active_root, probes = active_source_namespace_case
    result = build_environment.source_build_environment(
        tmp_path, "source-build-numpy", provision=provision
    )
    assert result.active is True
    assert result.root == active_root
    assert result.custody == payload["custody"]
    assert probes == [(Path(sys.executable), active_root)]


@pytest.mark.parametrize("corruption", ["missing", "recipe", "realized"])
def test_active_namespace_single_capture_rejects_manifest_drift(
    tmp_path: Path,
    active_source_namespace_case: tuple[dict[str, Any], Path, list[tuple[Path, Path]]],
    corruption: str,
) -> None:
    payload, active_root, probes = active_source_namespace_case
    path = active_root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST
    if corruption == "missing":
        path.unlink()
    else:
        persisted = json.loads(json.dumps(payload["custody"]))
        if corruption == "recipe":
            persisted["environment_id"] = "0" * 64
        else:
            persisted["realized_environment"]["environment_closure_sha256"] = "0" * 64
        path.write_text(json.dumps(persisted), encoding="utf-8")
    with pytest.raises(build_environment.SourceBuildEnvironmentError):
        build_environment.source_build_environment(tmp_path, "source-build-numpy")
    assert probes == (
        [] if corruption == "missing" else [(Path(sys.executable), active_root)]
    )


@pytest.mark.parametrize("drift", ["recipe", "path"])
def test_active_namespace_rejects_same_group_recipe_or_path_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    active_source_namespace_case: tuple[dict[str, Any], Path, list[tuple[Path, Path]]],
    drift: str,
) -> None:
    payload, active_root, probes = active_source_namespace_case
    if drift == "recipe":
        changed_uv = {**payload["custody"]["uv"], "sha256": "f" * 64}
        monkeypatch.setattr(
            build_environment, "_uv_identity", lambda: (tmp_path / "uv.exe", changed_uv)
        )
    else:
        wrong_root = active_root.parent / ("0" * 64)
        wrong_root.mkdir()
        (wrong_root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST).write_text(
            json.dumps(payload["custody"]), encoding="utf-8"
        )
        monkeypatch.setattr(build_environment.sys, "prefix", str(wrong_root))
        active_root = wrong_root
    with pytest.raises(build_environment.SourceBuildEnvironmentError):
        build_environment.source_build_environment(tmp_path, "source-build-numpy")
    assert probes == [(Path(sys.executable), active_root)]


def test_active_namespace_different_group_remains_a_distinct_inactive_selection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    active_source_namespace_case: tuple[dict[str, Any], Path, list[tuple[Path, Path]]],
) -> None:
    payload, active_root, probes = active_source_namespace_case
    other_lock = json.loads(json.dumps(payload["custody"]["lock_closure"]))
    other_lock["dependency_group"] = "source-build-scipy"
    other_lock["closure_sha256"] = canonical_json_sha256(
        {key: value for key, value in other_lock.items() if key != "closure_sha256"}
    )
    monkeypatch.setattr(
        build_environment,
        "selected_uv_lock_group_closure",
        lambda *_args, **_kwargs: other_lock,
    )
    result = build_environment.source_build_environment(tmp_path, "source-build-scipy")
    assert result.active is False
    assert result.root != active_root
    assert result.custody["dependency_group"] == "source-build-scipy"
    assert probes == [(Path(sys.executable), active_root)]


def test_source_build_reexec_uses_typed_args_and_invoking_worktree_src(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path, monkeypatch)
    environment = build_environment.LockedSourceBuildEnvironment(
        root=spec[0],
        python_executable=spec[1],
        manifest_path=spec[2],
        custody=spec[3],
        active=False,
    )
    observed: dict[str, object] = {}

    def run(argv, **kwargs):
        observed["argv"] = argv
        observed.update(kwargs)
        return subprocess.CompletedProcess(argv, 19)

    monkeypatch.setattr(producer.process_guard, "run_completed_command", run)
    monkeypatch.setenv("PYTHONPATH", r"D:\poison;C:\OneDrive\stale")
    monkeypatch.setenv("PYTHONHOME", r"D:\poison-python")
    ambient_scripts = tmp_path / "ambient-scripts"
    ambient_scripts.mkdir()
    locked_scripts = environment.python_executable.parent
    locked_scripts.mkdir(parents=True)
    script_name = "cython.exe" if os.name == "nt" else "cython"
    locked_cython = locked_scripts / script_name
    ambient_cython = ambient_scripts / script_name
    locked_cython.write_text("locked", encoding="utf-8")
    ambient_cython.write_text("ambient", encoding="utf-8")
    locked_cython.chmod(0o755)
    ambient_cython.chmod(0o755)
    inherited_path = os.pathsep.join((str(ambient_scripts), os.environ["PATH"]))
    monkeypatch.setenv("PATH", inherited_path)

    result = producer._run_locked_source_extension_producer(
        environment,
        invocation=SourceExtensionSetInvocation(
            command="produce-set",
            package="numpy",
            package_version="2.5.1",
            module_set="pact-witness",
            python_version="3.12",
            source="source-root",
            build_root="build-root",
            target="wasm",
            abi_tier="cpython-abi",
            json_output=True,
        ),
    )

    assert result == 19
    assert list(observed["argv"]) == [
        str(spec[1]),
        "-P",
        "-m",
        "molt.cli",
        "extension",
        "produce-set",
        "--package",
        "numpy",
        "--package-version",
        "2.5.1",
        "--module-set",
        "pact-witness",
        "--python-version",
        "3.12",
        "--source",
        "source-root",
        "--build-root",
        "build-root",
        "--target",
        "wasm",
        "--abi-tier",
        "cpython-abi",
        "--json",
        "--prepared",
    ]
    assert observed["check"] is False
    assert "capture_output" not in observed
    child_environment = observed["env"]
    assert isinstance(child_environment, dict)
    assert child_environment["PYTHONPATH"] == str(
        Path(__file__).resolve().parents[2] / "src"
    )
    assert "PYTHONHOME" not in child_environment
    assert child_environment["PYTHONNOUSERSITE"] == "1"
    assert child_environment["VIRTUAL_ENV"] == str(spec[0])
    assert child_environment["PATH"] == os.pathsep.join(
        (str(locked_scripts.resolve()), inherited_path)
    )
    resolved_cython = shutil.which("cython", path=child_environment["PATH"])
    assert resolved_cython is not None
    assert Path(resolved_cython).samefile(locked_cython)


@pytest.mark.parametrize(
    ("separator", "scripts", "inherited", "expected"),
    [
        (
            ";",
            r"C:\custody\environment\Scripts",
            r"C:\Program Files\LLVM\bin;C:\Windows\System32",
            r"C:\custody\environment\Scripts;C:\Program Files\LLVM\bin;"
            r"C:\Windows\System32",
        ),
        (
            ":",
            "/custody/environment/bin",
            "/opt/llvm/bin:/usr/bin",
            "/custody/environment/bin:/opt/llvm/bin:/usr/bin",
        ),
    ],
)
def test_locked_console_tool_path_is_cross_platform_and_ordered(
    separator: str,
    scripts: str,
    inherited: str,
    expected: str,
) -> None:
    assert (
        producer._locked_console_tool_path(scripts, inherited, separator=separator)
        == expected
    )


def test_locked_console_tool_path_handles_absent_host_path(tmp_path: Path) -> None:
    scripts = tmp_path / "environment/bin"
    scripts.mkdir(parents=True)
    assert producer._locked_console_tool_path(scripts, None) == str(scripts.resolve())


def _producer_boundary_environment(tmp_path: Path, *, active: bool):
    root = tmp_path / "environment"
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    python.parent.mkdir(parents=True)
    python.write_bytes(b"fixture interpreter; never executed")
    return build_environment.LockedSourceBuildEnvironment(
        root=root,
        python_executable=python,
        manifest_path=root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
        custody={},
        active=active,
    )


def _invoke_producer_mode(tmp_path: Path, *, candidate: bool, prepared: bool) -> int:
    arguments: dict[str, Any] = {
        "package": "numpy",
        "package_version": "2.5.1",
        "module_set": "pact-witness",
        "python_version": "3.12",
        "source": str(tmp_path / "source"),
        "build_root": str(tmp_path / "build"),
        "target": "wasm",
        "prepared": prepared,
    }
    if candidate:
        return producer.attest_source_extension_set_candidate(
            **arguments,
            output=str(tmp_path / "destination/candidate"),
        )
    return producer.produce_source_extension_set(**arguments)


@pytest.mark.parametrize("candidate", [False, True])
@pytest.mark.parametrize("active", [False, True])
def test_producer_prepares_once_before_prepared_reexec_without_publication_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, candidate: bool, active: bool
) -> None:
    (tmp_path / "source").mkdir()
    environment = _producer_boundary_environment(tmp_path, active=active)
    events: list[object] = []
    monkeypatch.setattr(
        producer,
        "resolve_source_extension_candidate_custody_path",
        lambda value: Path(value).resolve(),
    )

    def source_environment(*_args, provision: bool = False):
        events.append(("environment", provision))
        return environment

    def reexec(realized, *, invocation):
        assert realized == environment
        assert invocation.prepared is True
        assert invocation.command == (
            "attest-set-candidate" if candidate else "produce-set"
        )
        events.append("reexec")
        return 23

    monkeypatch.setattr(producer, "source_build_environment", source_environment)
    monkeypatch.setattr(
        producer,
        "verify_source_extension_checkout",
        lambda *a, **kw: events.append("source"),
    )
    monkeypatch.setattr(
        producer,
        "_provision_recursive_submodules",
        lambda *a: events.append("provision-submodules"),
    )
    monkeypatch.setattr(
        producer,
        "_verify_recursive_submodules",
        lambda *a: events.append("verify-submodules"),
    )
    monkeypatch.setattr(
        producer,
        "_run_locked_source_extension_producer",
        reexec,
    )
    monkeypatch.setattr(
        producer,
        "_acquire_file_lock",
        lambda *_args, **_kwargs: pytest.fail(
            "parent must not hold producer publication lock across re-exec"
        ),
    )

    assert _invoke_producer_mode(tmp_path, candidate=candidate, prepared=False) == 23
    assert events == [
        ("environment", False),
        "source",
        "provision-submodules",
        "verify-submodules",
        ("environment", True),
        "reexec",
    ]
    assert not (tmp_path / "destination").exists()


@pytest.mark.parametrize("candidate", [False, True])
@pytest.mark.parametrize("prepared", [False, True])
@pytest.mark.parametrize(
    "invalid",
    [
        "build-file",
        "build-nonempty",
        "build-parent-file",
        "overlap-source",
        "overlap-build",
        "build-in-source",
        "source-in-build",
    ],
)
def test_invalid_output_topology_rejected_before_standalone_setup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
    candidate: bool,
    prepared: bool,
    invalid: str,
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    build = tmp_path / "build"
    destination = tmp_path / "destination" / "output"
    if invalid == "build-file":
        build.write_bytes(b"retain")
    elif invalid == "build-nonempty":
        build.mkdir()
        (build / "retain").write_bytes(b"retain")
    elif invalid == "build-parent-file":
        build.write_bytes(b"retain")
        build = build / "child"
    elif invalid == "overlap-source":
        destination = source / "output"
    elif invalid == "overlap-build":
        destination = build / "output"
    elif invalid == "build-in-source":
        build = source / "build"
    else:
        build = tmp_path
    if prepared:
        environment = _producer_boundary_environment(tmp_path, active=True)

        def current_environment(*args, provision: bool):
            assert provision is False
            return environment

        monkeypatch.setattr(producer, "source_build_environment", current_environment)
        monkeypatch.setattr(
            producer, "verify_source_extension_checkout", lambda *a, **kw: None
        )
        monkeypatch.setattr(producer, "_verify_recursive_submodules", lambda *a: ())
        monkeypatch.setattr(
            producer, "verify_source_extension_abi_headers", lambda *a, **kw: None
        )
    before = tuple(sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*")))

    def forbidden(*args, **kwargs):
        pytest.fail("invalid output topology reached environment/setup/output mutation")

    if not prepared:
        monkeypatch.setattr(producer, "source_build_environment", forbidden)
    for name in (
        "prepare_source_extension_prerequisites",
        "_run_locked_source_extension_producer",
        "_acquire_file_lock",
    ):
        monkeypatch.setattr(producer, name, forbidden)
    monkeypatch.setattr(
        producer, "source_extension_set_root", lambda *a, **kw: destination
    )
    monkeypatch.setattr(
        producer,
        "resolve_source_extension_candidate_custody_path",
        lambda value: Path(value).resolve(),
    )
    arguments = dict(
        package="numpy",
        package_version="2.5.1",
        module_set="pact-witness",
        python_version="3.12",
        source=str(source),
        build_root=str(build),
        target="wasm",
        prepared=prepared,
    )
    result = (
        producer.attest_source_extension_set_candidate(
            **arguments, output=str(destination)
        )
        if candidate
        else producer.produce_source_extension_set(**arguments)
    )
    assert result != 0
    error = capsys.readouterr().err
    assert (
        "disjoint" in error
        or "not a directory" in error
        or "prior configuration" in error
    )
    assert (
        tuple(sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*")))
        == before
    )


def test_fresh_build_root_validation_does_not_create_parent(tmp_path: Path) -> None:
    build = tmp_path / "absent" / "build"
    producer._require_fresh_build_root(build)
    assert not build.parent.exists()


def test_candidate_custody_rejected_before_standalone_setup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
) -> None:
    (tmp_path / "source").mkdir()
    monkeypatch.setattr(
        producer,
        "source_build_environment",
        lambda *a, **kw: pytest.fail(
            "invalid candidate custody reached environment setup"
        ),
    )
    assert _invoke_producer_mode(tmp_path, candidate=True, prepared=False) != 0
    assert "canonical candidate custody" in capsys.readouterr().err
    assert not (tmp_path / "destination").exists()


@pytest.mark.parametrize("candidate", [False, True])
@pytest.mark.parametrize("failure", ["inactive", "stale", "source", "submodules"])
def test_prepared_producer_rejects_prerequisite_drift_without_any_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
    candidate: bool,
    failure: str,
) -> None:
    (tmp_path / "source").mkdir()
    environment = _producer_boundary_environment(tmp_path, active=failure != "inactive")
    events = []

    def source_environment(*_args, provision: bool):
        assert provision is False
        events.append("environment")
        if failure == "stale":
            raise build_environment.SourceBuildEnvironmentError(
                "stale fixture attestation"
            )
        return environment

    def verify_source(*args, **kwargs):
        events.append("source")
        if failure == "source":
            raise ValueError("source fixture drift")

    def verify_submodules(*args):
        events.append("submodules")
        raise producer.SourceExtensionProducerError("submodule fixture drift")

    def forbidden(*args, **kwargs):
        pytest.fail(
            "prepared prerequisite failure must not repair, restart or touch output"
        )

    monkeypatch.setattr(producer, "source_build_environment", source_environment)
    monkeypatch.setattr(producer, "verify_source_extension_checkout", verify_source)
    monkeypatch.setattr(producer, "_verify_recursive_submodules", verify_submodules)
    for name in (
        "prepare_source_extension_prerequisites",
        "_provision_recursive_submodules",
        "_run_locked_source_extension_producer",
        "_acquire_file_lock",
        "source_extension_set_root",
        "resolve_source_extension_candidate_custody_path",
    ):
        monkeypatch.setattr(producer, name, forbidden)
    assert _invoke_producer_mode(tmp_path, candidate=candidate, prepared=True) != 0
    assert events == (
        ["environment"]
        if failure in {"inactive", "stale"}
        else ["environment", "source"]
        if failure == "source"
        else ["environment", "source", "submodules"]
    )
    expected = (
        "active locked"
        if failure == "inactive"
        else (
            "submodule fixture drift"
            if failure == "submodules"
            else f"{failure} fixture"
        )
    )
    assert expected in capsys.readouterr().err
    assert not (tmp_path / "build").exists()
    assert not (tmp_path / "destination").exists()


@pytest.mark.parametrize("candidate", [False, True])
def test_prepared_producer_verifies_before_destination_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    candidate: bool,
) -> None:
    (tmp_path / "source").mkdir()
    environment = _producer_boundary_environment(tmp_path, active=True)
    events = []

    class VerifiedBeforeOutput(Exception):
        pass

    def source_environment(*_args, provision: bool):
        assert provision is False
        events.append("environment")
        return environment

    def destination(*args, **kwargs):
        assert events == ["environment", "source", "submodules", "headers"]
        raise VerifiedBeforeOutput

    def forbidden(*args, **kwargs):
        pytest.fail("prepared execution must never prepare or restart")

    monkeypatch.setattr(producer, "source_build_environment", source_environment)
    monkeypatch.setattr(
        producer,
        "verify_source_extension_checkout",
        lambda *a, **kw: events.append("source"),
    )
    monkeypatch.setattr(
        producer,
        "_verify_recursive_submodules",
        lambda *a: events.append("submodules") or (),
    )
    monkeypatch.setattr(
        producer,
        "verify_source_extension_abi_headers",
        lambda *a, **kw: events.append("headers"),
    )
    monkeypatch.setattr(producer, "source_extension_set_root", destination)
    monkeypatch.setattr(
        producer, "resolve_source_extension_candidate_custody_path", destination
    )
    for name in (
        "prepare_source_extension_prerequisites",
        "_provision_recursive_submodules",
        "_run_locked_source_extension_producer",
    ):
        monkeypatch.setattr(producer, name, forbidden)
    with pytest.raises(VerifiedBeforeOutput):
        _invoke_producer_mode(tmp_path, candidate=candidate, prepared=True)
    assert not (tmp_path / "destination").exists()


@pytest.mark.parametrize("field", ["root", "python_executable", "manifest_path"])
def test_shared_preparation_rejects_changed_environment_address(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
) -> None:
    from dataclasses import replace

    environment = _producer_boundary_environment(tmp_path, active=False)
    changed = replace(environment, **{field: tmp_path / "substituted"})
    monkeypatch.setattr(producer, "source_build_environment", lambda *a, **kw: changed)
    monkeypatch.setattr(
        producer, "verify_source_extension_checkout", lambda *a, **kw: None
    )
    monkeypatch.setattr(producer, "_provision_recursive_submodules", lambda *a: None)
    monkeypatch.setattr(producer, "_verify_recursive_submodules", lambda *a: ())
    extension_set = producer.source_extension_set("numpy", "2.5.1", "pact-witness")
    with pytest.raises(
        producer.SourceExtensionProducerError, match=f"environment {field}"
    ):
        producer.prepare_source_extension_prerequisites(
            extension_set,
            tmp_path,
            repo_root=tmp_path,
            planned_environment=environment,
        )


def test_meson_setup_uses_typed_driver(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[str, ...]] = []

    backend = producer._SourceNinjaDriver(
        image=_tool_image(tmp_path / "embedded tools" / "ninja"),
        manifest={},
    )
    monkeypatch.setenv("NINJA", "unattested-ambient-ninja")

    def run_process(argv, *, cwd, env):
        assert cwd == tmp_path / "source"
        assert env["NINJA"] == str(backend.image.path)
        calls.append(tuple(argv))
        return subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="", stderr=""
        )

    monkeypatch.setattr(producer, "_run_process", run_process)

    producer._run_meson_setup(
        source_root=tmp_path / "source",
        build_root=tmp_path / "build",
        meson_cross_files=(
            tmp_path / "metadata/meson.cross",
            tmp_path / "metadata/build-tools.cross",
        ),
        meson_native=tmp_path / "metadata/meson.native",
        setup_args=("-Dblas=none",),
        backend=backend,
        driver=producer._SourceMesonDriver(
            command=(sys.executable, "-m", "mesonbuild.mesonmain"),
            manifest={"kind": "build-environment"},
        ),
    )

    assert calls == [
        (
            producer.sys.executable,
            "-m",
            "mesonbuild.mesonmain",
            "setup",
            str(tmp_path / "build"),
            str(tmp_path / "source"),
            "--cross-file",
            str(tmp_path / "metadata/meson.cross"),
            "--cross-file",
            str(tmp_path / "metadata/build-tools.cross"),
            "--native-file",
            str(tmp_path / "metadata/meson.native"),
            f"--prefix={producer.MESON_INSTALL_PREFIX}",
            "-Dblas=none",
        )
    ]


def test_upstream_vendored_meson_is_the_driver_authority(tmp_path: Path) -> None:
    source = tmp_path / "source"
    driver = source / "vendor/meson.py"
    driver.parent.mkdir(parents=True)
    driver.write_text("# upstream meson\n", encoding="utf-8")
    (source / "pyproject.toml").write_text(
        "[tool.meson-python]\nmeson = 'vendor/meson.py'\n",
        encoding="utf-8",
    )

    resolved = producer._source_meson_driver(source, _source_environment())

    assert resolved.command == (sys.executable, str(driver.resolve()))
    assert resolved.manifest_payload() == {
        "kind": "source-vendored",
        "path": "vendor/meson.py",
        "sha256": hashlib.sha256(driver.read_bytes()).hexdigest(),
    }


def test_generated_input_materialization_uses_one_upstream_meson_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    build = tmp_path / "build"
    source.mkdir()
    build.mkdir()
    version = build / "numpy/version.py"
    generated_c = build / "numpy/_core/loops.c"
    calls: list[tuple[str, ...]] = []
    monkeypatch.setattr(
        producer,
        "_missing_installed_generated_inputs",
        lambda **_kwargs: {version},
    )
    monkeypatch.setattr(
        producer,
        "_missing_extension_generated_inputs",
        lambda **_kwargs: {generated_c},
    )

    def run_process(argv, *, cwd):
        assert cwd == source
        calls.append(tuple(argv))
        version.parent.mkdir(parents=True)
        generated_c.parent.mkdir(parents=True)
        version.write_text("version = '2.5.1'\n", encoding="utf-8")
        generated_c.write_text("int generated;\n", encoding="utf-8")
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(producer, "_run_process", run_process)
    backend = producer._SourceNinjaDriver(
        image=_tool_image(tmp_path / "ninja"),
        manifest={"distribution": "ninja"},
    )

    materialized = producer._materialize_generated_inputs(
        backend=backend,
        source_root=source,
        build_root=build,
        intro_targets=build / "meson-info/intro-targets.json",
        intro_installed=build / "meson-info/intro-installed.json",
        extension_set=SourceExtensionSet(
            package="numpy",
            package_version="2.5.1",
            source=SourceExtensionSource("git", "a" * 40),
            name="pact-witness",
            seal_name="numpy-witness",
            variants=(),
            build_dependency_group="source-build-numpy",
            meson_setup_args=(),
            use_pkg_config=False,
            required_installed_files=(),
            extensions=(),
        ),
    )

    assert materialized == (generated_c, version)
    assert calls == [
        (
            str(backend.image.path),
            "-C",
            str(build),
            "numpy/_core/loops.c",
            "numpy/version.py",
        )
    ]


def test_cython_generated_input_uses_standalone_regeneration_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    build = tmp_path / "build"
    build.mkdir()
    generated_c = build / "scipy/ndimage/_ni_label.pyd.p/_ni_label.c"
    pyx = tmp_path / "source/scipy/ndimage/src/_ni_label.pyx"
    pyx.parent.mkdir(parents=True)
    pyx.write_text("cdef int value\n", encoding="utf-8")
    intro_targets = build / "meson-info/intro-targets.json"
    intro_targets.parent.mkdir(parents=True)
    intro_targets.write_text(
        json.dumps(
            [
                {
                    "id": "_ni_label",
                    "name": "_ni_label",
                    "type": "shared module",
                    "filename": [str(build / "scipy/ndimage/_ni_label.pyd")],
                    "target_sources": [
                        {
                            "generated_sources": [str(generated_c)],
                        }
                    ],
                }
            ]
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        producer,
        "_load_ninja_build_all_inputs",
        lambda _root: {generated_c.resolve(): (pyx.resolve(),)},
    )
    backend = producer._SourceNinjaDriver(
        image=_tool_image(tmp_path / "ninja"),
        manifest={"distribution": "ninja"},
    )

    def generated_c_pyx_from_ninja(**kwargs):
        assert tuple(kwargs["ninja_command"]) == backend.command
        return pyx, None

    monkeypatch.setattr(
        producer._source_extension_cython,
        "generated_c_pyx_from_ninja",
        generated_c_pyx_from_ninja,
    )

    missing = producer._missing_extension_generated_inputs(
        backend=backend,
        build_root=build,
        intro_targets=intro_targets,
        extension_set=SourceExtensionSet(
            package="scipy",
            package_version="1.18.0",
            source=SourceExtensionSource("git", "a" * 40),
            name="pact-witness",
            seal_name="scipy-witness",
            variants=(),
            build_dependency_group="source-build-scipy",
            meson_setup_args=(),
            use_pkg_config=False,
            required_installed_files=(),
            extensions=(
                SourceExtensionSpec(
                    module="scipy.ndimage._ni_label",
                    target="_ni_label",
                    python_exports=("scipy.ndimage._ni_label",),
                    capabilities=(),
                    provided_capsules=(),
                    exclude_linked_static_libraries=(),
                ),
            ),
        ),
    )

    assert missing == set()


@pytest.mark.parametrize("console_script", [False, True])
def test_meson_pkg_config_is_pinned_and_attested(
    console_script: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    payload = _build_environment_manifest([producer.MOLT_PKGCONF_REQUIREMENT])
    tool = _install_realized_tool(
        payload,
        tmp_path,
        "pkg-config",
        distribution="pkgconf",
        console_script=console_script,
    )
    monkeypatch.setattr(producer.sys, "prefix", str(tmp_path))
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="3.0.1\n", stderr=""
        ),
    )
    environment = _source_environment(payload)

    config_tool = producer._ensure_meson_pkg_config(tmp_path, environment)

    assert config_tool.path == tool
    assert config_tool.manifest_payload() == {
        "name": "pkg-config",
        "path": tool.name,
        "distribution": "pkgconf",
        "version": "3.0.1.post0",
        "sha256": hashlib.sha256(tool.read_bytes()).hexdigest(),
    }
    assert producer._source_build_config_tools(environment) == (config_tool,)


def test_meson_pkg_config_rejects_mutated_realized_tool(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    payload = _build_environment_manifest([producer.MOLT_PKGCONF_REQUIREMENT])
    tool = _install_realized_tool(
        payload, tmp_path, "pkg-config", distribution="pkgconf"
    )
    monkeypatch.setattr(producer.sys, "prefix", str(tmp_path))
    tool.write_bytes(b"mutated-unattested-tool")
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("mutated tool must not execute"),
    )
    with pytest.raises(ValueError, match="content differs"):
        producer._ensure_meson_pkg_config(tmp_path, _source_environment(payload))


def test_realized_inventory_public_projections_are_immutable(tmp_path: Path) -> None:
    payload = _build_environment_manifest([producer.MOLT_PKGCONF_REQUIREMENT])
    _install_realized_tool(payload, tmp_path, "pkg-config", distribution="pkgconf")
    inventory = producer.SourceBuildInventory(payload["custody"], tmp_path)
    owner = inventory.distribution("PkgConf")
    assert owner is not None
    with pytest.raises(TypeError):
        inventory.identity["scripts_root"] = "mutable-poison"
    with pytest.raises(TypeError):
        inventory.distributions["pkgconf"] = {}
    with pytest.raises(TypeError):
        owner["version"] = "99"
    with pytest.raises(TypeError):
        owner["entry_points"][0]["value"] = "poison:main"
    with pytest.raises(TypeError):
        owner["installed_files"][0]["path"] = "poison"
    with pytest.raises(TypeError):
        owner["console_scripts"]["pkg-config"][0] = "poison"


def test_realized_inventory_detaches_caller_owned_receipt_before_tool_checks(
    tmp_path: Path,
) -> None:
    payload = _build_environment_manifest([producer.MOLT_PKGCONF_REQUIREMENT])
    tool = _install_realized_tool(
        payload, tmp_path, "pkg-config", distribution="pkgconf"
    )
    realized = payload["custody"]["realized_environment"]
    inventory = producer.SourceBuildInventory(payload["custody"], tmp_path)
    relative = tool.relative_to(tmp_path).as_posix()
    entry = next(row for row in realized["tree"]["entries"] if row["path"] == relative)
    node = next(
        row for row in realized["tree"]["file_nodes"] if row["id"] == entry["node"]
    )
    realized["scripts_root"] = "poison"
    realized["distributions"][0]["console_scripts"]["pkg-config"].clear()
    realized["distributions"][0]["installed_files"].clear()
    entry["access"]["writable"] = not entry["access"]["writable"]
    assert inventory.executable("pkg-config", distribution="pkgconf").path == tool
    changed = b"replaced-tool-with-rewritten-unsealed-expectations"
    tool.write_bytes(changed)
    node["size"] = len(changed)
    node["sha256"] = hashlib.sha256(changed).hexdigest()
    with pytest.raises(ValueError, match="content differs"):
        inventory.executable("pkg-config", distribution="pkgconf")


@pytest.mark.parametrize("version", ["3.15.0a1", "3.15.0b2", "3.15.0rc1"])
def test_producer_manifest_preserves_prerelease_marker_version(version: str) -> None:
    payload = _build_environment_manifest()
    payload["marker_environment"]["python_full_version"] = version
    payload["marker_environment"]["implementation_version"] = version
    payload["marker_environment"]["python_version"] = "3.15"
    environment = _source_environment(payload)
    assert environment.manifest_payload()["python"]["version"] == version
    assert (
        environment.manifest_payload()["marker_environment"]
        == payload["marker_environment"]
    )


def test_meson_pkg_config_missing_does_not_install_into_active_interpreter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )
    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="never installs into its active interpreter",
    ):
        producer._ensure_meson_pkg_config(tmp_path, _source_environment())


def test_meson_config_tool_cross_is_generic_and_deterministic(tmp_path: Path) -> None:
    tools = (
        producer._SourceBuildConfigTool(
            name="pybind11-config",
            image=_tool_image(tmp_path / "Scripts/pybind11-config.exe"),
            distribution="pybind11",
            version="3.0.4",
        ),
        producer._SourceBuildConfigTool(
            name="numpy-config",
            image=_tool_image(tmp_path / "Scripts/numpy-config.exe"),
            distribution="numpy",
            version="2.5.1",
        ),
    )
    cross = tmp_path / "metadata/build-tools.cross"

    assert producer._materialize_meson_config_tool_cross(cross, tools) == cross
    assert cross.read_text(encoding="utf-8") == (
        "[binaries]\n"
        f"numpy-config = ['{tools[1].path}']\n"
        f"pybind11-config = ['{tools[0].path}']\n"
    ).replace("\\", "\\\\")


def test_recursive_submodule_verification_rejects_drift(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            args=[], returncode=0, stdout="+deadbeef scipy/_lib/pocketfft\n", stderr=""
        ),
    )

    with pytest.raises(producer.SourceExtensionProducerError, match="unpinned"):
        producer._verify_recursive_submodules(tmp_path)


def test_recursive_submodule_verification_rejects_incomplete_pinned_tree(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[str, ...]] = []

    def run_process(argv, *, cwd):
        assert cwd == tmp_path
        calls.append(tuple(argv))
        if "foreach" in argv:
            return subprocess.CompletedProcess(
                args=list(argv),
                returncode=0,
                stdout=f"subprojects/boost_math/math\t{'d' * 40}\n",
                stderr="",
            )
        if "status" in argv:
            return subprocess.CompletedProcess(
                args=list(argv),
                returncode=0,
                stdout=" deadbeef subprojects/boost_math/math\n",
                stderr="",
            )
        return subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout=" D missing.cpp\n", stderr=""
        )

    monkeypatch.setattr(producer, "_run_process", run_process)

    with pytest.raises(
        producer.SourceExtensionProducerError, match="incomplete pinned submodule"
    ):
        producer._verify_recursive_submodules(tmp_path)
    assert calls[-1][-3:] == (
        "status",
        "--porcelain=v1",
        "--untracked-files=no",
    )


def test_recursive_submodule_attestation_is_canonical_path_order(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    for relative in ("z/submodule", "a/submodule"):
        (tmp_path / relative).mkdir(parents=True)

    def run_process(argv, *, cwd):
        assert cwd == tmp_path
        assert argv[:2] == ("git", "--no-optional-locks")
        if "foreach" in argv:
            assert "git --no-optional-locks rev-parse HEAD" in argv[-1]
            return subprocess.CompletedProcess(
                args=list(argv),
                returncode=0,
                stdout=(f"z/submodule\t{'0' * 40}\na/submodule\t{'f' * 40}\n"),
                stderr="",
            )
        if "submodule" in argv:
            return subprocess.CompletedProcess(
                args=list(argv),
                returncode=0,
                stdout=(
                    f" {'0' * 40} z/submodule (heads/main)\n"
                    f" {'f' * 40} a/submodule (heads/main)\n"
                ),
                stderr="",
            )
        return subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="", stderr=""
        )

    monkeypatch.setattr(producer, "_run_process", run_process)

    assert tuple(
        item.manifest_payload()
        for item in producer._verify_recursive_submodules(tmp_path)
    ) == (
        {"path": "a/submodule", "commit": "f" * 40},
        {"path": "z/submodule", "commit": "0" * 40},
    )


def test_source_checkout_verification_disables_optional_index_writes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_set_registry

    extension_set = producer.source_extension_set("numpy", "2.5.1", "pact-witness")
    commands = []

    def run(argv, **kwargs):
        commands.append(argv)
        assert argv[:2] == ["git", "--no-optional-locks"]
        return subprocess.CompletedProcess(
            argv,
            0,
            stdout=extension_set.source.commit if "rev-parse" in argv else "",
            stderr="",
        )

    monkeypatch.setattr(source_extension_set_registry, "run_completed_command", run)
    producer.verify_source_extension_checkout(extension_set, tmp_path)
    assert len(commands) == 2
    assert commands[-1][-3:] == ["status", "--porcelain=v1", "--untracked-files=all"]


def test_recursive_submodules_are_provisioned_from_pinned_checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[str, ...]] = []

    def run_process(argv, *, cwd):
        assert cwd == tmp_path
        calls.append(tuple(argv))
        return subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="", stderr=""
        )

    monkeypatch.setattr(producer, "_run_process", run_process)

    producer._provision_recursive_submodules(tmp_path)

    assert calls == [
        (
            "git",
            "-c",
            "core.longpaths=true",
            "-C",
            str(tmp_path),
            "submodule",
            "update",
            "--init",
            "--recursive",
        )
    ]


def test_complete_set_validator_rejects_duplicate_module_sidecar(
    tmp_path: Path,
) -> None:
    publish = tmp_path / "publish"
    _write_complete_root(publish, marker="new")
    extension_set = SourceExtensionSet(
        package="scipy",
        package_version="1.18.0",
        source=SourceExtensionSource("git", "a" * 40),
        name="pact-witness",
        seal_name="pact_scipy_witness",
        variants=(),
        build_dependency_group="source-build-scipy",
        meson_setup_args=(),
        use_pkg_config=True,
        required_installed_files=(),
        extensions=tuple(
            SourceExtensionSpec(
                module=module,
                target=module.rsplit(".", 1)[-1],
                python_exports=(module,),
                capabilities=(),
                provided_capsules=(),
                exclude_linked_static_libraries=(),
            )
            for module in _MODULES
        ),
        required_config_tools=("pkg-config",),
    )
    set_manifest = {
        "schema_version": producer.SOURCE_EXTENSION_SET_SCHEMA_VERSION,
        "kind": "molt-source-extension-set",
        "package": "scipy",
        "name": "pact-witness",
        "seal_name": "pact_scipy_witness",
        "source_head": "a" * 40,
        "submodules": [],
        "build_environment": _build_environment_manifest(),
        "meson": _write_meson_metadata(publish, extension_set),
        "cpython": "3.12",
        "abi_tier": "cpython-abi",
        "target_triple": "wasm32-wasip1",
        "package_version": "1.18.0",
        "target_metadata": _write_target_metadata(publish),
        "installed_package_files": [],
        "extensions": [
            {
                "module": spec.module,
                "target": spec.target,
                "python_exports": list(spec.python_exports),
                "capabilities": list(spec.capabilities),
                "provided_capsules": list(spec.provided_capsules),
                "exclude_linked_static_libraries": list(
                    spec.exclude_linked_static_libraries
                ),
                "artifact_sha256": hashlib.sha256(
                    publish.joinpath(*spec.module.split("."))
                    .with_suffix(".molt.wasm")
                    .read_bytes()
                ).hexdigest(),
                "wheel_sha256": hashlib.sha256(
                    f"wheel:{spec.module}".encode()
                ).hexdigest(),
                "object_closure_sha256": json.loads(
                    publish.joinpath(*spec.module.split("."))
                    .with_suffix(".molt.wasm.extension_manifest.json")
                    .read_text(encoding="utf-8")
                )["object_closure"]["closure_sha256"],
            }
            for spec in extension_set.extensions
        ],
    }
    variant = SourceExtensionVariant(
        target_python=TargetPythonVersion(3, 12, 0),
        abi_tier="cpython-abi",
        target_triple="wasm32-wasip1",
    )
    set_validation.validate_source_extension_set_publish_root(
        publish_root=publish,
        extension_set=extension_set,
        variant=variant,
        set_manifest=set_manifest,
    )
    target_metadata = set_manifest["target_metadata"]
    assert isinstance(target_metadata, dict)
    target_metadata["schema_version"] = 2
    with pytest.raises(
        set_validation.SourceExtensionSetValidationError,
        match="target metadata contract",
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )
    target_metadata["schema_version"] = SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION
    target_facts = target_metadata["target"]
    assert isinstance(target_facts, dict)
    target_facts["artifact_kind"] = "static_archive"
    with pytest.raises(
        set_validation.SourceExtensionSetValidationError,
        match="canonical target plan",
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )
    target_facts["artifact_kind"] = "wasm_relocatable_object"
    set_manifest["extensions"][0]["target"] = "wrong-target"
    with pytest.raises(
        set_validation.SourceExtensionSetValidationError,
        match="published extension artifacts differ",
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )
    set_manifest["extensions"][0]["target"] = extension_set.extensions[0].target
    duplicate = (
        publish / ".duplicate/scipy/ndimage/_nd_image.molt.wasm.extension_manifest.json"
    )
    duplicate.parent.mkdir(parents=True)
    duplicate.write_text("{}", encoding="utf-8")

    with pytest.raises(
        set_validation.SourceExtensionSetValidationError, match="unexpected"
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )
    duplicate.unlink()
    opposite_variant = (
        publish / "scipy/ndimage/_nd_image.molt.a.extension_manifest.json"
    )
    opposite_variant.write_text("{}", encoding="utf-8")

    with pytest.raises(
        set_validation.SourceExtensionSetValidationError, match="unexpected"
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )
    opposite_variant.unlink()
    opposite_artifact = publish / "scipy/ndimage/_nd_image.molt.a"
    opposite_artifact.write_bytes(b"opposite-target")

    with pytest.raises(
        set_validation.SourceExtensionSetValidationError, match="unexpected"
    ):
        set_validation.validate_source_extension_set_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            variant=variant,
            set_manifest=set_manifest,
        )


def test_extension_staging_rewrites_all_inputs_into_relocatable_seal_payload(
    tmp_path: Path,
) -> None:
    source_root = tmp_path / "checkout"
    build_root = tmp_path / "meson-build"
    transaction = tmp_path / "transaction"
    output = transaction / "builds" / "00-extension"
    publish = transaction / "publish"
    module = "scipy.ndimage._nd_image"
    source = source_root / "scipy/ndimage/src/nd_image.c"
    generated = build_root / "scipy/ndimage/_nd_image.c"
    header = source_root / "scipy/ndimage/src/nd_image.h"
    for path, content in (
        (
            source,
            b"int source_symbol(void) { return 1; }\n"
            b"int import_numpy(void) { return _import_array(); }\n",
        ),
        (generated, b"int PyInit__nd_image(void) { return 2; }\n"),
        (header, b"#define ND_IMAGE_VALUE 1\n"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    expected_inputs = {
        source_extension_input_custody_path(
            hashlib.sha256(path.read_bytes()).hexdigest()
        ).as_posix(): path.read_bytes()
        for path in (source, generated, header)
    }
    artifact = output / "scipy/ndimage/_nd_image.molt.wasm"
    artifact.parent.mkdir(parents=True)
    artifact.write_bytes(b"\x00asm-object")
    wheel = output / "scipy-1.0-py3-molt_abi1-wasm32_wasip1.whl"
    raw_wheel_sha256 = _write_test_extension_wheel(
        wheel,
        extension_path="scipy/ndimage/_nd_image.molt.wasm",
        extension_bytes=artifact.read_bytes(),
    )
    closure = {
        "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
        "root_symbol": "PyInit__nd_image",
        "init_symbol_owner": "1.o",
        "defined_symbols": ["PyInit__nd_image", "source_symbol"],
        "undefined_symbols": [],
        "runtime_symbols": [],
        "required_c_api_symbols": [],
        "required_capsules": [],
        "project_generated_c_api_symbols": [],
        "wasm_imports": [],
        "objects": [
            {
                "source": str(source),
                "object": "0.o",
                "producer_unit": {
                    "target_id": "_nd_image",
                    "object": "_nd_image.so.p/0.o",
                },
                "language": "c",
                "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                "object_sha256": "1" * 64,
                "defined_symbols": ["source_symbol"],
                "undefined_symbols": [],
                "compile_command": ["clang", "-x", "c", "-c", str(source)],
                "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                "dependencies": [
                    {
                        "path": str(header),
                        "sha256": hashlib.sha256(header.read_bytes()).hexdigest(),
                    }
                ],
                "required_c_api_symbols": [],
                "required_capsules": [],
                "project_generated_c_api_symbols": [],
            },
            {
                "source": str(generated),
                "object": "1.o",
                "producer_unit": {
                    "target_id": "_nd_image",
                    "object": "_nd_image.so.p/1.o",
                },
                "language": "c",
                "source_sha256": hashlib.sha256(generated.read_bytes()).hexdigest(),
                "object_sha256": "2" * 64,
                "defined_symbols": ["PyInit__nd_image"],
                "undefined_symbols": [],
                "compile_command": ["clang", "-x", "c", "-c", str(generated)],
                "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                "dependencies": [],
                "required_c_api_symbols": [],
                "required_capsules": [],
                "project_generated_c_api_symbols": [],
            },
        ],
    }
    manifest = {
        "module": module,
        "init_symbol": "PyInit__nd_image",
        "artifact_kind": "wasm_relocatable_object",
        "target_triple": "wasm32-wasip1",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "extension": artifact.name,
        "wheel": wheel.name,
        "wheel_sha256": raw_wheel_sha256,
        "extension_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "sources": [str(source), str(generated)],
        "source_plan": {
            "source_root": str(source_root),
            "build_root": str(build_root),
            "target_selector": "_nd_image",
            "sources": [str(source)],
            "generated_sources": [str(generated)],
            "compile_units": [
                {"source": str(source)},
                {"source": str(generated)},
            ],
            "digest": "stale-location-dependent-digest",
        },
        "build": {"source_plan_digest": "stale"},
        "object_closure": closure,
    }
    _closure_identity, closure_sha256 = finalize_source_extension_object_closure(
        manifest
    )
    sidecar = artifact.with_name(artifact.name + ".extension_manifest.json")
    sidecar.write_text(json.dumps(manifest), encoding="utf-8")
    intro = publish / "provenance/metadata/meson/intro-targets.json"
    commands = publish / "provenance/metadata/meson/compile-commands.json"
    intro.parent.mkdir(parents=True)
    intro.write_text("{}\n", encoding="utf-8")
    commands.write_text("[]\n", encoding="utf-8")
    produced = producer._ProducedExtension(
        module=module,
        target="_nd_image",
        capabilities=(),
        output_root=output,
        manifest_path=output / "extension_manifest.json",
        artifact_path=artifact,
        artifact_manifest_path=sidecar,
        wheel_path=wheel,
        artifact_sha256=hashlib.sha256(artifact.read_bytes()).hexdigest(),
        wheel_sha256=hashlib.sha256(wheel.read_bytes()).hexdigest(),
        object_closure_sha256=closure_sha256,
    )

    staged = producer._stage_extension(
        produced,
        publish_root=publish,
        location_roots=(
            (source_root, "@source"),
            (build_root, "@build"),
            (transaction, "@transaction"),
        ),
        plan_metadata={"intro_targets": intro, "compile_commands": commands},
    )

    staged_manifest = json.loads(staged.artifact_manifest_path.read_text())
    validate_source_extension_manifest_input_custody(staged_manifest)
    assert staged_manifest["runtime_python_import_modules"] == []
    assert "source_root" not in staged_manifest["source_plan"]
    assert "build_root" not in staged_manifest["source_plan"]
    assert "compile_units" not in staged_manifest["source_plan"]
    assert "generated_sources" not in staged_manifest["source_plan"]
    assert staged_manifest["sources"] == [
        item["source"] for item in staged_manifest["object_closure"]["objects"]
    ]
    expected_capsule = "numpy.core._multiarray_umath._ARRAY_API"
    assert staged_manifest["object_closure"]["required_capsules"] == [expected_capsule]
    assert _manifest_sequence(
        staged_manifest,
        staged_manifest["object_closure"]["objects"][0],
        "required_capsules",
    ) == [expected_capsule]
    assert str(tmp_path) not in json.dumps(staged_manifest)
    for item in staged_manifest["object_closure"]["objects"]:
        staged_source = (
            staged.artifact_manifest_path.parent / item["source"]
        ).resolve()
        assert staged_source.is_file()
        assert staged_source.is_relative_to(
            (publish / "provenance/compiled-inputs").resolve()
        )
    staged_wheel = (
        staged.artifact_manifest_path.parent / staged_manifest["wheel"]
    ).resolve()
    assert staged_wheel == staged.wheel_path.resolve()
    assert staged_manifest["wheel_sha256"] == producer._sha256_file(staged_wheel)
    assert staged.wheel_sha256 == staged_manifest["wheel_sha256"]
    with zipfile.ZipFile(staged_wheel) as archive:
        embedded = json.loads(archive.read("extension_manifest.json"))
        assert embedded["extension"] == "scipy/ndimage/_nd_image.molt.wasm"
        assert archive.read(embedded["extension"]) == staged.artifact_path.read_bytes()
        assert "_nd_image.molt.wasm" not in archive.namelist()
        assert str(tmp_path) not in json.dumps(embedded)
        assert embedded["extension_sha256"] == staged_manifest["extension_sha256"]
        assert embedded["object_closure"]["required_capsules"] == [expected_capsule]
        validate_source_extension_manifest_input_custody(embedded)
        assert embedded["runtime_python_import_modules"] == []
        assert _manifest_sequence(
            embedded,
            embedded["object_closure"]["objects"][0],
            "required_capsules",
        ) == [expected_capsule]
        embedded_inputs = {
            item["source"] for item in embedded["object_closure"]["objects"]
        }
        embedded_inputs.update(
            dependency["path"]
            for item in embedded["object_closure"]["objects"]
            for dependency in _manifest_dependencies(embedded, item)
        )
        assert embedded_inputs == set(expected_inputs)
        for member, data in expected_inputs.items():
            assert archive.namelist().count(member) == 1
            assert archive.read(member) == data
        assert all(
            info.date_time == (1980, 1, 1, 0, 0, 0) for info in archive.infolist()
        )
    assert staged_manifest["object_closure"]["closure_sha256"] == (
        staged.object_closure_sha256
    )

    # The extracted wheel, not either producer checkout or the publication
    # directory, must own every input needed to restage a sealed extension.
    for original_input in (source, generated, header):
        original_input.unlink()
    extracted = tmp_path / "scipy" / "relocated" / "scipy" / "extracted"
    with zipfile.ZipFile(staged_wheel) as archive:
        archive.extractall(extracted)
    for member in expected_inputs:
        (publish / member).unlink()
    extracted_manifest_path = extracted / "extension_manifest.json"
    extracted_manifest = json.loads(extracted_manifest_path.read_text(encoding="utf-8"))
    # This fixture has synthetic artifact bytes, so prove the real seal layout
    # helpers here without claiming full binary admission or command resealing.
    extracted_artifact, artifact_errors = _resolve_declared_artifact(
        manifest=extracted_manifest,
        manifest_path=extracted_manifest_path,
    )
    assert artifact_errors == []
    assert extracted_artifact is not None
    assert (
        extracted_artifact
        == (extracted / "scipy/ndimage/_nd_image.molt.wasm").resolve()
    )
    assert (
        _source_package_root_for_manifest(
            artifact_path=extracted_artifact,
            module_parts=tuple(extracted_manifest["module"].split(".")),
        )
        == extracted.resolve()
    )
    resealed_root = tmp_path / "resealed"
    resealed_manifest_path = (
        resealed_root / "scipy/ndimage/_nd_image.molt.wasm.extension_manifest.json"
    )
    restaged_inputs = stage_source_extension_manifest_inputs(
        extracted_manifest,
        manifest_path=extracted_manifest_path,
        publish_root=resealed_root,
    )
    resealed_manifest = project_source_extension_manifest_inputs(
        extracted_manifest,
        source_manifest_path=extracted_manifest_path,
        output_manifest_path=resealed_manifest_path,
        publish_root=resealed_root,
        staged_inputs=restaged_inputs,
    )
    validate_source_extension_manifest_input_custody(resealed_manifest)
    assert len(restaged_inputs) == len(expected_inputs)
    assert resealed_manifest["runtime_python_import_modules"] == []
    for item in resealed_manifest["object_closure"]["objects"]:
        inputs = [(item["source"], item["source_sha256"])]
        inputs.extend(
            (dependency["path"], dependency["sha256"])
            for dependency in _manifest_dependencies(resealed_manifest, item)
        )
        for reference, digest in inputs:
            resolved, errors = resolve_source_extension_manifest_input(
                reference,
                manifest_path=resealed_manifest_path,
                expected_sha256=digest,
            )
            assert errors == []
            assert (
                resolved
                == (
                    resealed_root / source_extension_input_custody_path(digest)
                ).resolve()
            )
            assert (
                resolved.read_bytes()
                == expected_inputs[
                    source_extension_input_custody_path(digest).as_posix()
                ]
            )


@pytest.mark.parametrize("shared_tools", [False, True])
def test_machine_tool_roots_are_canonicalized_without_duplicating_shared_tools(
    tmp_path: Path,
    shared_tools: bool,
) -> None:
    target_bin = tmp_path / "target-sdk/bin"
    build_bin = target_bin if shared_tools else tmp_path / "native-sdk/bin"
    metadata = {
        "toolchain": {"tools": {"cc": {"path": str(target_bin / "clang")}}},
        "build_toolchain": {"tools": {"cc": {"path": str(build_bin / "clang")}}},
    }
    roots = producer._producer_location_roots(
        source_root=tmp_path / "source",
        build_root=tmp_path / "build",
        transaction_root=tmp_path / "transaction",
        metadata_payload=metadata,
        config_tools=(),
    )
    canonical = producer._canonicalize_locations(metadata, roots)
    assert canonical["toolchain"]["tools"]["cc"]["path"] == "@llvm-bin/clang"
    expected_build = "@llvm-bin/clang" if shared_tools else "@build-llvm-bin/clang"
    assert canonical["build_toolchain"]["tools"]["cc"]["path"] == expected_build
    assert len([path for path, _token in roots if path == target_bin]) == 1


def test_stage_build_metadata_recomputes_canonical_leaf_and_identity_digests(
    tmp_path: Path,
) -> None:
    transaction = tmp_path / "transaction"
    publish = transaction / "publish"
    metadata_root = transaction / "target-metadata"
    pkgconfig = metadata_root / "pkgconfig"
    pkgconfig.mkdir(parents=True)
    (pkgconfig / "python3.pc").write_text(f"prefix={transaction}\n", encoding="utf-8")
    (metadata_root / "meson.cross").write_text(
        f"[properties]\nsys_root = '{transaction}'\n", encoding="utf-8"
    )
    (metadata_root / "meson.native").write_text(
        f"[binaries]\nc = '{transaction}/clang'\n", encoding="utf-8"
    )
    (metadata_root / "source-extension-target-metadata.json").write_text(
        "{}\n", encoding="utf-8"
    )
    meson = tmp_path / "meson"
    meson.mkdir()
    intro = meson / "intro-targets.json"
    commands = meson / "compile_commands.json"
    installed = meson / "intro-installed.json"
    intro.write_text("[]\n", encoding="utf-8")
    commands.write_text("[]\n", encoding="utf-8")
    installed.write_text("{}\n", encoding="utf-8")
    raw_payload = {
        "schema_version": SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION,
        "kind": "molt-source-extension-target-metadata",
        "target_triple": "wasm32-wasip1",
        "target": {
            "requested": "wasm",
            "compiler_target_triple": "wasm32-wasip1",
            "artifact_kind": "wasm_relocatable_object",
        },
        "python": {"implementation": "cpython", "version": "3.12"},
        "paths": {"out_dir": str(metadata_root)},
        "digests": {
            "python_pc_sha256": "stale",
            "meson_cross_sha256": "stale",
            "meson_native_sha256": "stale",
        },
        "digest": "stale",
    }

    staged, canonical = producer._stage_build_metadata(
        publish_root=publish,
        metadata_root=metadata_root,
        intro_targets=intro,
        compile_commands=commands,
        intro_installed=installed,
        config_tool_cross=None,
        target_metadata_payload=raw_payload,
        location_roots=((transaction, "@transaction"),),
    )

    assert canonical["digests"] == {
        "python_pc_sha256": producer._sha256_file(
            staged["target/pkgconfig/python3.pc"]
        ),
        "meson_cross_sha256": producer._sha256_file(staged["target/meson.cross"]),
        "meson_native_sha256": producer._sha256_file(staged["target/meson.native"]),
    }
    identity = dict(canonical)
    digest = identity.pop("digest")
    assert (
        digest
        == hashlib.sha256(
            json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    )
    assert (
        json.loads(staged["target/source-extension-target-metadata.json"].read_text())
        == canonical
    )
    assert "stale" not in json.dumps(canonical)


def test_recover_and_prune_preserves_unjournaled_transaction_family(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    destination = tmp_path / "canonical-seal"
    abandoned = [
        tmp_path / ".canonical-seal.produce-alpha",
        tmp_path / ".canonical-seal.produce-beta",
    ]
    unrelated = tmp_path / ".other-seal.produce-preserve"
    for root in (*abandoned, unrelated):
        (root / "package-store").mkdir(parents=True)
        (root / "evidence.txt").write_text("fixture\n", encoding="utf-8")
    recovered: list[Path] = []
    monkeypatch.setattr(
        publication,
        "recover_source_package_seal_commits",
        lambda root, **kwargs: recovered.append(root) or (),
    )

    with (
        _held_publication_custody(destination) as custody,
        pytest.warns(RuntimeWarning, match="without a completed publication receipt"),
    ):
        publication.recover_and_prune_source_extension_transactions(
            destination, custody=custody
        )

    assert recovered == [root / "package-store" for root in abandoned]
    assert all(root.exists() for root in abandoned)
    assert unrelated.is_dir()


def test_recover_and_prune_fails_closed_on_legacy_retired_destination(
    tmp_path: Path,
) -> None:
    destination = tmp_path / "canonical-seal"
    abandoned = tmp_path / ".canonical-seal.produce-interrupted"
    retired = abandoned / "retired-destination"
    retired.mkdir(parents=True)
    (retired / "legacy.txt").write_text("preserved\n", encoding="utf-8")

    with pytest.raises(
        publication.SourcePackageSealVerificationError,
        match="legacy source-extension transaction contains a retired canonical destination",
    ):
        with _held_publication_custody(destination) as custody:
            publication.recover_and_prune_source_extension_transactions(
                destination, custody=custody
            )
    assert (retired / "legacy.txt").read_text(encoding="utf-8") == "preserved\n"
    assert not destination.exists()
