"""Recipe identity must not depend on a launcher's imported native modules."""

from __future__ import annotations

from tests.process_guard_common import guard_result_for_test
from molt import python_environment_contract as _contract
from molt.python_environment_identity import PythonIdentityVerifier

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
from molt import source_build_environment_selection as selection
from molt import source_build_environment_schema as authority_schema
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

    captured = authority._python_identity()
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
    executable = create_test_venv(root)
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
    # No -B or cache prefix: prove the filesystem backstop, not argv policy.
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


@pytest.mark.parametrize("development", [False, True])
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
    tmp_path, development, fault
):
    from molt.exact_json import canonical_json_sha256
    from tests.python_environment_test_support import (
        build_environment_manifest,
        materialize_development_environment,
    )

    if development:
        image = materialize_development_environment(
            tmp_path / "checkout", tmp_path / "artifacts"
        )
        manifest = copy.deepcopy(image.custody)
        realized = copy.deepcopy(manifest["realized_image"]["image"])
    else:
        manifest = copy.deepcopy(
            build_environment_manifest(["ninja==1.13.0"])["custody"]
        )
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
    field = "realized_image" if development else "realized_environment"
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
        old = (
            manifest["realized_image"]["image"]
            if development
            else manifest["realized_environment"]
        )
        old["tree"]["entries"][0]["access"]["writable"] = False
        live_rows[0]["access"]["writable"] = True
        old["tree"]["manifest_sha256"] = canonical_json_sha256(old["tree"]["entries"])
        if development:
            manifest["realized_image"] = {
                "image": old,
                "image_sha256": canonical_json_sha256(old),
            }
        else:
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
        old = (
            manifest["realized_image"]["image"]
            if development
            else manifest["realized_environment"]
        )
        old["tree"]["manifest_sha256"] = "f" * 64
    elif fault == "shape":
        manifest["realized_image" if development else "realized_environment"] = (
            "malformed"
        )
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
        if not development:
            authority.validate_python_environment_identity(live)
        authority._admit_attestation(manifest, custody, live)
    else:
        with pytest.raises(_contract.SourceBuildEnvironmentError, match="differs"):
            authority._admit_attestation(manifest, custody, live)
    assert manifest == original


@pytest.fixture
def selection_image(tmp_path):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "checkout"
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", groups=("dev",)
    )
    return repo, image


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "schema",
        "contract",
        "contract-type",
        "environment_id",
        "checkout",
        "groups",
        "inputs",
        "binding",
        "extra",
        "missing-key",
        "missing-file",
        "duplicate",
        "oversized",
    ],
)
def test_selection_projection_rejects_raw_record_corruption(selection_image, fault):
    repo, image = selection_image
    path = image.root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION
    raw = path.read_text(encoding="utf-8")
    payload = json.loads(raw)
    if fault == "missing-file":
        path.unlink()
    elif fault == "duplicate":
        path.write_text('{"schema":"duplicate",' + raw.lstrip()[1:], encoding="utf-8")
    elif fault == "oversized":
        path.write_bytes(b" " * (1024 * 1024 + 1))
    elif fault is not None:
        if fault == "contract":
            payload["producer_contract"]["source_build"] = -1
        elif fault == "contract-type":
            payload["producer_contract"]["source_build"] = float(
                payload["producer_contract"]["source_build"]
            )
        elif fault == "inputs":
            payload["declared_inputs"]["uv.lock"] = "0" * 64
        elif fault == "binding":
            payload["editable_binding"]["source_root"] = str(repo / "foreign")
        elif fault == "extra":
            payload["extra"] = True
        elif fault == "missing-key":
            del payload["python_version"]
        else:
            payload[fault] = ["other"] if fault == "groups" else "wrong"
        path.write_text(json.dumps(payload), encoding="utf-8")
    if fault is None:
        selected = selection.read_development_environment_selection(
            image.root, repo_root=repo, groups=("dev",)
        )
        assert selected.root == image.root
        assert selected.groups == ("dev",)
        assert selected.python_version.text == payload["python_version"]
    else:
        with pytest.raises(_contract.SourceBuildEnvironmentError):
            selection.read_development_environment_selection(image.root, repo_root=repo)


@pytest.mark.parametrize("fault", ["python", "empty-groups", "checkout", "address"])
def test_selection_projection_binds_actual_requested_coordinates(
    selection_image, tmp_path, fault
):
    repo, image = selection_image
    root = image.root
    kwargs = {"repo_root": repo, "groups": ("dev",)}
    if fault == "python":
        kwargs["python"] = "99.1"
    elif fault == "empty-groups":
        kwargs["groups"] = ()
    elif fault == "checkout":
        foreign = tmp_path / "foreign"
        foreign.mkdir()
        kwargs["repo_root"] = foreign
    else:
        root = image.root.with_name("0" * 64)
        image.root.rename(root)
    with pytest.raises(_contract.SourceBuildEnvironmentError):
        selection.read_development_environment_selection(root, **kwargs)


@pytest.mark.parametrize(
    "fault",
    [
        "provisional",
        "manifest-missing",
        "manifest-directory",
        "manifest-link",
        "prefix-missing",
        "prefix-populated",
        "prefix-link",
    ],
)
def test_selection_requires_completed_physical_publication(
    selection_image, tmp_path, fault
):
    from molt.python_environment_location import environment_provisioning_record_path

    repo, image = selection_image
    prefix = image.root / "bytecode"
    if fault == "provisional":
        marker = environment_provisioning_record_path(image.root)
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text("{}", encoding="utf-8")
    elif fault.startswith("manifest"):
        image.manifest_path.unlink()
        if fault == "manifest-directory":
            image.manifest_path.mkdir()
        elif fault == "manifest-link":
            target = tmp_path / "manifest"
            target.write_text("{}", encoding="utf-8")
            try:
                image.manifest_path.symlink_to(target)
            except OSError:
                pytest.skip("file symlinks unavailable")
    elif fault == "prefix-populated":
        (prefix / "late.pyc").write_bytes(b"executable cache")
    else:
        prefix.rmdir()
        if fault == "prefix-link":
            target = tmp_path / "empty"
            target.mkdir()
            try:
                prefix.symlink_to(target, target_is_directory=True)
            except OSError:
                pytest.skip("directory symlinks unavailable")
    with pytest.raises(_contract.SourceBuildEnvironmentError):
        selection.read_development_environment_selection(image.root, repo_root=repo)


@pytest.mark.parametrize("version", ["3.15.0a1", "3.15.0b2", "3.15.0rc1"])
def test_selection_preserves_prerelease_identity(selection_image, version):
    repo, image = selection_image
    path = image.root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["python_version"] = version
    path.write_text(json.dumps(payload), encoding="utf-8")
    selected = selection.read_development_environment_selection(
        image.root, repo_root=repo
    )
    assert selected.python_version.text == version
    assert selected.python_version.is_prerelease
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="requested Python"):
        selection.read_development_environment_selection(
            image.root, repo_root=repo, python="3.15"
        )


def test_selection_runs_without_site_capture_subprocess_or_full_manifest(
    selection_image,
):
    from tests.process_guard_common import run_guarded_test_process

    repo, image = selection_image
    # A final manifest must exist, but ordinary selection must not decode it.
    image.manifest_path.write_bytes(b"not a full manifest; proof must reject this")
    code = r"""
import builtins,io,json,os,sys,subprocess
from pathlib import Path
sys.path.insert(0,sys.argv[1])
blocked=('packaging','molt.python_environment_custody','molt.python_environment_identity',
         'molt.python_capture','molt.source_build_environment')
class NoHeavyImports:
    def find_spec(self,fullname,path=None,target=None):
        if any(fullname==name or fullname.startswith(name+'.') for name in blocked):
            raise AssertionError('selection imported '+fullname)
sys.meta_path.insert(0,NoHeavyImports())
def no_spawn(*args,**kwargs):
    raise AssertionError('selection spawned a child')
subprocess.Popen=no_spawn
manifest=Path(sys.argv[3])/'molt-source-build-environment.json'
def guarded_open(original):
    def opened(file,*args,**kwargs):
        if isinstance(file,(str,bytes,Path)) and Path(os.fsdecode(file)).resolve()==manifest.resolve():
            raise AssertionError('selection read the full manifest')
        return original(file,*args,**kwargs)
    return opened
builtins.open=guarded_open(builtins.open)
io.open=guarded_open(io.open)
from molt.source_build_environment_selection import read_development_environment_selection
selected=read_development_environment_selection(Path(sys.argv[3]),repo_root=Path(sys.argv[2]),groups=('dev',))
assert not any(name=='packaging' or name.startswith('packaging.') for name in sys.modules)
print(json.dumps({'root':str(selected.root),'groups':selected.groups}))
"""
    result = run_guarded_test_process(
        [
            sys.executable,
            "-B",
            "-I",
            "-S",
            "-c",
            code,
            str(Path(__file__).resolve().parents[2] / "src"),
            str(repo),
            str(image.root),
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"root": str(image.root), "groups": ["dev"]}


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


def test_selection_rejects_same_byte_readme_alias_rebinding(selection_image, tmp_path):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo, _image = selection_image
    project = repo / "pyproject.toml"
    project.write_text(
        project.read_text(encoding="utf-8").replace(
            "[project]\n", "[project]\nreadme='README.md'\n", 1
        ),
        encoding="utf-8",
    )
    for name in ("A.md", "B.md"):
        (repo / name).write_bytes(b"same installed description\n")
    alias = repo / "README.md"
    try:
        alias.symlink_to("A.md")
    except OSError:
        pytest.skip("file symlinks unavailable")
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", groups=("dev",)
    )
    assert selection.read_development_environment_selection(image.root, repo_root=repo)
    alias.unlink()
    alias.symlink_to("B.md")
    assert (repo / "A.md").read_bytes() == alias.read_bytes()
    with pytest.raises(_contract.SourceBuildEnvironmentError):
        selection.read_development_environment_selection(image.root, repo_root=repo)


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


def test_raw_python_pin_spelling_changes_published_address(selection_image, tmp_path):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo, image = selection_image
    pin = repo / ".python-version"
    pin.write_bytes(pin.read_bytes() + b"\n")
    with pytest.raises(_contract.SourceBuildEnvironmentError):
        selection.read_development_environment_selection(image.root, repo_root=repo)
    changed = materialize_development_environment(
        repo, tmp_path / "artifacts", groups=("dev",)
    )
    assert changed.root != image.root


def test_live_development_admission_reads_one_recipe_and_preserves_empty_groups(
    tmp_path, monkeypatch
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "checkout"
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", groups=("dev",)
    )
    realized = image.custody["realized_image"]["image"]
    reads = []
    read = authority._read_attestation

    def observed(path):
        reads.append(path)
        return read(path)

    monkeypatch.setattr(authority, "_read_attestation", observed)
    request = _contract.DevelopmentEnvironmentRequest(repo.resolve(), None, ("dev",))
    admitted = authority.admit_published_environment(
        image.root, realized, development_admission=request
    )
    assert admitted == image.custody
    assert reads == [image.manifest_path]
    # An unconstrained metadata read is distinct from exact empty admission.
    assert authority.load_development_environment(
        image.root, repo_root=repo, groups=None
    )
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="requested groups"):
        authority.admit_published_environment(
            image.root,
            realized,
            development_admission=_contract.DevelopmentEnvironmentRequest(
                repo, None, ()
            ),
        )
    (image.root / "bytecode" / "late.pyc").write_bytes(b"unselected cache")
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="physical empty"):
        authority.load_development_environment(image.root, repo_root=repo)


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
        with pytest.raises(_contract.SourceBuildEnvironmentError, match=error):
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
        with pytest.raises(_contract.SourceBuildEnvironmentError):
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
            _contract.SourceBuildEnvironmentError, match="physical root"
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
        with pytest.raises(_contract.SourceBuildEnvironmentError, match=error):
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
