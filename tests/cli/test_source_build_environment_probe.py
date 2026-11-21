"""Recipe identity must not depend on a launcher's imported native modules."""

from __future__ import annotations

from tests.process_guard_common import guard_result_for_test
from molt import python_environment_contract as _contract
from molt.python_environment_identity import (
    PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    PythonIdentityVerifier,
)

import hashlib
import copy
import json
import os
import shutil
import stat
from pathlib import Path
import sys

import pytest

from molt import source_build_environment as authority
from molt import python_environment_location as location
from molt.process_environment import ProcessEnvironment
from molt import source_build_environment_selection as selection
from tests.python_environment_test_support import (
    copy_python_capture_authority,
    create_test_venv,
    runtime_identity_manifest,
    wheel_target_manifest,
)
from tests.process_guard_common import install_module_view


@pytest.fixture
def real_published_image(tmp_path):
    """Real no-pip venv and installed package; no mocked capture or admission."""
    from molt.exact_json import canonical_json_sha256, write_exact
    from molt.file_deletion import delete_path
    from molt.python_environment_location import _site_roots
    from tests.python_environment_test_support import lock_closure_manifest
    from tests.process_guard_common import run_guarded_test_process

    # Address the image with the private interpreter it will actually use.
    # A copied base can have a different runtime closure from the host install.
    bootstrap = create_test_venv(tmp_path / "bootstrap")
    with PythonIdentityVerifier() as verifier:
        captured = authority._probe_source_build_python(
            bootstrap, verifier=verifier, capture_wheel_target=True
        )
    runtime = captured["runtime"]
    closure = lock_closure_manifest(["hf529-probe==1.0"], [("hf529-probe", "1.0")])
    closure["wheel_target"] = captured["wheel_target"]
    closure["closure_sha256"] = canonical_json_sha256(
        {key: value for key, value in closure.items() if key != "closure_sha256"}
    )
    recipe = {
        "schema_version": _contract.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "producer_contract": _contract.source_build_environment_contract(),
        "declared_inputs": {
            "pyproject.toml": "a" * 64,
            "uv.lock": "b" * 64,
            ".python-version": None,
        },
        "dependency_groups": closure["dependency_groups"],
        "dependency_group_requirements": closure["requirements"],
        "lock_closure": closure,
        "python_runtime": runtime,
        "editable_binding": None,
        "bytecode_policy": _contract.IMAGE_BYTECODE_POLICY,
        "uv": {"executable": "uv", "version": "fixture", "sha256": "d" * 64},
    }
    identity = canonical_json_sha256(recipe)
    root = tmp_path / "images" / identity
    created = run_guarded_test_process(
        [
            str(bootstrap),
            "-B",
            "-m",
            "venv",
            "--without-pip",
            "--copies" if os.name == "nt" else "--symlinks",
            str(root),
        ],
        timeout=PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    )
    assert created.returncode == 0, created.stderr
    executable = location.environment_python_path(root)
    (site,) = _site_roots(root)
    module = site / "hf529_probe.py"
    module.write_text("VALUE = 529\n", encoding="utf-8")
    metadata = site / "hf529_probe-1.0.dist-info"
    metadata.mkdir()
    (metadata / "METADATA").write_text(
        "Metadata-Version: 2.1\nName: hf529-probe\nVersion: 1.0\n", encoding="utf-8"
    )
    (metadata / "WHEEL").write_text(
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        encoding="utf-8",
    )
    (metadata / "RECORD").write_text(
        "hf529_probe.py,,\n"
        + "".join(
            f"{metadata.name}/{name},,\n" for name in ("METADATA", "WHEEL", "RECORD")
        ),
        encoding="utf-8",
    )
    (root / "bytecode").mkdir()
    authority._install_no_bytecode_hook(
        executable, ProcessEnvironment(os.environ), root
    )
    custody = {**recipe, "environment_id": identity}
    realized = authority._probe_environment_identity(executable, root)
    manifest_path = root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST
    write_exact(manifest_path, authority._attested_environment(custody, realized))
    try:
        yield root, executable, module, manifest_path
    finally:
        ok, error = delete_path(root, owning_root=root.parent)
        assert ok, error


def _admit_published_image(root, executable):
    """Run the production probe, then the caller's admission, for ``root``."""
    recorded = json.loads(
        (root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST).read_bytes()
    )
    recipe = {
        key: value for key, value in recorded.items() if key != "realized_environment"
    }
    captured = authority._probe_environment_identity(
        executable, root, attestation=recorded
    )
    return authority._admit_attestation(recorded, recipe, captured)


@pytest.mark.slow
@pytest.mark.skipif(shutil.which("uv") is None, reason="needs the real uv")
def test_published_image_admits_after_uv_recreates_lock(
    real_published_image: tuple[Path, Path, Path, Path],
) -> None:
    from tests.process_guard_common import run_guarded_test_process

    root, executable, _module, manifest = real_published_image
    lock = root / ".lock"
    assert not lock.exists()
    before = manifest.read_bytes()

    # CI pins its base interpreter with uv environment variables. Give this
    # invocation only the private fixture's interpreter selection.
    env = {key: value for key, value in os.environ.items() if not key.startswith("UV_")}
    env["VIRTUAL_ENV"] = str(root)
    result = run_guarded_test_process(
        [
            "uv",
            "--offline",
            "--no-config",
            "--no-cache",
            "pip",
            "uninstall",
            "--dry-run",
            "hf529-probe",
        ],
        cwd=root,
        env=env,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert lock.read_bytes() == b""
    admitted = _admit_published_image(root, executable)

    assert all(
        entry["path"] != ".lock"
        for entry in admitted["realized_environment"]["tree"]["entries"]
    )
    assert manifest.read_bytes() == before
    assert lock.read_bytes() == b""


def test_legacy_attestation_admits_with_uv_lock_absent() -> None:
    from tests.python_environment_test_support import (
        build_environment_manifest,
        realized_environment_manifest,
    )

    expected = build_environment_manifest(["ninja==1.13.0"])["custody"]
    custody = {
        key: value for key, value in expected.items() if key != "realized_environment"
    }
    live = expected["realized_environment"]
    legacy = {
        **custody,
        "realized_environment": realized_environment_manifest(
            custody["python_runtime"], [("ninja", "1.13.0")], legacy_uv_lock=True
        ),
    }
    original = copy.deepcopy(legacy)
    assert authority._admit_attestation(legacy, custody, live) == expected
    assert legacy == original


def test_installation_seal_preserves_external_links_and_image_alias_topology(tmp_path):
    from molt.file_deletion import delete_path
    from molt.python_environment_location import seal_environment_installation

    image = tmp_path / "image"
    site = image / "lib" / "site-packages"
    site.mkdir(parents=True)
    scripts = image / ("Scripts" if os.name == "nt" else "bin")
    scripts.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    source = outside / "source.py"
    source.write_bytes(b"VALUE = 529\n")
    before_mode = source.stat().st_mode
    first = site / "first.py"
    second = scripts / "second.py"
    first.hardlink_to(source)
    second.hardlink_to(source)
    (scripts / "external").symlink_to(outside, target_is_directory=True)
    (image / "bytecode").mkdir()
    try:
        seal_environment_installation(image)
        seal_environment_installation(image)
        assert source.stat().st_mode == before_mode
        assert outside.stat().st_mode & stat.S_IWUSR
        assert source.read_bytes() == first.read_bytes() == second.read_bytes()
        assert first.samefile(second) and not first.samefile(source)
        assert not first.stat().st_mode & stat.S_IWUSR
        assert not list(image.glob(".seal-*"))
        assert (image / "bytecode").stat().st_mode & stat.S_IWUSR
    finally:
        assert delete_path(image, owning_root=tmp_path) == (True, "")


@pytest.mark.skipif(os.name != "posix", reason="actual POSIX mode/bytecode denial")
@pytest.mark.parametrize("partial", [False, True])
@pytest.mark.slow
def test_sealed_image_refuses_real_isolated_bytecode_and_keeps_admission(
    real_published_image, partial
):
    from tests.process_guard_common import run_guarded_test_process

    root, executable, module, manifest = real_published_image
    before = manifest.read_bytes()
    if partial:
        # Exact interim operator action: lib directories only, files writable.
        for path in (root / "lib", *(root / "lib").rglob("*")):
            if path.is_dir():
                path.chmod(stat.S_IMODE(path.stat().st_mode) & ~0o222)
        assert module.stat().st_mode & stat.S_IWUSR
    from molt.cli.source_build_inventory import SourceBuildInventory

    live_custody = _admit_published_image(root, executable)
    inventory = SourceBuildInventory(live_custody, root)
    assert (
        inventory.file(
            module.relative_to(root).as_posix(), distribution="hf529-probe"
        ).path
        == module
    )
    for path in (root / "lib", root / "bin", *root.rglob("*")):
        if path.is_symlink() or not location.environment_installation_entry(
            path.relative_to(root).as_posix()
        ):
            continue
        assert path.stat().st_mode & 0o222 == 0, path
    assert (root / "bytecode").stat().st_mode & stat.S_IWUSR
    assert manifest.read_bytes() == before  # Never rewrite a legacy attestation.
    with pytest.raises(PermissionError):
        (module.parent / "__pycache__").mkdir()
    # No -B or cache prefix: prove the image's own rules, not argv policy.
    child = run_guarded_test_process(
        [str(executable), "-I", "-c", "import hf529_probe; print(hf529_probe.VALUE)"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert child.returncode == 0 and child.stdout == "529\n", child.stderr
    assert not list(root.rglob("*.pyc"))
    assert _admit_published_image(root, executable) == live_custody
    assert manifest.read_bytes() == before


@pytest.mark.slow
def test_image_hook_refuses_isolated_bytecode_through_a_writable_tree(
    real_published_image: tuple[Path, Path, Path, Path],
) -> None:
    """HF-544: a privileged Windows token writes through the mode seal.

    The tree stays writable here, as such a token sees it. Only the image's
    own site hook can keep `-I` without `-B` from compiling into the image.
    """
    from tests.process_guard_common import run_guarded_test_process

    root, executable, module, _manifest = real_published_image
    hook = module.parent / _contract.IMAGE_NO_BYTECODE_HOOK
    assert hook.read_bytes() == _contract.IMAGE_NO_BYTECODE_HOOK_SOURCE
    argv = [str(executable), "-I", "-c", "import hf529_probe; print(hf529_probe.VALUE)"]

    def run() -> list[Path]:
        child = run_guarded_test_process(
            argv, capture_output=True, text=True, timeout=30
        )
        assert child.returncode == 0 and child.stdout == "529\n", child.stderr
        return sorted(root.rglob("*.pyc"))

    assert run() == []
    # Control: the same launch without the hook compiles into the image, so the
    # assertion above observes the hook rather than an inert interpreter.
    hook.unlink()
    assert run() != []


@pytest.mark.slow
@pytest.mark.parametrize("fault", ["bytes", "earlier_hook"])
def test_capture_refuses_a_hook_site_startup_would_not_run_first(
    real_published_image: tuple[Path, Path, Path, Path], fault: str
) -> None:
    root, executable, module, _manifest = real_published_image
    hook = module.parent / _contract.IMAGE_NO_BYTECODE_HOOK
    if fault == "bytes":
        hook.write_bytes(b"import sys\n")
        expected = "unsupported molt-no-bytecode.v1 startup declaration"
    else:
        # "-" sorts before "0", so site.addsitedir() would run this file first.
        (module.parent / "-early.pth").write_bytes(b"import json\n")
        expected = "before the image no-bytecode hook"
    with pytest.raises(_contract.SourceBuildEnvironmentError, match=expected):
        authority._probe_environment_identity(executable, root)


@pytest.mark.slow
def test_modified_sealed_image_refusal_names_file_and_preserves_attestation(
    real_published_image,
):
    root, executable, module, manifest = real_published_image
    before = manifest.read_bytes()
    authority.seal_environment_installation(root)
    module.chmod(module.stat().st_mode | stat.S_IWUSR)
    module.write_text("VALUE = 530\n", encoding="utf-8")
    with pytest.raises(_contract.SourceBuildEnvironmentError) as refused:
        _admit_published_image(root, executable)
    assert "differs from captured live bytes" in str(refused.value)
    assert module.relative_to(root).as_posix() in str(refused.value)
    assert "sha256" in str(refused.value)
    assert manifest.read_bytes() == before


@pytest.mark.skipif(os.name != "posix", reason="actual POSIX mode bits")
@pytest.mark.slow
def test_sealed_attestation_skips_the_seal_and_refuses_reopened_write_bits(
    real_published_image,
):
    from molt.exact_json import write_exact

    root, executable, module, manifest = real_published_image
    recorded = json.loads(manifest.read_bytes())
    recipe = {
        key: value for key, value in recorded.items() if key != "realized_environment"
    }
    authority.seal_environment_installation(root)
    sealed = authority._attested_environment(
        recipe, authority._probe_environment_identity(executable, root)
    )
    # Independent oracle: the publisher's record of an image sealed before capture.
    assert location.attestation_records_writable_installation(recorded)
    assert not location.attestation_records_writable_installation(sealed)
    write_exact(manifest, sealed)
    assert _admit_published_image(root, executable) == sealed
    module.chmod(module.stat().st_mode | stat.S_IWUSR)
    with pytest.raises(_contract.SourceBuildEnvironmentError) as refused:
        _admit_published_image(root, executable)
    assert module.relative_to(root).as_posix() in str(refused.value)
    assert module.stat().st_mode & stat.S_IWUSR  # Admission did not reseal it.


@pytest.mark.parametrize(
    "fault",
    [
        "none",
        "partial",
        "content",
        "read",
        "execute",
        "write-up",
        "added",
        "removed",
        "outside",
        "checksum",
        "shape",
    ],
)
def test_admission_accepts_only_one_way_installed_permission_transition(
    tmp_path, fault
):
    from molt.exact_json import canonical_json_sha256
    from tests.python_environment_test_support import build_environment_manifest

    manifest = copy.deepcopy(build_environment_manifest(["ninja==1.13.0"])["custody"])
    realized = copy.deepcopy(manifest["realized_environment"])
    tree = realized["tree"]
    rows = tree["entries"]
    installed = [
        row
        for row in rows
        if location.environment_installation_entry(row["path"])
        and row["kind"] in {"file", "hardlink", "directory"}
    ]
    assert installed
    # Independent oracle: explicitly chosen old/new permission values. Digests
    # merely make each fixture structurally valid; they decide no acceptance.
    for row in installed:
        row["access"]["writable"] = True
    tree["manifest_sha256"] = canonical_json_sha256(rows)
    field = "realized_environment"
    custody = {key: value for key, value in manifest.items() if key != field}
    realized["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in realized.items()
            if key != "environment_closure_sha256"
        }
    )
    manifest = authority._attested_environment(custody, realized)
    live = copy.deepcopy(realized)
    live_rows = live["tree"]["entries"]
    target = next(row for row in live_rows if row["path"] == installed[0]["path"])
    selected = live_rows if fault != "partial" else [target]
    for row in selected:
        if location.environment_installation_entry(row["path"]) and row["kind"] in {
            "file",
            "hardlink",
            "directory",
        }:
            row["access"]["writable"] = False
    if fault == "read":
        target["access"]["readable"] = not target["access"]["readable"]
    elif fault == "execute":
        target["access"]["executable"] = not target["access"]["executable"]
    elif fault == "write-up":
        old = manifest["realized_environment"]
        old["tree"]["entries"][0]["access"]["writable"] = False
        live_rows[0]["access"]["writable"] = True
        old["tree"]["manifest_sha256"] = canonical_json_sha256(old["tree"]["entries"])
        old["environment_closure_sha256"] = canonical_json_sha256(
            {
                key: value
                for key, value in old.items()
                if key != "environment_closure_sha256"
            }
        )
    elif fault == "content":
        live["tree"]["file_nodes"][0]["sha256"] = "f" * 64
    elif fault == "added":
        live_rows.append(
            {
                "path": "lib/late",
                "kind": "directory",
                "access": {"readable": True, "writable": False, "executable": True},
            }
        )
    elif fault == "removed":
        live_rows.remove(target)
    elif fault == "outside":
        outside = next(
            row
            for row in live_rows
            if not location.environment_installation_entry(row["path"])
            and "access" in row
        )
        outside["access"]["writable"] = not outside["access"]["writable"]
    elif fault == "checksum":
        old = manifest["realized_environment"]
        old["tree"]["manifest_sha256"] = "f" * 64
    elif fault == "shape":
        manifest["realized_environment"] = "malformed"
    live["tree"]["manifest_sha256"] = canonical_json_sha256(live_rows)
    live["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in live.items()
            if key != "environment_closure_sha256"
        }
    )
    original = copy.deepcopy(manifest)
    if fault in {"none", "partial"}:
        authority.validate_python_environment_identity(live)
        authority._admit_attestation(manifest, custody, live)
    else:
        with pytest.raises(_contract.SourceBuildEnvironmentError, match="differs"):
            authority._admit_attestation(manifest, custody, live)
    assert manifest == original


def _metadata_project(repo, readme):
    repo.mkdir(parents=True, exist_ok=True)
    (repo / "pyproject.toml").write_text(
        "[project]\nname='selector-fixture'\nversion='1.0'\n"
        "requires-python='>=3.12'\ndependencies=[]\nreadme="
        + json.dumps(readme)
        + "\n[dependency-groups]\n",
        encoding="utf-8",
    )
    (repo / "uv.lock").write_text("version=1\n", encoding="utf-8")


def test_declared_readme_alias_retarget_rejects_same_bytes(tmp_path):
    repo = tmp_path / "checkout"
    _metadata_project(repo, "README.md")
    for name in ("A.md", "B.md"):
        (repo / name).write_bytes(b"identical description\n")
    alias = repo / "README.md"
    try:
        alias.symlink_to("A.md")
    except OSError:
        pytest.skip("file symlinks unavailable")
    before = selection.capture_source_build_inputs(repo, editable=True)
    alias.unlink()
    alias.symlink_to("B.md")
    after = selection.capture_source_build_inputs(repo, editable=True)
    assert before.digests == after.digests
    assert before.editable_binding != after.editable_binding
    assert "A.md" in before.editable_binding["metadata_inputs"]
    assert "B.md" in after.editable_binding["metadata_inputs"]
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="binding"):
        before.verify()


def test_declared_metadata_symlink_then_parent_matches_backend_open(tmp_path):
    repo = tmp_path / "checkout"
    _metadata_project(repo, "alias/../README.md")
    (repo / "nested/child").mkdir(parents=True)
    (repo / "README.md").write_bytes(b"wrong lexical collapse")
    (repo / "nested/README.md").write_bytes(b"actual backend input")
    try:
        (repo / "alias").symlink_to("nested/child", target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks unavailable")
    declared = repo / "alias/../README.md"
    expected = hashlib.sha256(declared.read_bytes()).hexdigest()
    inputs = selection.capture_source_build_inputs(repo, editable=True)
    assert inputs.digests["alias/../README.md"] == expected
    assert inputs.editable_binding["metadata_inputs"]["nested/README.md"] == expected
    assert "README.md" not in inputs.editable_binding["metadata_inputs"]


def test_optional_python_pin_absence_and_raw_bytes_are_inputs(tmp_path):
    _metadata_project(tmp_path, "README.md")
    (tmp_path / "README.md").write_bytes(b"description")
    missing = selection.capture_source_build_inputs(tmp_path, editable=True)
    assert missing.digests[".python-version"] is None
    pin = tmp_path / ".python-version"
    pin.write_bytes(b"3.12\n")
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="changed"):
        missing.verify()
    first = selection.capture_source_build_inputs(tmp_path, editable=True)
    pin.write_bytes(b"3.12\n\n")
    second = selection.capture_source_build_inputs(tmp_path, editable=True)
    assert first.digests[".python-version"] == hashlib.sha256(b"3.12\n").hexdigest()
    assert second.digests[".python-version"] == hashlib.sha256(b"3.12\n\n").hexdigest()
    assert first.digests != second.digests


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
        _contract.SourceBuildEnvironmentError,
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
        _contract.SourceBuildEnvironmentError,
        match="package source admission failed",
    ) as caught:
        authority._probe_environment_identity(
            executable, root, external_root=foreign, source_root=selected
        )
    assert str(foreign / "src/molt") in str(caught.value)
    assert str(selected / "src/molt") in str(caught.value)


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
    assert (
        options["timeout"]
        == authority.python_environment_identity.PYTHON_IDENTITY_PROBE_TIMEOUT_S
    )
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


@pytest.mark.parametrize("attested", [None, "sealed", "legacy"])
def test_realized_environment_probe_uses_read_only_capture(
    monkeypatch, tmp_path, attested
):
    expected = {"realized": "environment"}
    calls = []
    attestation = (
        None
        if attested is None
        else {
            "realized_environment": {
                "tree": {
                    "entries": [
                        {
                            "path": "lib/python3.12/site-packages/module.py",
                            "kind": "file",
                            "access": {
                                "readable": True,
                                "writable": attested == "legacy",
                                "executable": False,
                            },
                        }
                    ]
                }
            }
        }
    )

    def run(argv, **kwargs):
        verifier_prefix = Path(argv[4].split("=", 1)[1])
        assert verifier_prefix.is_dir()
        assert not list(verifier_prefix.iterdir())
        assert not verifier_prefix.is_relative_to(tmp_path)
        calls.append(argv)
        return guard_result_for_test(
            argv, 0, json.dumps(expected), "", descendants_closed=True
        )

    sealed = []
    monkeypatch.setattr(authority.process_guard, "run_completed_command", run)
    monkeypatch.setattr(
        authority, "validate_python_environment_identity", lambda value: value
    )
    monkeypatch.setattr(
        authority,
        "seal_environment_installation",
        lambda root: sealed.append((root, list(calls))),
    )
    executable = Path("selected-python")
    probe = authority._probe_environment_identity
    assert probe(executable, tmp_path, attestation=attestation) == expected
    # Only a legacy attestation with writable installed entries is sealed, and
    # before the capture starts; the child stays a plain capture that imports
    # nothing from the image.
    assert sealed == ([(tmp_path, [])] if attested == "legacy" else [])
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


@pytest.mark.parametrize("attested", ["sealed", "legacy"])
def test_probe_refusal_names_the_seal_only_after_sealing(
    monkeypatch, tmp_path, attested
):
    attestation = {
        "realized_environment": {
            "tree": {
                "entries": [
                    {
                        "path": "lib/python3.12/site-packages/module.py",
                        "kind": "file",
                        "access": {
                            "readable": True,
                            "writable": attested == "legacy",
                            "executable": False,
                        },
                    }
                ]
            }
        }
    }
    sealed = []
    monkeypatch.setattr(authority, "seal_environment_installation", sealed.append)

    def refuse(*_args, **_kwargs):
        raise _contract.SourceBuildEnvironmentError("capture refused")

    monkeypatch.setattr(authority, "_probe_source_build_python", refuse)
    with pytest.raises(_contract.SourceBuildEnvironmentError) as refused:
        authority._probe_environment_identity(
            Path("selected-python"), tmp_path, attestation=attestation
        )
    if attested == "legacy":
        assert sealed == [tmp_path]
        assert str(refused.value) == (
            f"published image was sealed before admission: {tmp_path}; capture refused"
        )
    else:
        assert sealed == []
        assert str(refused.value) == "capture refused"


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
        _contract.SourceBuildEnvironmentError, match="runtime probe returned"
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
    with pytest.raises(_contract.SourceBuildEnvironmentError):
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
        _contract.SourceBuildEnvironmentError,
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
    (tmp_path / "uv.lock").write_text("version=1\n", encoding="utf-8")
    source = tmp_path / "demo.py"
    source.write_text("VALUE = 1\n", encoding="utf-8")
    binding = selection.capture_source_build_inputs(
        tmp_path, editable=True
    ).editable_binding
    source.write_text("VALUE = 2\n", encoding="utf-8")
    assert (
        selection.capture_source_build_inputs(tmp_path, editable=True).editable_binding
        == binding
    )
    (tmp_path / "README.md").write_text(
        "changed installed description\n", encoding="utf-8"
    )
    assert (
        selection.capture_source_build_inputs(tmp_path, editable=True).editable_binding
        != binding
    )
    assert binding["source_root"] == str(tmp_path)
    assert "demo.py" not in binding["metadata_inputs"]


@pytest.mark.slow
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
