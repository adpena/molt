"""Recipe identity must not depend on a launcher's imported native modules."""

from __future__ import annotations

from tests.process_guard_common import guard_result_for_test
from molt.python_environment_identity import PythonIdentityVerifier

import json
import shutil
from pathlib import Path
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
    from molt import python_interpreter, temporary_artifacts

    executable = tmp_path / "selected-python"
    runtime = runtime_identity_manifest()
    version = str(runtime["version"])
    (tmp_path / ".python-version").write_text(version + "\n", encoding="utf-8")
    markers = authority_schema.canonical_source_marker_environment()
    prefixes = []
    prefix_identity = None
    stages = []
    original = RuntimeError("independent selected recipe failure")

    def observe(stage, prefix):
        nonlocal prefix_identity
        if prefix_identity is None:
            prefix_identity = temporary_artifacts._identity(prefix)
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)
        stages.append(stage)
        assert all(value == prefix for value in prefixes)
        if failure == stage:
            raise original

    def resolve(selector, *, env, cwd, verifier):
        assert selector == version and cwd == tmp_path
        assert env["UV_PYTHON_DOWNLOADS"] == "never"
        observe("resolve", verifier.prefix)
        return ("selected-python-command",)

    def identity(command, *, env, cwd, verifier):
        assert command == ("selected-python-command",)
        observe("identity", verifier.prefix)
        return python_interpreter.PythonInterpreter(
            command, str(executable), version, "CPython"
        )

    def capture(selected, *, verifier, capture_wheel_target):
        assert selected == executable
        assert capture_wheel_target
        observe("runtime", verifier.prefix)
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
        return guard_result_for_test(
            command, 0, json.dumps(markers), "", descendants_closed=True
        )

    monkeypatch.setattr(python_interpreter, "resolve_python_selector", resolve)
    monkeypatch.setattr(python_interpreter, "probe_python_command", identity)
    monkeypatch.setattr(authority, "_probe_source_build_python", capture)
    monkeypatch.setattr(authority.process_guard, "run_completed_command", marker_probe)
    try:
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
        assert prefixes
        if failure == "markers":
            # This guarded-launch seam supplies no closure receipt. Retention
            # is required even though the fixture never starts a real child.
            assert not hasattr(original, "guarded_result")
            assert prefixes[0].is_dir()
            assert any(str(prefixes[0]) in note for note in original.__notes__)
        else:
            # The earlier stubs fail before a guarded launch; successful marker
            # results explicitly prove closure through guard_result_for_test.
            assert not prefixes[0].exists()

    finally:
        # Synthetic core only: retire any retained fixture allocation using
        # its captured inode authority, never a newly resolved owner.
        if prefixes and prefixes[0].exists():
            assert prefix_identity is not None
            temporary_artifacts._cleanup_temporary_directory(
                prefixes[0], prefix_identity
            )


@pytest.mark.parametrize(
    "selector,identity_version,runtime_version,marker_version,error",
    [
        ("3.15.1", "3.15.1", "3.15.1", "3.15.1", None),
        ("3.15", "3.15.10", "3.15.10", "3.15.10", None),
        ("command", "3.15.1", "3.15.1", "3.15.1", None),
        (
            "3.15.1",
            "3.15.1",
            "3.15.10",
            "3.15.10",
            "requested CPython 3.15.1 differs from captured runtime 3.15.10",
        ),
        (
            "3.15.1",
            "3.15.10",
            "3.15.1",
            "3.15.1",
            "selected interpreter identity differs from captured runtime",
        ),
        (
            "3.15.1",
            "3.15.1",
            "3.15.1",
            "3.15.10",
            "dependency markers differ from selected Python runtime",
        ),
        (
            "command",
            "3.15.1",
            "3.15.10",
            "3.15.10",
            "selected interpreter identity differs from captured runtime",
        ),
        (
            "command",
            "3.15.1",
            "3.15.1",
            "3.15.10",
            "dependency markers differ from selected Python runtime",
        ),
    ],
)
def test_selected_recipe_joins_request_identity_runtime_and_markers_before_publication(
    tmp_path,
    monkeypatch,
    selector,
    identity_version,
    runtime_version,
    marker_version,
    error,
):
    from molt import python_interpreter

    executable = tmp_path / "selected-python"
    requested = str(executable) if selector == "command" else selector
    runtime = runtime_identity_manifest(python=runtime_version)
    markers = authority_schema.canonical_source_marker_environment()
    markers.update(
        python_full_version=marker_version,
        python_version=".".join(marker_version.split(".")[:2]),
        implementation_version=marker_version,
    )
    stages = []
    prefixes = []

    def observe(stage, prefix):
        stages.append(stage)
        assert prefix.is_dir() and not list(prefix.iterdir())
        prefixes.append(prefix)

    def resolve(value, **kwargs):
        assert value == requested
        assert kwargs["env"]["UV_PYTHON_DOWNLOADS"] == "never"
        observe("discovery", kwargs["verifier"].prefix)
        return ("selected-command",)

    def identity(command, **kwargs):
        assert command == ("selected-command",)
        observe("identity", kwargs["verifier"].prefix)
        return python_interpreter.PythonInterpreter(
            command, str(executable), identity_version, "CPython"
        )

    def capture(selected, **kwargs):
        assert selected == executable and kwargs["capture_wheel_target"] is True
        observe("runtime", kwargs["verifier"].prefix)
        return {"runtime": runtime, "wheel_target": wheel_target_manifest(runtime)}

    def marker_probe(command, **kwargs):
        assert command[0] == str(executable)
        observe("markers", prefixes[-1])
        return guard_result_for_test(
            command, 0, json.dumps(markers), "", descendants_closed=True
        )

    monkeypatch.setattr(python_interpreter, "resolve_python_selector", resolve)
    monkeypatch.setattr(python_interpreter, "probe_python_command", identity)
    monkeypatch.setattr(authority, "_probe_source_build_python", capture)
    monkeypatch.setattr(authority.process_guard, "run_completed_command", marker_probe)
    monkeypatch.setattr(
        authority,
        "_environment_spec",
        lambda *_a, **_k: pytest.fail(
            "incoherent capture reached recipe/provisioning/publication"
        ),
    )
    if error is None:
        assert authority._selected_python_recipe(tmp_path, requested, {}) == (
            executable,
            runtime,
            markers,
            wheel_target_manifest(runtime),
        )
    else:
        with pytest.raises(authority_schema.SourceBuildEnvironmentError, match=error):
            authority.development_environment(
                tmp_path,
                python=requested,
                groups=("dev",),
                provision=True,
                environ={},
            )
    assert stages == ["discovery", "identity", "runtime", "markers"]
    assert prefixes and all(prefix == prefixes[0] for prefix in prefixes)
    assert not prefixes[0].exists()
    assert not list(tmp_path.iterdir())


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
        return guard_result_for_test(
            argv, 0, json.dumps(expected), "", descendants_closed=True
        )

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
    expected = runtime_identity_manifest()
    from molt.python_environment_identity import PythonIdentityVerifier

    with PythonIdentityVerifier() as verifier:
        prefix = verifier.prefix

        def forbidden_allocation(*_args, **_kwargs):
            raise AssertionError("recipe already owns the verifier namespace")

        def run(command, **kwargs):
            assert kwargs["memory_guard_prefix"] == "MOLT_CLI"
            assert command[1:6] == ["-B", "-I", "-X", f"pycache_prefix={prefix}", "-S"]
            assert prefix.is_dir() and not list(prefix.iterdir())
            return guard_result_for_test(
                command, 0, json.dumps(expected), "", descendants_closed=True
            )

        monkeypatch.setattr(authority, "PythonIdentityVerifier", forbidden_allocation)
        monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
        assert (
            authority._probe_source_build_python(
                Path("selected-python"), verifier=verifier
            )
            == expected
        )
        assert prefix.is_dir() and not list(prefix.iterdir())
    assert not prefix.exists()


def test_realized_environment_probe_uses_read_only_capture(monkeypatch, tmp_path):
    expected = {"realized": "environment"}
    calls = []

    def run(argv, **kwargs):
        verifier_prefix = Path(argv[4].split("=", 1)[1])
        assert verifier_prefix.is_dir()
        assert not list(verifier_prefix.iterdir())
        assert not verifier_prefix.is_relative_to(tmp_path)
        calls.append(argv)
        return guard_result_for_test(
            argv, 0, json.dumps(expected), "", descendants_closed=True
        )

    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        authority, "validate_python_environment_identity", lambda value: value
    )
    executable = Path("selected-python")
    probe = authority._probe_environment_identity
    assert probe(executable, tmp_path) == expected
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
def test_recipe_probe_rejects_invalid_or_unattested_runtime(
    verifier, monkeypatch, output
):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: guard_result_for_test(
            argv, 0, output, "", descendants_closed=True
        ),
    )
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError, match="runtime probe returned"
    ):
        authority._probe_source_build_python(Path("selected-python"), verifier=verifier)


@pytest.mark.parametrize("output", ["not json", '{"schema":1,"schema":2}', "{}"])
def test_realized_environment_capture_rejects_invalid_realization(
    tmp_path, monkeypatch, output
):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: guard_result_for_test(
            argv, 0, output, "", descendants_closed=True
        ),
    )
    with pytest.raises(authority_schema.SourceBuildEnvironmentError):
        authority._probe_environment_identity(Path("selected-python"), tmp_path)


def test_recipe_probe_reports_failure_instead_of_inprocess_fallback(
    verifier, monkeypatch
):
    monkeypatch.setattr(
        authority.process_guard,
        "run_completed_command",
        lambda argv, **kwargs: guard_result_for_test(
            argv, 7, "", "capture refused", descendants_closed=True
        ),
    )
    with pytest.raises(
        authority_schema.SourceBuildEnvironmentError,
        match="runtime content: capture refused",
    ):
        authority._probe_source_build_python(Path("selected-python"), verifier=verifier)


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


@pytest.mark.parametrize("python", ["3.13", "3.15.1"])
def test_cross_minor_development_reader_uses_recorded_wheel_target(
    tmp_path, monkeypatch, python
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "project"
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", python=python, groups=("dev",)
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


@pytest.mark.parametrize("mode", ["read", "explicit-reuse"])
@pytest.mark.parametrize(
    "requested,error",
    [
        ("3.15", None),
        ("3.15.1", None),
        ("3.15.0", "different requested Python"),
        ("3.15.10", "different requested Python"),
        ("3", "invalid Python interpreter request"),
        ("3.15.1.0", "invalid Python interpreter request"),
        ("py315", "invalid Python interpreter request"),
    ],
)
def test_development_image_reuse_preserves_exact_host_request(
    tmp_path, monkeypatch, mode, requested, error
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )
    from molt.python_interpreter import parse_target_python_version

    repo = tmp_path / "project"
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", python="3.15.1", groups=("dev",)
    )
    before = image.manifest_path.read_bytes()
    monkeypatch.setattr(
        authority,
        "_selected_python_recipe",
        lambda *_a, **_k: pytest.fail(
            "image-reuse request spawned another runtime probe"
        ),
    )

    def read():
        if mode == "read":
            return authority.load_development_environment(
                image.root, repo_root=repo, python=requested, groups=("dev",)
            )
        return authority.development_environment(
            repo,
            python=requested,
            groups=("dev",),
            environ={"UV_PROJECT_ENVIRONMENT": str(image.root)},
        )

    if error is None:
        assert read().root == image.root
    else:
        with pytest.raises(authority_schema.SourceBuildEnvironmentError, match=error):
            read()
    assert image.manifest_path.read_bytes() == before
    with pytest.raises(ValueError, match="unsupported Python target"):
        parse_target_python_version("3.15")


def test_real_tag_capture_keeps_exact_runtime_identity(
    verifier,
):
    base = Path(getattr(sys, "_base_executable", None) or sys.executable).resolve()
    plain = authority._probe_source_build_python(base, verifier=verifier)
    captured = authority._probe_source_build_python(
        base, verifier=verifier, capture_wheel_target=True
    )
    assert captured["runtime"] == plain
    assert (
        captured["wheel_target"]["runtime_closure_sha256"]
        == plain["runtime_closure_sha256"]
    )
    assert captured["wheel_target"]["tags"]


@pytest.fixture
def verifier():
    with PythonIdentityVerifier() as owner:
        yield owner


@pytest.mark.parametrize("stage", ["discovery", "identity", "runtime", "markers"])
@pytest.mark.parametrize("closed", [False, True])
def test_selected_recipe_guard_failure_retains_original_whole_probe_lifetime(
    tmp_path, monkeypatch, stage, closed
):
    import gc
    import subprocess
    import weakref
    from types import SimpleNamespace
    from molt import temporary_artifacts, toolchain_identity, python_interpreter

    runtime = runtime_identity_manifest()
    version = str(runtime["version"])
    executable = tmp_path / "selected-python"
    executable.write_bytes(b"never executed by this core-guard fixture")
    markers = authority_schema.canonical_source_marker_environment()
    phases = ["discovery", "identity", "runtime", "markers"]
    calls = []
    owners = []
    identities = []
    original_allocation = temporary_artifacts.OwnedTemporaryDirectory

    def allocate(**kwargs):
        allocation = original_allocation(dir=tmp_path, **kwargs)
        owners.append(allocation)
        identities.append(temporary_artifacts._identity(Path(allocation.name)))
        return allocation

    receipt = SimpleNamespace(descendants_closed=closed)
    failure = subprocess.TimeoutExpired([stage], 120)
    failure.guarded_result = receipt

    def run(command, **kwargs):
        phase = phases[len(calls)]
        calls.append(phase)
        assert kwargs["memory_guard_prefix"] in {"MOLT_PYTHON_IDENTITY", "MOLT_CLI"}
        prefix = Path(command[command.index("-X") + 1].removeprefix("pycache_prefix="))
        assert prefix == Path(owners[0].name) and prefix.is_dir()
        if phase == stage:
            raise failure
        if phase in {"discovery", "identity"}:
            payload = {
                "implementation": "CPython",
                "version": version,
                "executable": str(executable),
            }
        elif phase == "runtime":
            payload = {
                "runtime": runtime,
                "wheel_target": wheel_target_manifest(runtime),
            }
        else:
            payload = markers
        return guard_result_for_test(
            command, 0, json.dumps(payload), "", descendants_closed=True
        )

    monkeypatch.setattr(temporary_artifacts, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda name, **_k: None if name == "uv" else executable,
    )
    monkeypatch.setattr(
        python_interpreter,
        "python_command_candidates",
        lambda *_a, **_k: [[str(executable)], ["must-not-run"]],
    )
    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        authority,
        "_provision_source_build_environment",
        lambda *_a, **_k: pytest.fail("failed probe reached publication"),
    )
    with pytest.raises(subprocess.TimeoutExpired) as caught:
        authority.development_environment(
            tmp_path, python=version, groups=("dev",), provision=True, environ={}
        )
    assert caught.value is failure and caught.value.guarded_result is receipt
    assert calls == phases[: phases.index(stage) + 1]
    assert len(owners) == 1
    path = Path(owners[0].name)
    allocation_ref = weakref.ref(owners[0])
    assert path.exists() is (not closed)
    failure.__traceback__ = None
    del caught
    owners.clear()
    gc.collect()
    assert allocation_ref() is None
    assert path.exists() is (not closed)
    if not closed:
        temporary_artifacts._cleanup_temporary_directory(path, identities[0])
