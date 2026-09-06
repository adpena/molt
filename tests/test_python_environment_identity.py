"""Bounded synthetic proofs for shared Python content and location custody."""

from __future__ import annotations

import base64
import copy
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import stat
from types import SimpleNamespace

import pytest

from molt import python_capture as capture
from molt import python_environment_custody as environment
from molt import python_environment_location as location
from molt import python_file_node_custody as files
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from tests.python_environment_test_support import runtime_identity_manifest


_PORTABLE_PATH_ALIASES = [
    pytest.param("tool", "TOOL", id="ascii-case"),
    pytest.param("caf\u00e9", "cafe\u0301", id="unicode-normalization"),
    pytest.param("stra\u00dfe", "STRASSE", id="unicode-casefold"),
]


def _tree(root: Path, context: files.PythonFileCaptureContext):
    pool = files._FileNodePool(capture_context=context)
    tree, _paths, _metadata = files._stable_tree_inventory(
        root, root_id="fixture", label="fixture", pool=pool
    )
    return tree, pool


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
    for name, data in (("b.py", b"b"), ("a.py", b"a"), ("c.py", b"c")):
        (tmp_path / name).write_bytes(data)
    context = files.PythonFileCaptureContext(hash_workers=workers)
    tree, pool = _tree(tmp_path, context)
    again, second_pool = _tree(tmp_path, context)
    assert tree == again
    assert pool.nodes == second_pool.nodes
    assert [row["sha256"] for row in pool.nodes] == [
        hashlib.sha256(data).hexdigest() for data in (b"a", b"b", b"c")
    ]
    assert context.inventory_profile()["hashed_files"] == 3
    assert context.inventory_profile()["hashed_bytes"] == 3
    assert len(context.file_custody()) == 3
    assert not any(isinstance(value, bytes) for value in vars(pool).values())
    assert pool.read_bound("file-node-0", label="fixture") == b"a"


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


def test_file_capture_rejects_same_size_mtime_restore(tmp_path):
    path = tmp_path / "a.py"
    path.write_bytes(b"before")
    context = files.PythonFileCaptureContext()
    before = path.stat()
    context.bind(path, before, label="fixture")
    path.write_bytes(b"after!")
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed"):
        context.bind(path, path.stat(), label="fixture")


def test_zero_inode_objects_use_distinct_path_identity(tmp_path, monkeypatch):
    paths = [tmp_path / "a.py", tmp_path / "b.py"]
    for path in paths:
        path.write_bytes(b"same")
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    # Zero-inode filesystems must not unify unrelated equal-content files.
    zero = os.stat_result((stat.S_IFREG | 0o600, 0, 0, 1, 0, 0, 4, 0, 0, 0))
    real_bind = context.bind
    monkeypatch.setattr(
        context,
        "bind",
        lambda path, _expected, *, label: real_bind(path, path.stat(), label=label),
    )
    assert pool.bind(paths[0], zero, label="zero inode") != pool.bind(
        paths[1], zero, label="zero inode"
    )
    assert len(pool.nodes) == 2


def test_capture_parallel_pending_work_is_bounded(tmp_path, monkeypatch):
    for index in range(40):
        (tmp_path / f"{index:02}.py").write_bytes(b"x")
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
            return map(function, work)

    monkeypatch.setattr(files, "ThreadPoolExecutor", Executor)
    _tree(tmp_path, files.PythonFileCaptureContext(hash_workers=2))
    assert windows == [8] * 5


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


def test_bare_runtime_capture_checks_earlier_files_before_return(tmp_path, monkeypatch):
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
        runtime.capture_current_python_runtime()


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
