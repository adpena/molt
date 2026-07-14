from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path

import pytest

from molt.cli import entrypoint_dispatch, entrypoint_parser
from molt.cli import source_extension_producer as producer
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


class _Distribution:
    def __init__(self, name: str, version: str) -> None:
        self.version = version
        self.metadata = {"Name": name}


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
        path.with_name(path.name + ".extension_manifest.json").write_text(
            json.dumps(
                {
                    "module": module,
                    "extension_sha256": artifact_sha256,
                    "wheel_sha256": f"wheel-{module}",
                    "object_closure": {"closure_sha256": f"closure-{module}"},
                }
            ),
            encoding="utf-8",
        )


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
        path.write_text(f"# {relative}\n", encoding="utf-8")
        installed[str(path)] = f"C:/prefix/Lib/site-packages/{relative}"
    unrelated = source / "array_api_extra/tests/__init__.py"
    unrelated.parent.mkdir(parents=True)
    unrelated.write_text("# unrelated subproject\n", encoding="utf-8")
    installed[str(unrelated)] = (
        "C:/prefix/Lib/site-packages/array_api_extra/tests/__init__.py"
    )
    intro = build / "meson-info" / "intro-installed.json"
    intro.parent.mkdir(parents=True)
    intro.write_text(json.dumps(installed), encoding="utf-8")

    staged = producer._stage_installed_python_files(
        intro_installed=intro,
        source_root=source,
        build_root=build,
        package="scipy",
        publish_root=publish,
    )

    assert len(staged) == 4
    assert (publish / "scipy/version.py").read_text(encoding="utf-8") == (
        "# scipy/version.py\n"
    )
    assert (publish / "scipy/__config__.py").is_file()
    assert not (publish / "array_api_extra").exists()


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
        producer._stage_installed_python_files(
            intro_installed=intro,
            source_root=source,
            build_root=build,
            package="scipy",
            publish_root=publish,
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
        target="wasm",
        abi_tier="cpython-abi",
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

    environment = producer._ensure_source_build_environment(tmp_path)

    assert environment.manifest_payload() == {
        "python_executable": producer.sys.executable,
        "requirements": list(requirements),
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
    }


def test_source_build_environment_installs_only_unsatisfied_requirements(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5", "Cython>=3.0"))
    versions = {"meson": "1.0", "Cython": "3.1.2"}

    def distribution(name: str) -> _Distribution:
        return _Distribution(name, versions[name])

    calls: list[tuple[str, ...]] = []

    def run_process(argv, *, cwd):
        assert cwd == tmp_path
        calls.append(tuple(argv))
        versions["meson"] = "1.8.0"
        return subprocess.CompletedProcess(
            args=list(argv), returncode=0, stdout="installed", stderr=""
        )

    monkeypatch.setattr(producer.importlib_metadata, "distribution", distribution)
    monkeypatch.setattr(producer, "_run_process", run_process)

    environment = producer._ensure_source_build_environment(tmp_path)

    assert calls == [
        (
            producer.sys.executable,
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "meson>=1.5",
        )
    ]
    assert [item.version for item in environment.resolved] == ["1.8.0", "3.1.2"]


def test_source_build_environment_install_failure_is_fail_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_build_pyproject(tmp_path, ("meson>=1.5",))

    def missing(_name: str):
        raise producer.importlib_metadata.PackageNotFoundError

    monkeypatch.setattr(producer.importlib_metadata, "distribution", missing)
    monkeypatch.setattr(
        producer,
        "_run_process",
        lambda argv, *, cwd: subprocess.CompletedProcess(
            args=list(argv), returncode=1, stdout="", stderr="index unavailable"
        ),
    )

    with pytest.raises(
        producer.SourceExtensionProducerError, match="index unavailable"
    ):
        producer._ensure_source_build_environment(tmp_path)


def test_meson_setup_uses_active_interpreter_module(
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
        meson_setup_args=(),
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
        "extensions": [
            {
                "module": spec.module,
                "target": spec.target,
                "capabilities": list(spec.capabilities),
                "artifact_sha256": hashlib.sha256(
                    publish.joinpath(*spec.module.split("."))
                    .with_suffix(".molt.wasm")
                    .read_bytes()
                ).hexdigest(),
                "wheel_sha256": f"wheel-{spec.module}",
                "object_closure_sha256": f"closure-{spec.module}",
            }
            for spec in extension_set.extensions
        ]
    }
    producer._validate_complete_publish_root(
        publish_root=publish,
        extension_set=extension_set,
        set_manifest=set_manifest,
    )
    set_manifest["extensions"][0]["target"] = "wrong-target"
    with pytest.raises(
        producer.SourceExtensionProducerError, match="module/target/capability"
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


def test_extension_set_publication_exposes_all_four_together(tmp_path: Path) -> None:
    destination = tmp_path / "pact-witness"
    staged = tmp_path / "transaction" / "publish"
    _write_complete_root(destination, marker="old")
    _write_complete_root(staged, marker="new")

    producer._publish_directory_atomically(staged, destination)

    assert (destination / "marker.txt").read_text(encoding="utf-8") == "new"
    assert all(
        destination.joinpath(*module.split(".")).with_suffix(".molt.wasm").is_file()
        for module in _MODULES
    )
    assert len(list(destination.glob("**/*.molt.wasm.extension_manifest.json"))) == 4
    assert all(
        destination.joinpath(*module.split("."))
        .with_suffix(".molt.wasm")
        .read_text(encoding="utf-8")
        == f"new:{module}"
        for module in _MODULES
    )
    assert not staged.exists()


def test_extension_set_publication_failure_restores_old_complete_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "pact-witness"
    staged = tmp_path / "transaction" / "publish"
    _write_complete_root(destination, marker="old")
    _write_complete_root(staged, marker="new")
    real_replace = os.replace

    def fail_new_root(source: str | Path, target: str | Path) -> None:
        if Path(source) == staged and Path(target) == destination:
            raise OSError("injected publication failure")
        real_replace(source, target)

    monkeypatch.setattr(producer.os, "replace", fail_new_root)

    with pytest.raises(OSError, match="injected publication failure"):
        producer._publish_directory_atomically(staged, destination)

    assert (destination / "marker.txt").read_text(encoding="utf-8") == "old"
    assert all(
        destination.joinpath(*module.split("."))
        .with_suffix(".molt.wasm")
        .read_text(encoding="utf-8")
        == f"old:{module}"
        for module in _MODULES
    )
    assert (staged / "marker.txt").read_text(encoding="utf-8") == "new"
