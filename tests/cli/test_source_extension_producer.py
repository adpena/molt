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

import pytest

from molt.cli import entrypoint_dispatch, entrypoint_parser
from molt.cli import source_build_environment as build_environment
from molt.cli import source_extension_producer as producer
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.extension_wheel import _write_extension_wheel
from molt.cli.source_extension_publication import (
    _source_extension_publication_custody,
)
from molt.scientific_stack_versions import (
    ScientificExtensionSet,
    ScientificExtensionSpec,
)


_MODULES = (
    "scipy.ndimage._nd_image",
    "scipy.ndimage._ni_label",
    "scipy.ndimage._rank_filter_1d",
    "scipy._lib._ccallback_c",
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


class _Distribution:
    def __init__(self, name: str, version: str) -> None:
        self.version = version
        self.metadata = {"Name": name}


def _write_distribution_metadata(root: Path, name: str, version: str) -> None:
    metadata = root / f"{name.replace('-', '_')}-{version}.dist-info" / "METADATA"
    metadata.parent.mkdir(parents=True)
    metadata.write_text(
        f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n",
        encoding="utf-8",
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
        path.write_text(f"{marker}:{module}", encoding="utf-8")
        artifact_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
        source_sha256 = hashlib.sha256(f"{path.stem}.c".encode()).hexdigest()
        manifest = {
            "module": module,
            "extension_sha256": artifact_sha256,
            "wheel_sha256": f"wheel-{module}",
            "object_closure": {
                "closure_sha256": f"closure-{module}",
                "objects": [
                    {
                        "source": f"{path.stem}.c",
                        "object": "0.o",
                        "source_sha256": source_sha256,
                        "object_sha256": "a" * 64,
                        "defined_symbols": [],
                        "undefined_symbols": [],
                        "compile_command": ["clang"],
                        "symbol_command": ["llvm-nm"],
                        "dependencies": [],
                    }
                ],
            },
        }
        producer._compact_source_extension_manifest(manifest)
        path.with_name(path.name + ".extension_manifest.json").write_text(
            json.dumps(manifest),
            encoding="utf-8",
        )


def _write_target_metadata(root: Path) -> dict[str, object]:
    target_root = root / "provenance/metadata/target"
    python_pc = target_root / "pkgconfig/python3.pc"
    meson_cross = target_root / "meson.cross"
    python_pc.parent.mkdir(parents=True, exist_ok=True)
    python_pc.write_text("prefix=@molt\n", encoding="utf-8")
    meson_cross.write_text("[binaries]\n", encoding="utf-8")
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
    metadata: dict[str, object] = {
        "schema_version": 2,
        "toolchain": {"tools": tools, "commands": commands},
        "digests": {
            "python_pc_sha256": producer._sha256_file(python_pc),
            "meson_cross_sha256": producer._sha256_file(meson_cross),
        },
    }
    encoded = json.dumps(metadata, sort_keys=True, separators=(",", ":")).encode()
    metadata["digest"] = hashlib.sha256(encoded).hexdigest()
    (target_root / "source-extension-target-metadata.json").write_text(
        json.dumps(metadata), encoding="utf-8"
    )
    return metadata


def test_produce_set_parser_has_no_partial_or_nondeterministic_lane() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "extension",
            "produce-set",
            "--package",
            "scipy",
            "--module-set",
            "pact-witness",
            "--source",
            "repos/scipy",
            "--build-root",
            "build/scipy-wasm",
            "--json",
        ]
    )

    assert args.extension_command == "produce-set"
    assert args.package == "scipy"
    assert args.module_set == "pact-witness"
    assert args.target == "wasm"
    assert args.abi_tier == "cpython-abi"
    assert args.expected_identity_sha256 is None
    assert args.expected_candidate_identity_sha256 is None
    assert not hasattr(args, "module")
    assert not hasattr(args, "deterministic")


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
            "--module-set",
            "pact-witness",
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
            "module_set": "pact-witness",
            "source": "repos/scipy",
            "build_root": "build/scipy-wasm",
            "target": "wasm",
            "abi_tier": "cpython-abi",
            "expected_identity_sha256": None,
            "expected_candidate_identity_sha256": None,
            "json_output": False,
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
        command=(sys.executable, "-m", "ninja"),
        manifest={"distribution": "ninja"},
    )
    monkeypatch.setattr(
        producer.commands,
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
        abi_tier="cpython-abi",
        tool_commands={"ld": ("/tools/wasm-ld",)},
        backend=backend,
    )

    assert actual is expected
    assert len(calls) == 1
    assert calls[0]["deterministic"] is True
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
    current_abi = producer._default_molt_c_api_version(producer._REPO_ROOT)
    manifest = {
        "deterministic": True,
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "target_triple": "wasm32-wasip1",
        "abi_tier": "cpython-abi",
        "molt_c_api_version": current_abi,
        "abi_tag": f"molt_abi{current_abi.split('.', 1)[0]}",
    }

    producer._audit_producer_contract(manifest, module="scipy.ndimage._nd_image")
    manifest["deterministic"] = False
    with pytest.raises(producer.SourceExtensionProducerError, match="deterministic"):
        producer._audit_producer_contract(manifest, module="scipy.ndimage._nd_image")


def test_source_build_environment_noop_records_exact_resolutions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    requirements = (
        "meson>=1.5",
        "Cython>=3.0",
        'ninja; python_version < "3"',
    )
    _write_build_pyproject(tmp_path, requirements)
    installed = {
        "meson": _Distribution("meson", "1.8.0"),
        "Cython": _Distribution("Cython", "3.1.2"),
    }
    monkeypatch.setattr(
        producer.importlib_metadata,
        "distribution",
        lambda name: installed[name],
    )
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail(
            "satisfied requirements must not install"
        ),
    )

    environment = producer._ensure_source_build_environment(
        tmp_path, custody={"environment_id": "test"}
    )

    assert environment.manifest_payload() == {
        "python": {
            "implementation": producer.sys.implementation.name,
            "version": (
                f"{producer.sys.version_info.major}.{producer.sys.version_info.minor}."
                f"{producer.sys.version_info.micro}"
            ),
            "executable": Path(producer.sys.executable).name,
        },
        "requirements": list(requirements),
        "marker_environment": producer.canonical_source_marker_environment(),
        "active_requirements": ["meson>=1.5", "Cython>=3.0"],
        "resolved": [
            {
                "requirement": "meson>=1.5",
                "distribution": "meson",
                "version": "1.8.0",
            },
            {
                "requirement": "Cython>=3.0",
                "distribution": "Cython",
                "version": "3.1.2",
            },
        ],
        "custody": {"environment_id": "test"},
    }


def test_source_build_environment_rejects_incomplete_locked_group(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5", "Cython>=3.0"))
    versions = {"meson": "1.0", "Cython": "3.1.2"}

    def distribution(name: str) -> _Distribution:
        return _Distribution(name, versions[name])

    monkeypatch.setattr(producer.importlib_metadata, "distribution", distribution)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="configured dependency group or frozen lock is incomplete.*meson>=1.5",
    ):
        producer._ensure_source_build_environment(
            tmp_path, custody={"environment_id": "test"}
        )


def test_source_build_environment_missing_distribution_is_fail_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5",))

    def missing(_name: str):
        raise producer.importlib_metadata.PackageNotFoundError

    monkeypatch.setattr(producer.importlib_metadata, "distribution", missing)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="configured dependency group or frozen lock is incomplete.*meson>=1.5",
    ):
        producer._ensure_source_build_environment(
            tmp_path, custody={"environment_id": "test"}
        )


def _locked_environment_spec(
    tmp_path: Path,
) -> tuple[Path, Path, Path, dict[str, object], Path]:
    root = tmp_path / "custody/environment"
    python = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    custody: dict[str, object] = {
        "schema_version": 2,
        "environment_id": "a" * 64,
        "dependency_group": "source-build-numpy",
        "dependency_group_requirements": ["ninja==1.13.0"],
        "uv_lock_sha256": "b" * 64,
        "python": {
            "implementation": "cpython",
            "version": "3.12.13",
            "platform": "win-amd64",
            "base_executable": "python.exe",
            "base_executable_sha256": "c" * 64,
        },
        "uv": {
            "executable": "uv.exe",
            "version": "uv 0.11.24",
            "sha256": "d" * 64,
        },
    }
    return (
        root,
        python,
        root / build_environment.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
        custody,
        tmp_path / "uv.exe",
    )


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
        lambda: {
            "implementation": "cpython",
            "version": "3.12.13",
            "platform": "win-amd64",
            "base_executable": "python.exe",
            "base_executable_sha256": "c" * 64,
        },
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
    assert custody["schema_version"] == 2
    address_payload = {key: custody[key] for key in custody if key != "environment_id"}
    assert (
        custody["environment_id"]
        == hashlib.sha256(
            json.dumps(address_payload, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    )
    old_address_payload = dict(address_payload)
    old_address_payload["schema_version"] = 1
    assert (
        custody["environment_id"]
        != hashlib.sha256(
            json.dumps(
                old_address_payload, sort_keys=True, separators=(",", ":")
            ).encode()
        ).hexdigest()
    )


def test_source_build_environment_failed_sync_leaves_only_provisional_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 7),
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="provisioning failed.*returned 7",
    ):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
        )

    assert not spec[2].exists()
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    assert json.loads(provisioning_path.read_text(encoding="utf-8")) == (
        build_environment._provisioning_record(spec[3])
    )


def test_source_build_environment_recovers_exact_provisional_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    calls = 0
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    distributions = [{"name": "ninja", "version": "1.13.0"}]
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_distributions",
        lambda _python: distributions,
    )

    def run(argv, **kwargs):
        nonlocal calls
        calls += 1
        if calls == 1:
            return subprocess.CompletedProcess(argv, 7)
        environment_root = Path(kwargs["environment"]["UV_PROJECT_ENVIRONMENT"])
        assert environment_root == spec[0]
        environment_python = environment_root / (
            "Scripts/python.exe" if os.name == "nt" else "bin/python"
        )
        environment_python.parent.mkdir(parents=True, exist_ok=True)
        environment_python.write_bytes(b"python")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(build_environment, "_run_uv_sync", run)

    with pytest.raises(build_environment.SourceBuildEnvironmentError):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
        )
    result = build_environment.provision_source_build_environment(
        tmp_path, "source-build-numpy"
    )

    assert calls == 2
    assert result.root == spec[0]
    assert json.loads(spec[2].read_text(encoding="utf-8")) == {
        **spec[3],
        "installed_distributions": distributions,
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
    spec = _locked_environment_spec(tmp_path)
    spec[0].mkdir(parents=True)
    if foreign_payload is not None:
        spec[2].write_text(json.dumps(foreign_payload), encoding="utf-8")
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: pytest.fail("foreign root must never be mutated"),
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="exact attestation or sibling provisioning record",
    ):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
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
    spec = _locked_environment_spec(tmp_path)
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text(json.dumps(foreign_payload), encoding="utf-8")
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="foreign source-build provisioning record",
    ):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
        )


def test_source_build_environment_rejects_malformed_sibling_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text("{not-json", encoding="utf-8")
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="malformed source-build provisioning record",
    ):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
        )


def test_installed_distributions_use_only_canonical_sysconfig_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    purelib = tmp_path / "environment" / "purelib"
    platlib = tmp_path / "environment" / "platlib"
    poison = tmp_path / "external"
    _write_distribution_metadata(purelib, "canonical-pure", "1.2.3")
    _write_distribution_metadata(platlib, "canonical-plat", "4.5.6")
    _write_legacy_distribution_metadata(poison, "ambient-poison", "99")
    monkeypatch.syspath_prepend(str(poison))
    monkeypatch.setenv("PYTHONPATH", str(poison))

    def get_path(scheme: str, *_args, **_kwargs) -> str:
        return str({"purelib": purelib, "platlib": platlib}[scheme])

    monkeypatch.setattr(build_environment.sysconfig, "get_path", get_path)

    assert build_environment._installed_distributions() == [
        {"name": "canonical-plat", "version": "4.5.6"},
        {"name": "canonical-pure", "version": "1.2.3"},
    ]


def test_distribution_probe_sanitizes_python_import_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: dict[str, object] = {}
    monkeypatch.setenv("PYTHONHOME", "poison-home")
    monkeypatch.setenv("PYTHONPATH", "poison-path")

    def run(argv, **kwargs):
        observed["argv"] = argv
        observed["env"] = kwargs["env"]
        return subprocess.CompletedProcess(argv, 0, stdout="[]", stderr="")

    monkeypatch.setattr(build_environment.subprocess, "run", run)

    assert (
        build_environment._probe_environment_distributions(Path(sys.executable)) == []
    )
    assert observed["argv"][:3] == [str(Path(sys.executable)), "-P", "-c"]
    environment = observed["env"]
    assert isinstance(environment, dict)
    assert "PYTHONHOME" not in environment
    assert "PYTHONPATH" not in environment
    assert environment["PYTHONNOUSERSITE"] == "1"


def test_distribution_probe_excludes_external_pythonpath_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    poison = tmp_path / "external"
    _write_legacy_distribution_metadata(poison, "ambient-probe-poison", "99")
    monkeypatch.setenv("PYTHONPATH", str(poison))

    rows = build_environment._probe_environment_distributions(Path(sys.executable))

    assert not any(row["name"] == "ambient-probe-poison" for row in rows)


def test_complete_environment_cleans_exact_stale_sibling_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    distributions = [{"name": "ninja", "version": "1.13.0"}]
    spec[1].parent.mkdir(parents=True)
    spec[1].write_bytes(b"python")
    spec[2].write_text(
        json.dumps({**spec[3], "installed_distributions": distributions}),
        encoding="utf-8",
    )
    provisioning_path = build_environment._provisioning_record_path(spec[0])
    provisioning_path.parent.mkdir(parents=True)
    provisioning_path.write_text(
        json.dumps(build_environment._provisioning_record(spec[3])),
        encoding="utf-8",
    )
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_distributions",
        lambda _python: distributions,
    )
    monkeypatch.setattr(
        build_environment,
        "_run_uv_sync",
        lambda *_args, **_kwargs: pytest.fail("complete root must not reprovision"),
    )

    result = build_environment.provision_source_build_environment(
        tmp_path, "source-build-numpy"
    )

    assert result.root == spec[0]
    assert not provisioning_path.exists()


def test_concurrent_source_build_provision_runs_one_sync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    calls: list[tuple[str, ...]] = []
    calls_lock = threading.Lock()
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    distributions = [{"name": "ninja", "version": "1.13.0"}]
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_distributions",
        lambda _python: distributions,
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
            build_environment.provision_source_build_environment(
                tmp_path, "source-build-numpy"
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
        "--group",
        "source-build-numpy",
        "--no-install-project",
    )
    assert json.loads(spec[2].read_text(encoding="utf-8")) == {
        **spec[3],
        "installed_distributions": distributions,
    }
    launcher = spec[1].parent / ("cython.exe" if os.name == "nt" else "cython")
    assert launcher.read_text(encoding="utf-8") == f"#!{spec[1]}\n"
    assert ".provision-" not in launcher.read_text(encoding="utf-8")


def test_source_build_provision_rejects_group_resolution_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    monkeypatch.setattr(
        build_environment,
        "_probe_environment_distributions",
        lambda _python: [{"name": "packaging", "version": "26.2"}],
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
        match="does not satisfy.*ninja==1.13.0",
    ):
        build_environment.provision_source_build_environment(
            tmp_path, "source-build-numpy"
        )

    assert not spec[2].exists()
    assert json.loads(
        build_environment._provisioning_record_path(spec[0]).read_text(encoding="utf-8")
    ) == (build_environment._provisioning_record(spec[3]))


def test_active_source_build_environment_rejects_mutated_ambient_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
    spec[0].mkdir(parents=True)
    manifest = {**spec[3], "installed_distributions": []}
    spec[2].write_text(json.dumps(manifest), encoding="utf-8")
    monkeypatch.setattr(build_environment, "_environment_spec", lambda *_args: spec)
    monkeypatch.setattr(build_environment.sys, "prefix", str(spec[0]))
    monkeypatch.setattr(
        build_environment,
        "_installed_distributions",
        lambda: [{"name": "ambient-drift", "version": "1"}],
    )

    with pytest.raises(
        build_environment.SourceBuildEnvironmentError,
        match="attestation is stale or invalid",
    ):
        build_environment.source_build_environment(tmp_path, "source-build-numpy")


def test_source_build_reexec_uses_typed_args_and_invoking_worktree_src(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    spec = _locked_environment_spec(tmp_path)
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

    monkeypatch.setattr(producer.subprocess, "run", run)
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
        package="numpy",
        module_set="pact-witness",
        source="source-root",
        build_root="build-root",
        target="wasm",
        abi_tier="cpython-abi",
        json_output=True,
    )

    assert result == 19
    assert observed["argv"] == [
        str(spec[1]),
        "-P",
        "-m",
        "molt.cli",
        "extension",
        "produce-set",
        "--package",
        "numpy",
        "--module-set",
        "pact-witness",
        "--source",
        "source-root",
        "--build-root",
        "build-root",
        "--target",
        "wasm",
        "--abi-tier",
        "cpython-abi",
        "--json",
    ]
    assert observed["check"] is False
    assert "capture_output" not in observed
    child_environment = observed["env"]
    assert isinstance(child_environment, dict)
    assert child_environment["PYTHONPATH"] == str(
        (producer._REPO_ROOT / "src").resolve()
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


def test_producer_never_accepts_ambient_environment_or_locks_before_reexec(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    spec = _locked_environment_spec(tmp_path)
    inactive = build_environment.LockedSourceBuildEnvironment(
        root=spec[0],
        python_executable=spec[1],
        manifest_path=spec[2],
        custody=spec[3],
        active=False,
    )
    monkeypatch.setattr(producer, "source_build_environment", lambda *_args: inactive)
    monkeypatch.setattr(
        producer, "provision_source_build_environment", lambda *_args: inactive
    )
    monkeypatch.setattr(
        producer,
        "_run_locked_source_extension_producer",
        lambda *_args, **_kwargs: 23,
    )
    monkeypatch.setattr(
        producer,
        "_acquire_file_lock",
        lambda *_args, **_kwargs: pytest.fail(
            "parent must not hold producer publication lock across re-exec"
        ),
    )

    assert (
        producer.produce_source_extension_set(
            package="numpy",
            module_set="pact-witness",
            source=str(source),
            build_root=str(tmp_path / "build"),
        )
        == 23
    )


def test_meson_setup_uses_typed_driver(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[str, ...]] = []

    def run_process(argv, *, cwd):
        assert cwd == tmp_path / "source"
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
        setup_args=("-Dblas=none",),
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

    resolved = producer._source_meson_driver(source)

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
        command=("ninja",),
        manifest={"distribution": "ninja"},
    )

    materialized = producer._materialize_generated_inputs(
        backend=backend,
        source_root=source,
        build_root=build,
        intro_targets=build / "meson-info/intro-targets.json",
        intro_installed=build / "meson-info/intro-installed.json",
        extension_set=ScientificExtensionSet(
            package="numpy",
            name="pact-witness",
            seal_name="numpy-witness",
            expected_identity_sha256="a" * 64,
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
            "ninja",
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
        command=(sys.executable, "-m", "ninja"),
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
        extension_set=ScientificExtensionSet(
            package="scipy",
            name="pact-witness",
            seal_name="scipy-witness",
            expected_identity_sha256="a" * 64,
            build_dependency_group="source-build-scipy",
            meson_setup_args=(),
            use_pkg_config=False,
            required_installed_files=(),
            extensions=(
                ScientificExtensionSpec(
                    module="scipy.ndimage._ni_label",
                    target="_ni_label",
                    python_exports=("scipy.ndimage._ni_label",),
                    capabilities=(),
                ),
            ),
        ),
    )

    assert missing == set()


def test_meson_pkg_config_is_pinned_and_attested(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = tmp_path / "Scripts/pkg-config.exe"
    tool.parent.mkdir()
    tool.write_bytes(b"real-tool-placeholder")
    resolved = producer._ResolvedBuildRequirement(
        requirement=producer.MOLT_PKGCONF_REQUIREMENT,
        distribution="pkgconf",
        version="3.0.1.post0",
    )
    monkeypatch.setattr(
        producer, "_installed_build_requirement", lambda *_args: resolved
    )
    monkeypatch.setattr(producer, "_active_console_script", lambda _name: tool)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="3.0.1\n", stderr=""
        ),
    )

    config_tool = producer._ensure_meson_pkg_config(tmp_path)

    assert config_tool == producer._SourceBuildConfigTool(
        name="pkg-config",
        path=tool,
        distribution="pkgconf",
        version="3.0.1.post0",
    )


def test_meson_pkg_config_missing_does_not_install_into_active_interpreter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(producer, "_installed_build_requirement", lambda *_args: None)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda *_args, **_kwargs: pytest.fail("producer must not invoke an installer"),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError,
        match="never installs into its active interpreter",
    ):
        producer._ensure_meson_pkg_config(tmp_path)


def test_meson_config_tool_cross_is_generic_and_deterministic(tmp_path: Path) -> None:
    tools = (
        producer._SourceBuildConfigTool(
            name="pybind11-config",
            path=tmp_path / "Scripts/pybind11-config.exe",
            distribution="pybind11",
            version="3.0.4",
        ),
        producer._SourceBuildConfigTool(
            name="numpy-config",
            path=tmp_path / "Scripts/numpy-config.exe",
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
        if "foreach" in argv:
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
    extension_set = ScientificExtensionSet(
        package="scipy",
        name="pact-witness",
        seal_name="pact_scipy_witness",
        expected_identity_sha256="a" * 64,
        build_dependency_group="source-build-scipy",
        meson_setup_args=(),
        use_pkg_config=True,
        required_installed_files=(),
        extensions=tuple(
            ScientificExtensionSpec(
                module=module,
                target=module.rsplit(".", 1)[-1],
                python_exports=(module,),
                capabilities=(),
            )
            for module in _MODULES
        ),
    )
    set_manifest = {
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
                "wheel_sha256": f"wheel-{spec.module}",
                "object_closure_sha256": f"closure-{spec.module}",
            }
            for spec in extension_set.extensions
        ],
    }
    producer._validate_complete_publish_root(
        publish_root=publish,
        extension_set=extension_set,
        set_manifest=set_manifest,
    )
    set_manifest["extensions"][0]["target"] = "wrong-target"
    with pytest.raises(
        producer.SourceExtensionProducerError, match="typed extension contracts"
    ):
        producer._validate_complete_publish_root(
            publish_root=publish,
            extension_set=extension_set,
            set_manifest=set_manifest,
        )
    set_manifest["extensions"][0]["target"] = extension_set.extensions[0].target
    duplicate = (
        publish / ".duplicate/scipy/ndimage/_nd_image.molt.wasm.extension_manifest.json"
    )
    duplicate.parent.mkdir(parents=True)
    duplicate.write_text("{}", encoding="utf-8")

    with pytest.raises(producer.SourceExtensionProducerError, match="unexpected"):
        producer._validate_complete_publish_root(
            publish_root=publish,
            extension_set=extension_set,
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
    for path, content in (
        (
            source,
            b"int source_symbol(void) { return 1; }\n"
            b"int import_numpy(void) { return _import_array(); }\n",
        ),
        (generated, b"int PyInit__nd_image(void) { return 2; }\n"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
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
        "schema_version": 1,
        "root_symbol": "PyInit__nd_image",
        "init_symbol_owner": "1.o",
        "runtime_symbols": [],
        "objects": [
            {
                "source": str(source),
                "object": "0.o",
                "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                "object_sha256": "1" * 64,
                "defined_symbols": ["source_symbol"],
                "undefined_symbols": [],
                "compile_command": ["clang", "-c", str(source)],
                "symbol_command": ["llvm-nm"],
                "dependencies": [],
            },
            {
                "source": str(generated),
                "object": "1.o",
                "source_sha256": hashlib.sha256(generated.read_bytes()).hexdigest(),
                "object_sha256": "2" * 64,
                "defined_symbols": ["PyInit__nd_image"],
                "undefined_symbols": [],
                "compile_command": ["clang", "-c", str(generated)],
                "symbol_command": ["llvm-nm"],
                "dependencies": [],
            },
        ],
    }
    manifest = {
        "module": module,
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
        "build": {"source_plan_digest": "stale", "object_closure_sha256": "stale"},
        "object_closure": closure,
    }
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
        object_closure_sha256="stale",
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
    assert "source_root" not in staged_manifest["source_plan"]
    assert "build_root" not in staged_manifest["source_plan"]
    assert "compile_units" not in staged_manifest["source_plan"]
    assert "generated_sources" not in staged_manifest["source_plan"]
    assert staged_manifest["sources"] == [
        item["source"] for item in staged_manifest["object_closure"]["objects"]
    ]
    expected_capsule = "numpy.core._multiarray_umath._ARRAY_API"
    assert staged_manifest["object_closure"]["required_capsules"] == [expected_capsule]
    assert producer._manifest_sequence(
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
        assert embedded["extension"] == "_nd_image.molt.wasm"
        assert archive.read(embedded["extension"]) == staged.artifact_path.read_bytes()
        assert "scipy/ndimage/_nd_image.molt.wasm" not in archive.namelist()
        assert str(tmp_path) not in json.dumps(embedded)
        assert embedded["extension_sha256"] == staged_manifest["extension_sha256"]
        assert embedded["object_closure"]["required_capsules"] == [expected_capsule]
        assert producer._manifest_sequence(
            embedded,
            embedded["object_closure"]["objects"][0],
            "required_capsules",
        ) == [expected_capsule]
        assert embedded["object_closure"]["objects"][0]["source"].startswith("@source/")
        assert embedded["object_closure"]["objects"][1]["source"].startswith("@build/")
        assert all(
            info.date_time == (1980, 1, 1, 0, 0, 0) for info in archive.infolist()
        )
    assert staged_manifest["object_closure"]["closure_sha256"] == (
        staged.object_closure_sha256
    )


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
        f"sys_root = '{transaction}'\n", encoding="utf-8"
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
        "schema_version": 2,
        "paths": {"out_dir": str(metadata_root)},
        "digests": {
            "python_pc_sha256": "stale",
            "meson_cross_sha256": "stale",
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


def test_recover_and_prune_producer_transactions_removes_whole_abandoned_family(
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
        producer,
        "recover_source_package_seal_commits",
        lambda root: recovered.append(root) or (),
    )

    with _held_publication_custody(destination) as custody:
        producer._recover_and_prune_producer_transactions(
            destination, publication_custody=custody
        )

    assert recovered == [root / "package-store" for root in abandoned]
    assert not any(root.exists() for root in abandoned)
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
        producer.SourceExtensionProducerError,
        match="legacy producer transaction contains a retired canonical destination",
    ):
        with _held_publication_custody(destination) as custody:
            producer._recover_and_prune_producer_transactions(
                destination, publication_custody=custody
            )
    assert (retired / "legacy.txt").read_text(encoding="utf-8") == "preserved\n"
    assert not destination.exists()
