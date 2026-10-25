"""Recipe identity must not depend on a launcher's imported native modules."""

from __future__ import annotations

import json
import shutil
from pathlib import Path
import subprocess
import sys

import pytest

from molt import source_build_environment as authority
from molt import source_build_environment_schema as authority_schema
from tests.python_environment_test_support import (
    copy_python_capture_authority,
    create_test_venv,
    runtime_identity_manifest,
    wheel_target_manifest,
)
from tests.process_guard_common import install_module_view


@pytest.mark.slow
def test_cross_checkout_probe_admits_selected_editable_reader_and_refuses_foreign_home(
    tmp_path,
):
    from molt import python_environment_identity as identity
    from molt.python_environment_location import _site_roots

    selected = tmp_path / "selected-\u03bb"
    copy_python_capture_authority(selected)
    foreign = tmp_path / "foreign"
    shutil.copytree(selected, foreign)
    # All three reader implementations are byte-identical. Only their selected
    # package homes differ; content equality cannot admit a foreign package.
    source = Path(identity.__file__)
    for checkout in (selected, foreign):
        assert (checkout / "src/molt/python_environment_identity.py").read_bytes() == (
            source.read_bytes()
        )
    root = tmp_path / "environment"
    executable = create_test_venv(root)
    (site,) = _site_roots(root)
    metadata = site / "molt_probe_fixture-1.0.dist-info"
    metadata.mkdir()
    (metadata / "METADATA").write_text(
        "Metadata-Version: 2.1\nName: molt-probe-fixture\nVersion: 1.0\n",
        encoding="utf-8",
    )
    (metadata / "WHEEL").write_text(
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        encoding="utf-8",
    )
    direct_url = metadata / "direct_url.json"
    direct_url.write_text(
        json.dumps({"url": selected.as_uri(), "dir_info": {"editable": True}}),
        encoding="utf-8",
    )
    declaration = site / "molt_probe_fixture.pth"
    declaration.write_text(str(selected / "src") + "\n", encoding="utf-8")
    (metadata / "RECORD").write_text(
        "".join(
            f"{path},,\n"
            for path in (
                declaration.name,
                *(
                    f"{metadata.name}/{name}"
                    for name in ("METADATA", "WHEEL", "direct_url.json", "RECORD")
                ),
            )
        ),
        encoding="utf-8",
    )
    # A .pth directory loads its genuine sitecustomize before the probe. The
    # old controller-bound launch must therefore refuse B before capture.
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError,
        match="package source admission failed",
    ):
        authority._probe_environment_identity(executable, root, external_root=selected)
    captured = authority._probe_environment_identity(
        executable, root, external_root=selected, source_root=selected
    )
    assert captured["external_roots"] == [
        {"id": "external-root-0", "path": str(selected.resolve())}
    ]
    assert [row["name"] for row in captured["distributions"]] == ["molt-probe-fixture"]
    assert not list(selected.rglob("__pycache__"))
    declaration.write_text(str(foreign / "src") + "\n", encoding="utf-8")
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError,
        match="package source admission failed",
    ) as caught:
        authority._probe_environment_identity(
            executable, root, external_root=foreign, source_root=selected
        )
    assert str(foreign / "src/molt") in str(caught.value)
    assert str(selected / "src/molt") in str(caught.value)


@pytest.mark.parametrize("failure", [None, "resolve", "identity", "runtime", "markers"])
def test_selected_recipe_keeps_one_empty_prefix_through_all_probes(
    tmp_path, monkeypatch, failure
):
    from molt import python_interpreter

    (tmp_path / ".python-version").write_text("3.14\n", encoding="utf-8")
    executable = tmp_path / "selected-python"
    runtime = runtime_identity_manifest()
    markers = authority_schema.canonical_source_marker_environment()
    prefixes = []
    stages = []
    original = RuntimeError("independent selected recipe failure")

    def observe(stage, prefix):
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)
        stages.append(stage)
        assert all(value == prefix for value in prefixes)
        if failure == stage:
            raise original

    def resolve(selector, *, env, cwd, verifier_bytecode_prefix):
        assert selector == "3.14" and cwd == tmp_path
        assert env["UV_PYTHON_DOWNLOADS"] == "never"
        observe("resolve", verifier_bytecode_prefix)
        return ("py", "-3.14")

    def identity(command, *, env, cwd, verifier_bytecode_prefix):
        assert command == ("py", "-3.14")
        observe("identity", verifier_bytecode_prefix)
        return python_interpreter.PythonInterpreter(
            command, str(executable), "3.14.1", "CPython"
        )

    def capture(selected, *, verifier_bytecode_prefix, capture_wheel_target):
        assert selected == executable
        assert capture_wheel_target
        observe("runtime", verifier_bytecode_prefix)
        return {"runtime": runtime, "wheel_target": wheel_target_manifest(runtime)}

    def marker_probe(command, **_kwargs):
        prefix = prefixes[0]
        assert command[:8] == [
            str(executable),
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-S",
            "-c",
            command[7],
        ]
        observe("markers", prefix)
        return subprocess.CompletedProcess(command, 0, json.dumps(markers), "")

    monkeypatch.setattr(python_interpreter, "resolve_python_selector", resolve)
    monkeypatch.setattr(python_interpreter, "probe_python_command", identity)
    monkeypatch.setattr(authority, "_probe_source_build_python", capture)
    monkeypatch.setattr(authority.process_guard, "run_completed_command", marker_probe)
    if failure is None:
        assert authority._selected_python_recipe(tmp_path, None, {}) == (
            executable,
            runtime,
            markers,
            wheel_target_manifest(runtime),
        )
        assert stages == ["resolve", "identity", "runtime", "markers"]
    else:
        with pytest.raises(RuntimeError) as caught:
            authority._selected_python_recipe(tmp_path, None, {})
        assert caught.value is original
        assert stages[-1] == failure
    assert prefixes and not prefixes[0].exists()


def test_recipe_captures_selected_base_with_isolated_probe(monkeypatch, tmp_path):
    base = tmp_path / "base-python"
    base.write_bytes(b"synthetic; never executed")
    install_module_view(monkeypatch, "sys", sys, authority, _base_executable=str(base))
    monkeypatch.setenv("PYTHONHOME", "unowned-home")
    monkeypatch.setenv("PYTHONPATH", "unowned-imports")
    monkeypatch.setenv("PYTHONDONTWRITEBYTECODE", "0")
    runtime = runtime_identity_manifest()
    expected = {"runtime": runtime, "wheel_target": wheel_target_manifest(runtime)}
    calls = []

    def run(argv, **kwargs):
        verifier_prefix = Path(argv[4].split("=", 1)[1])
        assert verifier_prefix.is_dir()
        assert not list(verifier_prefix.iterdir())
        calls.append((argv, kwargs))
        return subprocess.CompletedProcess(argv, 0, json.dumps(expected), "")

    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    assert authority._python_identity() == expected
    argv, options = calls.pop()
    assert not calls
    assert argv == [
        str(base.resolve()),
        "-B",
        "-I",
        "-X",
        argv[4],
        "-S",
        str(Path(authority.python_environment_identity.__file__).resolve()),
        "--capture-runtime",
        "--wheel-tag-source",
        str(authority.packaging_tag_source()),
    ]
    assert options["timeout"] == 120
    assert "PYTHONPATH" not in options["env"]
    assert "PYTHONHOME" not in options["env"]
    assert options["env"]["PYTHONDONTWRITEBYTECODE"] == "0"
    assert options["env"]["PYTHONNOUSERSITE"] == "1"
    assert not Path(argv[4].split("=", 1)[1]).exists()


def test_runtime_probe_keeps_callers_verifier_prefix(monkeypatch, tmp_path):
    prefix = tmp_path / "verifier"
    prefix.mkdir()
    expected = runtime_identity_manifest()

    def forbidden_allocation():
        raise AssertionError("recipe already owns the verifier namespace")

    def run(command, **_kwargs):
        assert command[1:6] == ["-B", "-I", "-X", f"pycache_prefix={prefix}", "-S"]
        assert prefix.is_dir() and not list(prefix.iterdir())
        return subprocess.CompletedProcess(command, 0, json.dumps(expected), "")

    monkeypatch.setattr(
        authority.python_environment_identity,
        "python_identity_verifier_prefix",
        forbidden_allocation,
    )
    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    assert (
        authority._probe_source_build_python(
            Path("selected-python"), verifier_bytecode_prefix=prefix
        )
        == expected
    )
    assert prefix.is_dir() and not list(prefix.iterdir())


def test_realized_environment_probe_preserves_mode_with_no_bytecode(
    monkeypatch, tmp_path
):
    expected = {"realized": "environment"}
    calls = []

    def run(argv, **kwargs):
        verifier_prefix = Path(argv[4].split("=", 1)[1])
        assert verifier_prefix.is_dir()
        assert not list(verifier_prefix.iterdir())
        assert not verifier_prefix.is_relative_to(tmp_path)
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, json.dumps(expected), "")

    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        authority, "validate_python_environment_identity", lambda value: value
    )
    executable = Path("selected-python")
    assert authority._probe_environment_identity(executable, tmp_path) == expected
    assert calls == [
        [
            str(executable),
            "-B",
            "-I",
            "-X",
            calls[0][4],
            str(Path(authority.python_environment_identity.__file__).resolve()),
            "--capture-environment",
            str(tmp_path.resolve()),
            "--admit-virtualenv-bootstrap",
        ]
    ]
    assert not Path(calls[0][4].split("=", 1)[1]).exists()


@pytest.mark.parametrize("output", ["not json", '{"schema":1,"schema":2}', "{}"])
def test_recipe_probe_rejects_invalid_or_unattested_runtime(monkeypatch, output):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: subprocess.CompletedProcess(argv, 0, output, ""),
    )
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError, match="runtime probe returned"
    ):
        authority._probe_source_build_python(Path("selected-python"))


def test_recipe_probe_reports_failure_instead_of_inprocess_fallback(monkeypatch):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: subprocess.CompletedProcess(
            argv, 7, "", "capture refused"
        ),
    )
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError,
        match="runtime content: capture refused",
    ):
        authority._probe_source_build_python(Path("selected-python"))


@pytest.mark.parametrize(
    "groups, expected",
    [((), ["root-dep"]), (("bench", "dev"), ["bench-dep", "dev-dep", "root-dep"])],
)
def test_development_lock_groups_keep_root_dependencies(tmp_path, groups, expected):
    from molt.python_uv_lock_identity import selected_uv_lock_group_closure

    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='demo'\nversion='1.0'\ndependencies=['root-dep==1.0']\n"
        "[dependency-groups]\ndev=['dev-dep==1.0']\nbench=['bench-dep==1.0']\n",
        encoding="utf-8",
    )
    lock = (
        "version=1\nrevision=1\nrequires-python='>=3.12'\n"
        "[[package]]\nname='demo'\nversion='1.0'\nsource={editable='.'}\n"
        "dependencies=[{name='root-dep'}]\n"
        "[package.dev-dependencies]\ndev=[{name='dev-dep'}]\nbench=[{name='bench-dep'}]\n"
    )
    for name in ("root-dep", "dev-dep", "bench-dep"):
        lock += (
            f"[[package]]\nname='{name}'\nversion='1.0'\nsource={{registry='https://example.invalid/simple'}}\n"
            f"wheels=[{{url='https://example.invalid/{name.replace('-', '_')}-1.0-py3-none-any.whl',hash='sha256:{'a' * 64}',size=1}}]\n"
        )
    (tmp_path / "uv.lock").write_text(lock, encoding="utf-8")
    requirements = tuple(f"{group}-dep==1.0" for group in groups)
    closure = selected_uv_lock_group_closure(
        tmp_path,
        None,
        requirements,
        dependency_groups=groups,
        wheel_target=wheel_target_manifest(),
    )
    assert closure["dependency_groups"] == list(groups)
    assert [row["name"] for row in closure["packages"]] == expected


def test_editable_binding_keeps_mutable_code_out_of_image_address(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='demo'\nversion='1.0'\nreadme='README.md'\n",
        encoding="utf-8",
    )
    (tmp_path / "README.md").write_text(
        "original installed description\n", encoding="utf-8"
    )
    source = tmp_path / "demo.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    binding = authority._editable_binding(tmp_path)
    source.write_text("VALUE = 2\n", encoding="utf-8")
    assert authority._editable_binding(tmp_path) == binding
    (tmp_path / "README.md").write_text(
        "changed installed description\n", encoding="utf-8"
    )
    assert authority._editable_binding(tmp_path) != binding
    assert binding["source_root"] == str(tmp_path)
    assert "demo.py" not in binding["metadata_inputs"]


@pytest.mark.parametrize("fault", ["none", "python", "groups", "editable-root"])
def test_development_loader_binds_requested_intent_and_editable_image(tmp_path, fault):
    from molt.exact_json import canonical_json_sha256
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "checkout"
    image = materialize_development_environment(repo, tmp_path / "artifacts")
    version = str(image.custody["python_runtime"]["version"])
    groups = ()
    if fault == "python":
        version = "99.99"
    elif fault == "groups":
        groups = ("other",)
    elif fault == "editable-root":
        manifest = json.loads(image.manifest_path.read_text(encoding="utf-8"))
        projection = manifest["realized_image"]
        # A self-consistent checksum cannot give a different checkout ownership
        # of this generation. Keep the exact recipe/path unchanged.
        projection["image"]["external_roots"][0]["path"] = str(tmp_path / "foreign")
        projection["image_sha256"] = canonical_json_sha256(projection["image"])
        image.manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    if fault == "none":
        selected = authority.load_development_environment(
            image.root, repo_root=repo, python=version, groups=groups
        )
        assert (
            selected.root == image.root
            and selected.bytecode_prefix == image.bytecode_prefix
        )
    else:
        with pytest.raises(authority_schema.SourceBuildEnvironmentError):
            authority.load_development_environment(
                image.root, repo_root=repo, python=version, groups=groups
            )


@pytest.mark.parametrize("coordinate", ["dot", "leaf-link", "ancestor-link"])
def test_development_loader_normalizes_coordinates_without_admitting_aliases(
    tmp_path, coordinate
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "project"
    image = materialize_development_environment(repo, tmp_path / "artifacts")
    before = image.manifest_path.read_bytes()
    if coordinate == "dot":
        step = image.root.parent / "coordinate"
        step.mkdir()
        requested = step / ".." / image.root.name
        assert (
            authority.load_development_environment(requested, repo_root=repo).root
            == image.root
        )
    else:
        alias = tmp_path / "alias"
        target = image.root if coordinate == "leaf-link" else image.root.parent
        try:
            alias.symlink_to(target, target_is_directory=True)
        except OSError:
            pytest.skip("directory symlinks unavailable")
        requested = alias if coordinate == "leaf-link" else alias / image.root.name
        with pytest.raises(
            authority_schema.SourceBuildEnvironmentError, match="physical root"
        ):
            authority.load_development_environment(requested, repo_root=repo)
    assert image.manifest_path.read_bytes() == before


def test_cross_minor_development_reader_uses_recorded_wheel_target(
    tmp_path, monkeypatch
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "project"
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", python="3.13", groups=("dev",)
    )
    monkeypatch.setattr(
        "packaging.tags.sys_tags",
        lambda: (_ for _ in ()).throw(AssertionError("reader host tags used")),
    )
    monkeypatch.setattr(
        authority,
        "_probe_source_build_python",
        lambda *_a, **_k: pytest.fail("cheap reader spawned target probe"),
    )
    assert (
        authority.load_development_environment(image.root, repo_root=repo).custody
        == image.custody
    )


def test_real_tag_capture_keeps_exact_runtime_identity():
    base = Path(getattr(sys, "_base_executable", None) or sys.executable).resolve()
    plain = authority._probe_source_build_python(base)
    captured = authority._probe_source_build_python(base, capture_wheel_target=True)
    assert captured["runtime"] == plain
    assert (
        captured["wheel_target"]["runtime_closure_sha256"]
        == plain["runtime_closure_sha256"]
    )
    assert captured["wheel_target"]["tags"]
