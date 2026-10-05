"""Bounded synthetic proofs for shared Python content and location custody."""

from __future__ import annotations

import base64
import copy
from contextlib import contextmanager
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import shutil
import sys
from types import SimpleNamespace

import pytest
from tools.command_execution import CommandExecutor

from molt import python_capture as capture
from molt import python_environment_custody as environment
from molt import python_environment_identity as identity
from molt import python_environment_location as location
from molt import python_file_node_custody as files
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from tests.python_environment_test_support import runtime_identity_manifest


_COMMANDS = CommandExecutor.for_file(__file__)


_PORTABLE_PATH_ALIASES = [
    pytest.param("tool", "TOOL", id="ascii-case"),
    pytest.param("caf\u00e9", "cafe\u0301", id="unicode-normalization"),
    pytest.param("stra\u00dfe", "STRASSE", id="unicode-casefold"),
]


@pytest.mark.parametrize("authority", ["", "localhost", "LOCALHOST", "LoCaLhOsT"])
def test_editable_file_uri_preserves_local_path_encoding(tmp_path, authority):
    root = tmp_path / "source space caf\u00e9"
    root.mkdir()
    payload = json.dumps(
        {
            "url": root.as_uri().replace("file://", f"file://{authority}", 1),
            "dir_info": {"editable": True},
        }
    ).encode()
    assert (
        location.editable_direct_url_path(payload, distribution="molt")
        == root.resolve()
    )


@pytest.mark.parametrize("suffix", ["%", "%GG", "%2520"])
def test_editable_file_uri_rejects_ambiguous_percent_encoding(tmp_path, suffix):
    payload = json.dumps(
        {
            "url": tmp_path.as_uri() + "/" + suffix,
            "dir_info": {"editable": True},
        }
    ).encode()
    with pytest.raises(PythonEnvironmentIdentityError, match="file URL"):
        location.editable_direct_url_path(payload, distribution="molt")


@pytest.mark.parametrize("url", ["file:relative", "file:../source", "file:C:relative"])
def test_editable_file_uri_relative_paths_keep_owned_diagnostic(url):
    payload = json.dumps({"url": url, "dir_info": {"editable": True}}).encode()
    with pytest.raises(PythonEnvironmentIdentityError, match="non-absolute"):
        location.editable_direct_url_path(payload, distribution="molt")


@pytest.mark.skipif(os.name != "nt", reason="DOS drives are Windows-only")
@pytest.mark.parametrize("separator", [":", "|"])
def test_editable_file_uri_admits_local_windows_drive_spellings(tmp_path, separator):
    url = tmp_path.as_uri().replace(
        tmp_path.drive, tmp_path.drive[0].lower() + separator, 1
    )
    payload = json.dumps({"url": url, "dir_info": {"editable": True}}).encode()
    assert (
        location.editable_direct_url_path(payload, distribution="molt")
        == tmp_path.resolve()
    )


@pytest.mark.parametrize("suffix", ["%00", "%FF"])
def test_editable_file_uri_cannot_alias_invalid_filesystem_text(tmp_path, suffix):
    payload = json.dumps(
        {"url": tmp_path.as_uri() + "/" + suffix, "dir_info": {"editable": True}}
    ).encode()
    with pytest.raises(PythonEnvironmentIdentityError):
        location.editable_direct_url_path(payload, distribution="molt")


@pytest.mark.parametrize(
    "url",
    [
        "https://localhost/source",
        "file://remote/source",
        "file:////remote/source",
        "file://localhost/source?query",
        "file://localhost/source#fragment",
    ],
)
def test_editable_file_uri_rejects_nonlocal_authority(url):
    payload = json.dumps({"url": url, "dir_info": {"editable": True}}).encode()
    with pytest.raises(PythonEnvironmentIdentityError, match="non-local"):
        location.editable_direct_url_path(payload, distribution="molt")


@pytest.mark.slow
def test_isolated_probe_never_writes_disposable_source_bytecode(tmp_path, monkeypatch):
    """Exercise real imports without relying on isolation-ignored environment flags."""
    source_root = Path(identity.__file__).resolve().parents[2]
    disposable = tmp_path / "source"
    for source in identity.python_capture_authority_paths():
        destination = disposable / source.relative_to(source_root)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    copied_authority = disposable / "src" / "molt" / "python_environment_identity.py"
    monkeypatch.setattr(identity, "__file__", str(copied_authority))

    def snapshot():
        return {
            path.relative_to(disposable).as_posix(): (
                hashlib.sha256(path.read_bytes()).hexdigest(),
                path.stat().st_mtime_ns,
            )
            for path in disposable.rglob("*")
            if path.is_file()
        }

    before = snapshot()
    assert not list(disposable.rglob("__pycache__"))
    command = [
        str(getattr(sys, "_base_executable", sys.executable)),
        *identity.python_identity_probe_arguments(("--capture-runtime",), no_site=True),
    ]
    for _ in range(2):
        completed = _COMMANDS.run(
            command,
            cwd=disposable,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "0"},
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
            check=False,
        )
        assert completed.returncode == 0, completed.stderr
        identity.validate_python_runtime_identity(json.loads(completed.stdout))
        assert not list(disposable.rglob("__pycache__"))
        assert snapshot() == before


def _tree(root: Path, context: files.PythonFileCaptureContext):
    pool = files._FileNodePool(capture_context=context)
    tree, _paths, _metadata = files._stable_tree_inventory(
        root, root_id="fixture", label="fixture", pool=pool
    )
    return tree, pool


@pytest.mark.parametrize("change", ["membership", "changed-size-restored-mtime"])
def test_runtime_session_rechecks_live_context_and_revokes_after_failure(
    tmp_path, monkeypatch, change
):
    monkeypatch.setattr(
        identity, "current_python_runtime_selection", lambda: {"selection": "fixture"}
    )
    root = tmp_path / "lib"
    root.mkdir()
    member = root / "module.py"
    member.write_bytes(b"before")
    original = member.stat()
    context = files.PythonFileCaptureContext()
    _tree(root, context)
    context.verify()
    output = io.StringIO()
    payload = {"runtime_closure_sha256": "a" * 64}

    def requests():
        yield "verify\n"
        if change == "membership":
            (root / "new.py").write_bytes(b"addition")
        else:
            member.write_bytes(b"after-with-changed-size")
            os.utime(member, ns=(original.st_atime_ns, original.st_mtime_ns))
        yield "verify\n"

    with pytest.raises(ValueError):
        identity._serve_runtime_session(
            payload, context, requests=requests(), responses=output
        )
    assert output.getvalue().splitlines() == [
        json.dumps(
            {"runtime": payload, "startup_selection": {"selection": "fixture"}},
            separators=(",", ":"),
            sort_keys=True,
        ),
        "a" * 64,
    ]
    with pytest.raises(ValueError, match="revoked"):
        context.verify()


def test_runtime_session_eof_closes_context_without_recapture(tmp_path, monkeypatch):
    monkeypatch.setattr(
        identity, "current_python_runtime_selection", lambda: {"selection": "fixture"}
    )
    context = files.PythonFileCaptureContext()
    _tree(tmp_path, context)
    payload = {"runtime_closure_sha256": "a" * 64}
    output = io.StringIO()
    identity._serve_runtime_session(
        payload, context, requests=iter(["verify\n", "verify\n"]), responses=output
    )
    assert output.getvalue().splitlines()[1:] == ["a" * 64, "a" * 64]
    with pytest.raises(ValueError, match="revoked"):
        context.verify()


@pytest.mark.parametrize(
    "change", ["archive", "new-pyvenv", "new-pth", "pyvenv-rewrite"]
)
def test_runtime_capture_fences_import_and_startup_selection(
    tmp_path, monkeypatch, change
):
    from molt import python_runtime_identity as runtime

    library = tmp_path / "lib"
    library.mkdir()
    (library / "module.py").write_bytes(b"source")
    executable = tmp_path / "python.exe"
    executable.write_bytes(b"python")
    pyvenv = tmp_path / "pyvenv.cfg"
    if change == "pyvenv-rewrite":
        pyvenv.write_bytes(b"home = original")
        original_config = pyvenv.stat()
    archive = tmp_path / f"python{sys.version_info.major}{sys.version_info.minor}.zip"
    monkeypatch.setattr(
        runtime,
        "sys",
        SimpleNamespace(
            base_prefix=str(tmp_path),
            prefix=str(tmp_path),
            executable=str(executable),
            version_info=sys.version_info,
            path=[str(library), str(archive)],
        ),
    )
    monkeypatch.setattr(runtime, "unicodedata", SimpleNamespace(__file__=None))
    monkeypatch.setattr(
        runtime,
        "_platform_identity",
        lambda: {"operating_system": "windows", "architecture": "x86_64"},
    )
    monkeypatch.setattr(runtime, "_base_runtime_paths", lambda: {"stdlib": library})
    monkeypatch.setattr(runtime, "_runtime_library", lambda: None)
    monkeypatch.setattr(runtime, "_runtime_linkage", lambda _library: "static")
    monkeypatch.setattr(
        runtime,
        "_runtime_capabilities",
        lambda *_args, **_kwargs: {
            "required_root_roles": ["stdlib"],
            "native_dependency_policy": "fixture",
        },
    )
    monkeypatch.setattr(
        runtime, "_native_dependency_closure", lambda *_args, **_kwargs: {}
    )
    from molt import python_native_locations as native_locations

    monkeypatch.setattr(
        native_locations,
        "_loaded_native_module_snapshot",
        lambda _os: SimpleNamespace(
            executable=executable,
            paths=(executable,),
            aliases={},
            contracts=(),
            macho_identities={},
        ),
    )
    context = files.PythonFileCaptureContext()
    payload, _roots, _pool, _explicit = runtime._capture_runtime_with_context(
        capture_context=context
    )
    assert payload["import_roots"][-1]["kind"] == "absent-archive"
    context.verify()
    if change == "archive":
        changed = archive
    elif change == "new-pth":
        changed = executable.with_suffix("._pth")
    else:
        changed = pyvenv
    changed.write_bytes(b"home = modified")
    if change == "pyvenv-rewrite":
        os.utime(changed, ns=(original_config.st_atime_ns, original_config.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        context.verify()
    changed.unlink()
    with pytest.raises(ValueError, match="revoked"):
        context.verify()


@pytest.mark.parametrize(
    "index,field",
    tuple(enumerate(("mode", "device", "inode", "size", "mtime_ns", "ctime_ns"))),
)
def test_snapshot_diagnostic_records_changed_root_field(index, field):
    before = (1, 2, 3, 4, 5, 6)
    after = list(before)
    after[index] = 99
    assert files._snapshot_difference((before, ()), (tuple(after), ())) == (
        f"root metadata changed: {field}={before[index]}->99"
    )


def test_snapshot_diagnostic_records_changed_entry_field(tmp_path):
    path = tmp_path / "entry"
    path.write_bytes(b"data")
    before = path.lstat()
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000))
    after = path.lstat()
    root = files._path_stat_identity(tmp_path.lstat())
    difference = files._snapshot_difference(
        files._snapshot_fingerprint(root, [("entry", path, before)]),
        files._snapshot_fingerprint(root, [("entry", path, after)]),
    )
    assert difference.startswith("entry metadata changed: entry [")
    assert f"mtime_ns={before.st_mtime_ns}->{after.st_mtime_ns}" in difference


@pytest.mark.parametrize("mode", [stat.S_IFDIR, stat.S_IFREG, stat.S_IFLNK])
def test_path_identity_separates_directory_allocation_from_file_length(mode):
    def metadata(size):
        return SimpleNamespace(
            st_mode=mode, st_dev=1, st_ino=2, st_size=size, st_mtime_ns=3, st_ctime_ns=4
        )

    before = files._path_stat_identity(metadata(0))
    after = files._path_stat_identity(metadata(4096))
    assert (before == after) is (mode == stat.S_IFDIR)


@pytest.mark.parametrize(
    "field", ["st_mode", "st_dev", "st_ino", "st_mtime_ns", "st_ctime_ns"]
)
def test_directory_identity_retains_replacement_access_and_timestamp_fences(field):
    values = dict(
        st_mode=stat.S_IFDIR,
        st_dev=1,
        st_ino=2,
        st_size=0,
        st_mtime_ns=3,
        st_ctime_ns=4,
    )
    before = files._path_stat_identity(SimpleNamespace(**values))
    values[field] += 1
    assert files._path_stat_identity(SimpleNamespace(**values)) != before


@pytest.mark.parametrize("change", ["add", "remove", "symlink"])
def test_outer_capture_verification_retains_tree_membership(tmp_path, change):
    path = tmp_path / "original.py"
    path.write_bytes(b"same")
    target = tmp_path / "second.py"
    target.write_bytes(b"same")
    link = tmp_path / "link.py"
    if change == "symlink":
        try:
            link.symlink_to(path.name)
        except OSError as exc:
            pytest.skip(f"symlink creation unavailable: {exc}")
    context = files.PythonFileCaptureContext()
    _tree(tmp_path, context)
    context.verify()
    before_root = tmp_path.lstat()
    if change == "add":
        (tmp_path / "new.py").write_bytes(b"new import")
    elif change == "remove":
        path.unlink()
    else:
        link.unlink()
        link.symlink_to(target.name)
    os.utime(tmp_path, ns=(before_root.st_atime_ns, before_root.st_mtime_ns))
    with pytest.raises(ValueError, match="changed|unavailable|could not be verified"):
        context.verify()


def test_outer_tree_fence_retains_pruning_policy(tmp_path):
    path = tmp_path / "original.py"
    path.write_bytes(b"same")
    excluded = tmp_path / "ignored"
    excluded.mkdir()
    context = files.PythonFileCaptureContext()
    files._stable_tree_inventory(
        tmp_path,
        root_id="fixture",
        label="fixture",
        pool=files._FileNodePool(capture_context=context),
        excluded=frozenset({"ignored"}),
    )
    (excluded / "data").write_bytes(b"not in the attested tree")
    context.verify()


@pytest.mark.parametrize("workers", [1, 2, 4])
def test_file_inventory_hashes_each_object_once_and_is_deterministic(tmp_path, workers):
    contents = [f"payload-{index:02}".encode() for index in range(70)]
    for index in reversed(range(len(contents))):
        (tmp_path / f"{index:02}.py").write_bytes(contents[index])
    context = files.PythonFileCaptureContext(hash_workers=workers)
    tree, pool = _tree(tmp_path, context)
    again, second_pool = _tree(tmp_path, context)
    assert tree == again
    assert pool.nodes == second_pool.nodes
    assert [row["sha256"] for row in pool.nodes] == [
        hashlib.sha256(data).hexdigest() for data in contents
    ]
    assert context.inventory_profile()["hashed_files"] == len(contents)
    assert context.inventory_profile()["hashed_bytes"] == sum(map(len, contents))
    assert len(context.file_custody()) == len(contents)
    assert not any(isinstance(value, bytes) for value in vars(pool).values())
    assert pool.read_bound("file-node-0", label="fixture") == b"payload-00"


def test_hardlink_alias_has_one_hash_and_every_absolute_path(tmp_path):
    original = tmp_path / "a.py"
    original.write_bytes(b"contents")
    alias = tmp_path / "b.py"
    try:
        os.link(original, alias)
    except OSError as exc:
        pytest.skip(f"hardlink creation unavailable: {exc}")
    context = files.PythonFileCaptureContext()
    tree, pool = _tree(tmp_path, context)
    assert len(pool.nodes) == 1
    assert [row["kind"] for row in tree["entries"]] == ["file", "hardlink"]
    assert context.inventory_profile()["hashed_files"] == 1
    assert len(context.file_custody()) == 2
    original.write_bytes(b"modified")
    with pytest.raises(ValueError, match="changed"):
        pool.read_bound("file-node-0", label="fixture")


def test_batch_checks_each_hardlink_path_against_its_snapshot(tmp_path, monkeypatch):
    original = tmp_path / "a.py"
    original.write_bytes(b"contents")
    alias = tmp_path / "b.py"
    try:
        os.link(original, alias)
    except OSError as exc:
        pytest.skip(f"hardlink creation unavailable: {exc}")
    rows = [(path, path.lstat()) for path in (original, alias)]
    capture_file = files.stable_regular_file_identity

    def replace_alias_after_hash(path, **kwargs):
        captured = capture_file(path, **kwargs)
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"contents")
        os.utime(replacement, ns=(rows[1][1].st_atime_ns, rows[1][1].st_mtime_ns))
        os.replace(replacement, alias)
        return captured

    monkeypatch.setattr(files, "stable_regular_file_identity", replace_alias_after_hash)
    context = files.PythonFileCaptureContext()
    with pytest.raises(ValueError, match="changed"):
        context.bind_many(rows, label="hardlink fixture")


def test_internal_directory_symlink_is_captured_as_one_owned_alias(tmp_path):
    target = tmp_path / "lib"
    target.mkdir()
    (target / "module.py").write_bytes(b"module")
    alias = tmp_path / "lib64"
    try:
        alias.symlink_to(target.name, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink creation unavailable: {exc}")

    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())

    entries = {row["path"]: row for row in tree["entries"]}
    assert entries["lib64"] == {
        "path": "lib64",
        "kind": "directory-symlink",
        "target_owner": "same-root",
        "target": "lib",
        "access": entries["lib"]["access"],
    }
    assert "lib64/module.py" not in entries
    assert tree["file_count"] == 1
    assert len(pool.nodes) == 1


def test_directory_symlink_escape_is_rejected(tmp_path):
    root = tmp_path / "environment"
    root.mkdir()
    external = tmp_path / "external"
    external.mkdir()
    alias = root / "lib64"
    try:
        alias.symlink_to(external, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink creation unavailable: {exc}")

    with pytest.raises(PythonEnvironmentIdentityError, match="symlink escapes custody"):
        _tree(root, files.PythonFileCaptureContext())
    with pytest.raises(PythonEnvironmentIdentityError, match="symlink escapes custody"):
        files._stable_tree_inventory(
            root,
            root_id="fixture",
            label="fixture",
            pool=files._FileNodePool(),
            external_symlink_roles={"base-executable": external},
        )


def test_macos_framework_stdlib_symlink_uses_exact_runtime_library_role(tmp_path):
    version = tmp_path / "Python.framework" / "Versions" / "3.12"
    stdlib = version / "lib" / "python3.12"
    config = stdlib / "config-3.12-darwin"
    config.mkdir(parents=True)
    runtime_library = version / "Python"
    runtime_library.write_bytes(b"framework runtime")
    archive = config / "libpython3.12.a"
    try:
        archive.symlink_to(runtime_library)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    tree, _paths, _metadata = files._stable_tree_inventory(
        stdlib,
        root_id="runtime-root-0",
        label="Python runtime",
        pool=files._FileNodePool(),
        external_symlink_roles={"runtime-library": runtime_library},
    )

    row = next(
        entry
        for entry in tree["entries"]
        if entry["path"] == "config-3.12-darwin/libpython3.12.a"
    )
    assert row == {
        "path": "config-3.12-darwin/libpython3.12.a",
        "kind": "symlink",
        "target_owner": "base-runtime",
        "target_role": "runtime-library",
        "access": row["access"],
    }
    assert tree["file_count"] == 1


@pytest.mark.parametrize("role", ["base-executable", "runtime-library", "unicodedata"])
def test_external_runtime_symlink_roles_are_derived_from_exact_files(tmp_path, role):
    root = tmp_path / "runtime-root"
    root.mkdir()
    external = tmp_path / role
    external.write_bytes(role.encode())
    link = root / "component"
    try:
        link.symlink_to(external)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    tree, _paths, _metadata = files._stable_tree_inventory(
        root,
        root_id="fixture",
        label="fixture",
        pool=files._FileNodePool(),
        external_symlink_roles={role: external},
    )

    assert tree["entries"] == [
        {
            "path": "component",
            "kind": "symlink",
            "target_owner": "base-runtime",
            "target_role": role,
            "access": tree["entries"][0]["access"],
        }
    ]


@pytest.mark.parametrize("target_kind", ["unrelated", "sibling", "mismatched-role"])
def test_external_runtime_symlink_rejects_non_authority_file(tmp_path, target_kind):
    root = tmp_path / "runtime-root"
    root.mkdir()
    framework = tmp_path / "Python.framework"
    framework.mkdir()
    runtime_library = framework / "Python"
    runtime_library.write_bytes(b"runtime")
    sibling = framework / "Python.debug"
    sibling.write_bytes(b"sibling")
    unrelated = tmp_path / "unrelated"
    unrelated.write_bytes(b"unrelated")
    target = unrelated if target_kind == "unrelated" else sibling
    declared = runtime_library if target_kind != "mismatched-role" else sibling
    link = root / "component"
    try:
        link.symlink_to(target if target_kind != "mismatched-role" else runtime_library)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    with pytest.raises(PythonEnvironmentIdentityError, match="symlink escapes custody"):
        files._stable_tree_inventory(
            root,
            root_id="fixture",
            label="fixture",
            pool=files._FileNodePool(),
            external_symlink_roles={"runtime-library": declared},
        )


def test_external_runtime_symlink_rejects_ambiguous_component_identity(tmp_path):
    root = tmp_path / "runtime-root"
    root.mkdir()
    external = tmp_path / "Python"
    external.write_bytes(b"runtime")
    link = root / "component"
    try:
        link.symlink_to(external)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")

    with pytest.raises(PythonEnvironmentIdentityError, match="ambiguous"):
        files._stable_tree_inventory(
            root,
            root_id="fixture",
            label="fixture",
            pool=files._FileNodePool(),
            external_symlink_roles={
                "base-executable": external,
                "runtime-library": external,
            },
        )


@pytest.mark.parametrize(
    "fault",
    ["unknown-role", "invented-owner", "target-path", "extra-metadata"],
)
def test_external_runtime_symlink_receipt_requires_canonical_role_reference(fault):
    access = {"readable": True, "writable": False, "executable": True}
    row = {
        "path": "component",
        "kind": "symlink",
        "target_owner": "base-runtime",
        "target_role": "runtime-library",
        "access": access,
    }
    if fault == "unknown-role":
        row["target_role"] = "framework/Python"
    elif fault == "invented-owner":
        row["target_owner"] = "framework-runtime"
    elif fault == "target-path":
        row["target"] = "/Library/Frameworks/Python.framework/Versions/3.12/Python"
    else:
        row["target_metadata"] = {"role": "runtime-library"}

    with pytest.raises(PythonEnvironmentIdentityError, match="symlink entry"):
        files._validate_inventory_entries(
            [row],
            label="fixture",
            nodes={},
            external_runtime_roles={"runtime-library": "file-node-0"},
        )


@pytest.mark.parametrize("fault", ["self-target", "file-target", "access", "shape"])
def test_directory_symlink_receipt_rejects_malformed_topology(tmp_path, fault):
    target = tmp_path / "lib"
    target.mkdir()
    alias = tmp_path / "lib64"
    try:
        alias.symlink_to(target.name, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink creation unavailable: {exc}")
    (tmp_path / "payload").write_bytes(b"payload")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    entries = copy.deepcopy(tree["entries"])
    row = next(entry for entry in entries if entry["path"] == "lib64")
    if fault == "self-target":
        row["target"] = "lib64"
    elif fault == "file-target":
        row["target"] = "payload"
    elif fault == "access":
        row["access"]["writable"] = not row["access"]["writable"]
    else:
        row["node"] = "file-node-0"

    with pytest.raises(
        PythonEnvironmentIdentityError, match="directory symlink|target"
    ):
        files._validate_inventory_entries(
            entries,
            label="fixture",
            nodes={node["id"]: node for node in pool.nodes},
        )


def test_file_capture_rejects_changed_size_with_restored_mtime(tmp_path):
    path = tmp_path / "a.py"
    path.write_bytes(b"before")
    context = files.PythonFileCaptureContext()
    before = path.stat()
    context.bind(path, before, label="fixture")
    path.write_bytes(b"after-with-changed-size")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        context.bind(path, path.stat(), label="fixture")


def test_file_capture_rejects_changed_size_between_batches(
    tmp_path,
):
    path = tmp_path / "a.py"
    path.write_bytes(b"before")
    context = files.PythonFileCaptureContext()
    before = path.stat()
    context.bind_many([(path, before)], label="fixture")
    path.write_bytes(b"after-with-changed-size")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))

    with pytest.raises(ValueError, match="changed"):
        context.bind(path, before, label="fixture")


@pytest.mark.parametrize("change", ["rewrite", "replace"])
def test_batch_final_fence_rejects_earlier_file_metadata_change(
    tmp_path, monkeypatch, change
):
    path = tmp_path / "a.py"
    path.write_bytes(b"before")
    before = path.lstat()

    def mutate():
        if change == "rewrite":
            path.write_bytes(b"after-with-changed-size")
        else:
            replacement = tmp_path / "replacement.py"
            replacement.write_bytes(b"after!")
            os.replace(replacement, path)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))

    later = tmp_path / "b.py"
    later.write_bytes(b"later")
    context = files.PythonFileCaptureContext()
    capture = files.stable_regular_file_identity

    def capture_then_mutate(candidate, **kwargs):
        captured = capture(candidate, **kwargs)
        if candidate == later:
            mutate()
        return captured

    monkeypatch.setattr(files, "stable_regular_file_identity", capture_then_mutate)
    context.bind_many([(path, before), (later, later.lstat())], label="fixture")
    with pytest.raises(ValueError, match="changed"):
        context.verify()


def test_batch_bind_uses_one_open_per_fresh_file_and_keeps_the_final_fence(
    tmp_path, monkeypatch
):
    from molt import toolchain_identity

    paths = [tmp_path / name for name in ("a.py", "b.py")]
    for path in paths:
        path.write_bytes(path.name.encode())
    opened = []
    open_stable = toolchain_identity.open_stable_regular_file

    def counted(path, **kwargs):
        opened.append(Path(path).name)
        return open_stable(path, **kwargs)

    monkeypatch.setattr(toolchain_identity, "open_stable_regular_file", counted)
    context = files.PythonFileCaptureContext()
    rows = [(path, path.lstat()) for path in paths]
    context.bind_many(rows, label="fixture")
    # Snapshot binding is part of each hashing open.
    assert sorted(opened) == ["a.py", "b.py"]
    # A subsequent bind independently verifies the stored generation.
    context.bind(*rows[0], label="fixture")
    assert sorted(opened) == ["a.py", "a.py", "b.py"]
    context.verify()
    assert sorted(opened) == ["a.py", "a.py", "a.py", "b.py", "b.py"]
    paths[1].write_bytes(b"changed-size.py")
    os.utime(paths[1], ns=(rows[1][1].st_atime_ns, rows[1][1].st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        context.verify()


def test_tree_membership_is_closed_once_by_the_final_fence(tmp_path, monkeypatch):
    package = tmp_path / "package"
    package.mkdir()
    (package / "module.py").write_bytes(b"module")
    root = tmp_path.resolve()
    scanned = []
    scandir = files.os.scandir

    def scan(path):
        scanned.append(Path(path))
        return scandir(path)

    monkeypatch.setattr(files.os, "scandir", scan)
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    bind_many = pool.bind_many

    def bind_then_add(rows, *, label):
        result = bind_many(rows, label=label)
        (package / "late.py").write_bytes(b"late import")
        return result

    monkeypatch.setattr(pool, "bind_many", bind_then_add)
    tree, _paths, _metadata = files._stable_tree_inventory(
        tmp_path, root_id="fixture", label="fixture", pool=pool
    )
    assert [row["path"] for row in tree["entries"]] == [
        "package",
        "package/module.py",
    ]
    assert scanned.count(root) == 1
    with pytest.raises(ValueError, match="changed during inventory"):
        context.verify()
    assert scanned.count(root) == 2


def test_zero_inode_objects_use_distinct_path_identity(tmp_path, monkeypatch):
    paths = [tmp_path / "a.py", tmp_path / "b.py"]
    for path in paths:
        path.write_bytes(b"same")
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    # Zero-inode filesystems must not unify unrelated equal-content files.
    zero = os.stat_result((stat.S_IFREG | 0o600, 0, 0, 1, 0, 0, 4, 0, 0, 0))
    real_bind = context.bind_many
    monkeypatch.setattr(
        context,
        "bind_many",
        lambda rows, *, label: real_bind(
            [(path, path.stat()) for path, _expected in rows], label=label
        ),
    )
    assert pool.bind(paths[0], zero, label="zero inode") != pool.bind(
        paths[1], zero, label="zero inode"
    )
    assert len(pool.nodes) == 2


def test_capture_parallel_pending_work_is_bounded(tmp_path, monkeypatch):
    for index in range(273):
        (tmp_path / f"{index:03}.py").write_bytes(b"x")
    windows = []

    class Executor:
        def __init__(self, *, max_workers):
            assert max_workers == 2

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def map(self, function, work):
            windows.append(len(work))
            assert all(1 <= len(batch) <= 32 for batch in work)
            return map(function, work)

    monkeypatch.setattr(files, "ThreadPoolExecutor", Executor)
    context = files.PythonFileCaptureContext(hash_workers=2)
    _tree(tmp_path, context)
    context.verify()
    # Capture and final verification each schedule nine bounded batches.
    assert windows == [8, 1, 8, 1]


def _envelope():
    return {
        "schema": capture.PYTHON_CAPTURE_SCHEMA,
        "identity": runtime_identity_manifest(),
        "file_custody": [
            {"path": "C:\\runtime\\python.exe", "size": 1, "sha256": "a" * 64}
        ],
        "node_custody": [{"node": "/file_nodes/0", "file_index": 0}],
        "inventory_profile": {
            "hash_workers": 1,
            "hashed_files": 1,
            "hashed_bytes": 1,
            "hash_seconds": 0.0,
        },
    }


def test_capture_envelope_separates_semantic_identity_from_telemetry():
    first = capture.validate_python_capture(_envelope())
    second = copy.deepcopy(first)
    second["inventory_profile"]["hash_seconds"] = 12.0
    assert capture.validate_python_capture(second)["identity"] == first["identity"]


@pytest.mark.parametrize(
    "packages",
    [[], [("example", "1.0")], [("zeta", "2.0"), ("alpha", "1.0")]],
)
def test_synthetic_environment_preserves_declared_empty_import_roots(packages):
    from tests.python_environment_test_support import realized_environment_manifest

    environment = realized_environment_manifest(runtime_identity_manifest(), packages)
    directories = {
        row["path"]
        for row in environment["tree"]["entries"]
        if row["kind"] == "directory"
    }
    assert environment["scripts_root"] in directories
    assert set(environment["site_roots"]).issubset(directories)
    assert len(environment["distributions"]) == len(packages)
    assert [row["name"] for row in environment["distributions"]] == sorted(
        name for name, _version in packages
    )
    assert environment["tree"]["file_count"] == 2 + len(packages)


def test_capture_node_bindings_share_physical_custody_across_authorities(tmp_path):
    from molt.exact_json import canonical_json_sha256
    from tests.python_environment_test_support import realized_environment_manifest

    context = files.PythonFileCaptureContext()
    shared = tmp_path / "file-0"
    shared.write_bytes(b"shared")
    runtime_pool = files._FileNodePool(capture_context=context)
    runtime_pool.bind(shared, shared.stat(), label="runtime")
    runtime = runtime_identity_manifest()
    runtime["file_nodes"] = runtime_pool.nodes
    runtime["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in runtime.items()
            if key != "runtime_closure_sha256"
        }
    )
    environment = realized_environment_manifest(runtime, [("example", "1.0")])
    environment_pool = files._FileNodePool(capture_context=context)
    for index in range(len(environment["tree"]["file_nodes"])):
        path = tmp_path / f"file-{index}"
        if index:
            path.write_bytes(f"environment-{index}".encode())
        environment_pool.bind(path, path.stat(), label="environment")
    environment["tree"]["file_nodes"] = environment_pool.nodes
    environment["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in environment.items()
            if key != "environment_closure_sha256"
        }
    )
    envelope = capture.python_capture_payload(environment, context)
    bindings = {row["node"]: row["file_index"] for row in envelope["node_custody"]}
    assert bindings["/runtime/file_nodes/0"] == bindings["/tree/file_nodes/0"]
    assert len(envelope["node_custody"]) == len(envelope["file_custody"]) + 1
    assert context.inventory_profile()["hashed_files"] == len(envelope["file_custody"])
    with pytest.raises(
        PythonEnvironmentIdentityError, match="producer custody binding"
    ):
        capture.python_capture_payload(copy.deepcopy(environment), context)


@pytest.mark.parametrize(
    "fault", ["missing", "duplicate", "unknown", "boolean", "negative", "outside"]
)
def test_capture_node_bindings_require_exact_typed_coverage(fault):
    envelope = _envelope()
    binding = envelope["node_custody"][0]
    if fault == "missing":
        envelope["node_custody"] = []
    elif fault == "duplicate":
        envelope["node_custody"] *= 2
    elif fault == "unknown":
        binding["node"] = "/file_nodes/1"
    else:
        binding["file_index"] = {"boolean": True, "negative": -1, "outside": 1}[fault]
    with pytest.raises(PythonEnvironmentIdentityError):
        capture.validate_python_capture(envelope)


def test_capture_rejects_equal_content_node_alias_within_one_authority():
    from molt.exact_json import canonical_json_sha256

    envelope = _envelope()
    runtime = envelope["identity"]
    runtime["file_nodes"].append({**runtime["file_nodes"][0], "id": "file-node-1"})
    root = runtime["runtime_roots"][0]
    root["entries"].append(
        {**root["entries"][0], "path": "other.py", "node": "file-node-1"}
    )
    root["file_count"] = 2
    root["node_ids"].append("file-node-1")
    root["manifest_sha256"] = canonical_json_sha256(root["entries"])
    runtime["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in runtime.items()
            if key != "runtime_closure_sha256"
        }
    )
    envelope["file_custody"].append(
        {**envelope["file_custody"][0], "path": "C:\\runtime\\z-other.py"}
    )
    envelope["node_custody"].append({"node": "/file_nodes/1", "file_index": 1})
    capture.validate_python_capture(envelope)
    envelope["node_custody"][1]["file_index"] = 0
    with pytest.raises(PythonEnvironmentIdentityError, match="distinct nodes"):
        capture.validate_python_capture(envelope)


def test_capture_envelope_rejects_windows_case_alias_on_every_host():
    payload = _envelope()
    payload["file_custody"].append(
        {"path": "C:\\RUNTIME\\PYTHON.EXE", "size": 1, "sha256": "a" * 64}
    )
    payload["file_custody"].sort(key=lambda row: row["path"])
    with pytest.raises(PythonEnvironmentIdentityError, match="path alias"):
        capture.validate_python_capture(payload)


@pytest.mark.parametrize("with_custody", [False, True])
def test_runtime_capture_checks_earlier_files_before_return(
    tmp_path, monkeypatch, with_custody
):
    from molt import python_runtime_identity as runtime

    path = tmp_path / "runtime.py"
    path.write_bytes(b"before")
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    pool.bind(path, path.stat(), label="runtime")

    def late_mutation(**_kwargs):
        path.write_bytes(b"after!")
        return {}, (), pool, {}

    monkeypatch.setattr(runtime, "_capture_runtime_with_context", late_mutation)
    with pytest.raises(ValueError, match="changed"):
        runtime.capture_current_python_runtime(with_custody=with_custody)


def test_runtime_custody_capture_has_one_full_finalization_fence(tmp_path, monkeypatch):
    from molt import python_runtime_identity as runtime

    path = tmp_path / "runtime.py"
    path.write_bytes(b"runtime")
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    pool.bind(path, path.lstat(), label="runtime")
    payload = runtime_identity_manifest()
    payload["file_nodes"] = pool.nodes
    payload["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in payload.items()
            if key != "runtime_closure_sha256"
        }
    )
    fences = []
    context.register_verification_fence(lambda: fences.append("full fence"))
    monkeypatch.setattr(
        runtime, "_capture_runtime_with_context", lambda **_kw: (payload, (), pool, {})
    )
    envelope = runtime.capture_current_python_runtime(
        capture_context=context, with_custody=True
    )
    assert envelope["identity"] is payload
    assert envelope["file_custody"] == [
        {"path": str(path), "size": 7, "sha256": hashlib.sha256(b"runtime").hexdigest()}
    ]
    assert fences == ["full fence"]
    path.write_bytes(b"changed")
    # Finalization is never cached across a later publication request.
    with pytest.raises(ValueError, match="changed"):
        capture.python_capture_payload(payload, context)


@pytest.mark.parametrize("change", ["content", "access", "symlink", "parent-link"])
def test_selected_tree_consumer_rejects_content_and_topology_drift(tmp_path, change):
    root = tmp_path / "environment"
    folder = root / "bin"
    folder.mkdir(parents=True)
    path = folder / "tool"
    path.write_bytes(b"tool")
    tree, pool = _tree(root, files.PythonFileCaptureContext())
    entries = {row["path"]: row for row in tree["entries"]}
    nodes = {row["id"]: row for row in pool.nodes}
    files.verify_tree_file(root, "bin/tool", entries=entries, nodes=nodes)
    if change == "content":
        path.write_bytes(b"evil")
    elif change == "access":
        path.chmod(stat.S_IREAD)
    else:
        try:
            if change == "symlink":
                copy_path = folder / "copy"
                copy_path.write_bytes(b"tool")
                path.unlink()
                path.symlink_to(copy_path)
            else:
                moved = root / "moved"
                folder.rename(moved)
                folder.symlink_to(moved, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"symlink creation unavailable: {exc}")
    try:
        with pytest.raises(ValueError, match="differs|indirection"):
            files.verify_tree_file(root, "bin/tool", entries=entries, nodes=nodes)
    finally:
        if change == "access":
            path.chmod(stat.S_IREAD | stat.S_IWRITE)


def test_selected_tree_file_resolves_normalized_receipt_to_host_spelling(tmp_path):
    path = tmp_path / "cafe\u0301"
    path.write_bytes(b"unicode")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    result = files.verify_tree_file(
        tmp_path,
        "caf\u00e9",
        entries={row["path"]: row for row in tree["entries"]},
        nodes={row["id"]: row for row in pool.nodes},
    )
    assert result.path.samefile(path)
    assert result.content.sha256 == hashlib.sha256(b"unicode").hexdigest()


@pytest.mark.parametrize("mutate", [False, True])
def test_selected_tree_batch_fences_earlier_files_and_shares_enumeration(
    tmp_path, monkeypatch, mutate
):
    folder = tmp_path / "tools"
    folder.mkdir()
    first, second = folder / "first", folder / "second"
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    scanned = []
    scandir = files.os.scandir
    identity = files.stable_regular_file_identity

    def scan(path):
        scanned.append(Path(path))
        return scandir(path)

    def capture(path, **kw):
        if mutate and path == second:
            first.write_bytes(b"other")
        return identity(path, **kw)

    monkeypatch.setattr(files.os, "scandir", scan)
    monkeypatch.setattr(files, "stable_regular_file_identity", capture)

    def verify():
        return files.verify_tree_files(
            tmp_path,
            ("tools/first", "tools/second", "tools/first"),
            entries={row["path"]: row for row in tree["entries"]},
            nodes={row["id"]: row for row in pool.nodes},
        )

    if mutate:
        with pytest.raises(ValueError, match="changed"):
            verify()
    else:
        result = verify()
        assert [item.path for item in result] == [first, second, first]
        assert [item.content.sha256 for item in result] == [
            hashlib.sha256(value).hexdigest()
            for value in (b"first", b"second", b"first")
        ]
        assert scanned.count(folder) == 1 and scanned.count(tmp_path) == 1


@pytest.mark.parametrize("kind", ["tree-reference", "runtime-role-reference"])
def test_selected_tree_interpreter_rejects_equal_content_retarget(tmp_path, kind):
    from molt.cli.source_build_inventory import SourceBuildInventory

    root = tmp_path / "environment"
    root.mkdir()
    base = tmp_path / "base-python"
    other = tmp_path / "other-python"
    base.write_bytes(b"python")
    other.write_bytes(b"python")
    selected = root / "python"
    # This test targets the live selected-entry consumer. Receipt parsing has
    # separate complete environment fixtures; here use the real file-node scan.
    inventory = SourceBuildInventory.__new__(SourceBuildInventory)
    inventory.root = root
    inventory.python_executable = selected
    inventory._python_kind = kind
    inventory._python_content = {
        "size": 6,
        "sha256": hashlib.sha256(b"python").hexdigest(),
    }
    if kind == "tree-reference":
        selected.write_bytes(b"python")
        tree, pool = _tree(root, files.PythonFileCaptureContext())
        inventory._entries = {row["path"]: row for row in tree["entries"]}
        inventory._nodes = {row["id"]: row for row in pool.nodes}
    else:
        try:
            selected.symlink_to(base)
        except OSError as exc:
            pytest.skip(f"symlink creation unavailable: {exc}")
    assert (
        inventory.python_identity(base_executable=base).sha256
        == hashlib.sha256(b"python").hexdigest()
    )
    selected.unlink()
    try:
        selected.symlink_to(other)
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    with pytest.raises(ValueError, match="differs"):
        inventory.python_identity(base_executable=base)


@pytest.mark.parametrize("name,alias", _PORTABLE_PATH_ALIASES)
def test_selected_tree_consumer_rejects_portable_aliases_on_every_host(
    tmp_path, monkeypatch, name, alias
):
    folder = tmp_path / "bin"
    folder.mkdir()
    (folder / name).write_bytes(b"tool")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    scandir = os.scandir

    @contextmanager
    def enumerate_aliases(path):
        # Only directory enumeration is synthetic; the receipt, parent checks
        # and selected file are real even on case-insensitive Windows volumes.
        with scandir(path) as children:
            rows = list(children)
        if Path(path) == folder:
            rows.append(SimpleNamespace(name=alias))
        yield iter(rows)

    with monkeypatch.context() as scoped:
        scoped.setattr(files.os, "scandir", enumerate_aliases)
        with pytest.raises(PythonEnvironmentIdentityError, match="path collision"):
            files.verify_tree_file(
                tmp_path,
                f"bin/{name}",
                entries={row["path"]: row for row in tree["entries"]},
                nodes={row["id"]: row for row in pool.nodes},
            )


@pytest.mark.parametrize("name,alias", _PORTABLE_PATH_ALIASES)
def test_selected_tree_consumer_and_capture_reject_added_portable_alias(
    tmp_path, name, alias
):
    folder = tmp_path / "bin"
    folder.mkdir()
    (folder / name).write_bytes(b"tool")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    try:
        with (folder / alias).open("xb") as stream:
            stream.write(b"alias")
    except FileExistsError:
        pytest.skip("filesystem does not permit distinct portable path aliases")
    with pytest.raises(PythonEnvironmentIdentityError, match="path collision"):
        files.verify_tree_file(
            tmp_path,
            f"bin/{name}",
            entries={row["path"]: row for row in tree["entries"]},
            nodes={row["id"]: row for row in pool.nodes},
        )
    with pytest.raises(PythonEnvironmentIdentityError, match="path collision"):
        _tree(tmp_path, files.PythonFileCaptureContext())


@pytest.mark.parametrize(
    "fault",
    [
        "missing",
        "duplicate",
        "relative",
        "size",
        "digest",
        "timing",
        "workers",
        "extra",
    ],
)
def test_capture_envelope_rejects_incomplete_or_malformed_custody(fault):
    payload = _envelope()
    if fault == "missing":
        payload["file_custody"] = []
    elif fault == "duplicate":
        payload["file_custody"] *= 2
    elif fault == "relative":
        payload["file_custody"][0]["path"] = "python.exe"
    elif fault == "size":
        payload["file_custody"][0]["size"] = True
    elif fault == "digest":
        payload["file_custody"][0]["sha256"] = "b" * 64
    elif fault == "timing":
        payload["inventory_profile"]["hash_seconds"] = float("nan")
    elif fault == "workers":
        payload["inventory_profile"]["hash_workers"] = True
    else:
        payload["unexpected"] = True
    with pytest.raises(PythonEnvironmentIdentityError):
        capture.validate_python_capture(payload)


def test_location_exact_native_files_do_not_broaden_watch_roots(tmp_path):
    prefix = tmp_path / "environment"
    prefix.mkdir()
    executable = prefix / "python.exe"
    executable.write_bytes(b"python")
    native = tmp_path / "system.dll"
    native.write_bytes(b"native")
    material = {
        "schema": location.PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "prefix": str(prefix),
        "selected_executable": str(executable),
        "base_executable": str(executable),
        "roots": [str(prefix)],
        "external_roots": [],
        "file_paths": [str(native)],
    }
    payload = {**material, "identity_sha256": canonical_json_sha256(material)}
    assert location.validate_python_environment_location(payload) == payload
    material["file_paths"] = [str(tmp_path)]
    with pytest.raises(PythonEnvironmentIdentityError, match="file kind"):
        location.validate_python_environment_location(
            {**material, "identity_sha256": canonical_json_sha256(material)}
        )


def _installed_fixture(root: Path):
    site = root / "site"
    metadata = site / "demo-1.0.dist-info"
    metadata.mkdir(parents=True)
    module = site / "demo.py"
    module.write_bytes(b"VALUE = 1\n")
    meta = metadata / "METADATA"
    meta.write_text(
        "Metadata-Version: 2.1\nName: Demo\nVersion: 1.0\n", encoding="utf-8"
    )
    rows = []
    for path in (module, meta):
        data = path.read_bytes()
        digest = (
            base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip("=")
        )
        rows.append(f"{path.relative_to(site).as_posix()},sha256={digest},{len(data)}")
    (metadata / "RECORD").write_text(
        "\n".join(rows) + "\ndemo-1.0.dist-info/RECORD,,\n", encoding="utf-8"
    )
    return module


@pytest.mark.parametrize("mutate", [False, True])
def test_distribution_metadata_is_parsed_from_attested_file_nodes(tmp_path, mutate):
    module = _installed_fixture(tmp_path)
    if mutate:
        module.write_bytes(b"VALUE = 2\n")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    args = (
        ["site"],
        "scripts",
        {row["path"]: row for row in tree["entries"]},
        {row["id"]: row for row in pool.nodes},
        pool,
    )
    if mutate:
        with pytest.raises(PythonEnvironmentIdentityError, match="RECORD mismatch"):
            environment._installed_distributions(*args)
    else:
        distributions, owned, scripts = environment._installed_distributions(*args)
        assert [(row["name"], row["version"]) for row in distributions] == [
            ("demo", "1.0")
        ]
        assert "site/demo.py" in owned
        assert scripts == {}


def test_runtime_startup_projection_preserves_actual_native_selection(
    tmp_path, monkeypatch
):
    from molt import python_runtime_identity as runtime
    from molt import python_native_locations as native_locations

    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    executable = tmp_path / "python"
    executable.write_bytes(b"python")
    first_image = first / "libpython.so"
    second_image = second / "libpython.so"
    first_image.write_bytes(b"first")
    second_image.write_bytes(b"other")
    monkeypatch.setattr(
        runtime, "_platform_identity", lambda: {"operating_system": "linux"}
    )
    monkeypatch.setattr(runtime, "_runtime_library", lambda: first_image)
    monkeypatch.setattr(runtime, "_base_runtime_paths", lambda: {"stdlib": tmp_path})
    monkeypatch.setattr(runtime, "_runtime_import_candidates", lambda _base: ([], []))
    monkeypatch.setattr(
        runtime,
        "sys",
        SimpleNamespace(
            executable=str(executable),
            prefix=str(tmp_path),
            base_prefix=str(tmp_path),
            path=[str(tmp_path)],
        ),
    )

    def forbidden_inventory(*_args, **_kwargs):
        raise AssertionError("selection must not inventory runtime content")

    monkeypatch.setattr(runtime, "_stable_tree_inventory", forbidden_inventory)
    observed = SimpleNamespace(
        executable=executable,
        paths=(executable, first_image),
        aliases={"libpython.so": first_image},
        contracts=("original-contract",),
        macho_identities={first_image: (1, 2)},
    )
    monkeypatch.setattr(
        native_locations, "_loaded_native_module_snapshot", lambda _os: observed
    )
    baseline = runtime.current_python_runtime_selection()
    assert baseline["native"]["executable"] == str(executable)
    observed.paths = (executable, second_image)
    assert runtime.current_python_runtime_selection() != baseline
    observed.paths = (executable, first_image)
    observed.aliases = {"libpython.so": second_image}
    assert runtime.current_python_runtime_selection() != baseline
    observed.aliases = {"libpython.so": first_image}
    observed.contracts = ("different-contract",)
    assert runtime.current_python_runtime_selection() != baseline
    observed.contracts = ("original-contract",)
    observed.macho_identities = {first_image: (3, 4)}
    assert runtime.current_python_runtime_selection() != baseline
