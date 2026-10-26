"""Bounded synthetic proofs for shared Python content and location custody."""

from __future__ import annotations

import base64
import builtins
import copy
from contextlib import contextmanager
import hashlib
import io
import importlib.machinery
import json
import os
from pathlib import Path
import py_compile
import stat
import sys
from types import ModuleType, SimpleNamespace

import pytest
from tools.command_execution import CommandExecutor

from molt import python_capture as capture
from molt import python_environment_custody as environment
from molt import python_environment_identity as identity
from molt import python_environment_location as location
from molt import python_file_node_custody as files
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from tests.python_environment_test_support import (
    copy_python_capture_authority,
    environment_location_manifest,
    runtime_identity_manifest,
)
from tests.process_guard_common import install_module_view
from tests.module_isolation import forget_module


_COMMANDS = CommandExecutor.for_file(__file__)


_PORTABLE_PATH_ALIASES = [
    pytest.param("tool", "TOOL", id="ascii-case"),
    pytest.param("caf\u00e9", "cafe\u0301", id="unicode-normalization"),
    pytest.param("stra\u00dfe", "STRASSE", id="unicode-casefold"),
]


@pytest.mark.parametrize(
    "arguments",
    [
        ["--capture-runtime"],
        ["--runtime-selection"],
        ["--locate-active-environment"],
        ["--editable-project", "environment", "project"],
        ["--capture-environment", "environment", "--publish-bytecode"],
    ],
)
def test_published_admission_requires_read_only_environment_capture(
    monkeypatch, capsys, arguments
):
    monkeypatch.setattr(
        sys, "argv", ["identity", *arguments, "--admit-published-environment"]
    )
    with pytest.raises(SystemExit) as caught:
        identity._parse_arguments()
    assert caught.value.code == 2
    assert "published admission requires a read-only environment capture" in (
        capsys.readouterr().err
    )


@pytest.mark.parametrize(
    "arguments",
    [
        ["--development-checkout", "checkout"],
        ["--admit-published-environment", "--development-python", "3.12"],
        ["--admit-published-environment", "--development-group", "dev"],
    ],
)
def test_development_admission_requires_complete_operation_intent(
    monkeypatch, capsys, arguments
):
    monkeypatch.setattr(
        sys, "argv", ["identity", "--capture-active-environment", *arguments]
    )
    with pytest.raises(SystemExit) as caught:
        identity._parse_arguments()
    assert caught.value.code == 2
    assert "development intent requires published admission and a checkout" in (
        capsys.readouterr().err
    )


@pytest.mark.parametrize("development", [False, True])
@pytest.mark.parametrize("with_custody", [False, True])
@pytest.mark.parametrize("verification_fails", [False, True])
def test_published_admission_keeps_capture_context_until_final_verification(
    tmp_path, monkeypatch, capsys, with_custody, verification_fails, development
):
    from molt import source_build_environment
    from molt import input_report

    events = []
    realized = {"environment": "independent captured bytes"}
    payload = {"identity": realized} if with_custody else realized

    class Context:
        def __init__(self, *, hash_workers):
            assert hash_workers == 1

        def __enter__(self):
            events.append("context-open")
            return self

        def __exit__(self, *args):
            events.append("context-closed")

        def verify(self):
            events.append("verified")
            if verification_fails:
                raise PythonEnvironmentIdentityError("captured input changed")

    def capture(root, *, capture_context, **_kwargs):
        assert root == tmp_path and isinstance(capture_context, Context)
        events.append("captured")
        return payload

    def admit(root, observed):
        assert root == tmp_path and observed is realized
        assert events == [
            "context-open",
            "captured",
            *(["intent"] if development else []),
        ]
        events.append("admitted")

    def load(root, *, repo_root, python, groups):
        assert root == tmp_path and repo_root == tmp_path / "checkout"
        assert python == "3.14" and groups == ("bench", "dev")
        assert events == ["context-open", "captured"]
        events.append("intent")

    arguments = [
        "identity",
        "--capture-environment",
        str(tmp_path),
        "--admit-published-environment",
        *(["--with-custody"] if with_custody else []),
        *(
            [
                "--development-checkout",
                str(tmp_path / "checkout"),
                "--development-python",
                "3.14",
                "--development-group",
                "bench",
                "--development-group",
                "dev",
            ]
            if development
            else []
        ),
    ]
    monkeypatch.setattr(sys, "argv", arguments)
    monkeypatch.setattr(identity, "_arguments", identity._parse_arguments())
    monkeypatch.setattr(input_report, "worker_initializer", lambda _: None)
    monkeypatch.setattr(files, "PythonFileCaptureContext", Context)
    monkeypatch.setattr(identity, "capture_current_python_environment", capture)
    monkeypatch.setattr(source_build_environment, "admit_published_environment", admit)
    monkeypatch.setattr(source_build_environment, "load_development_environment", load)
    if verification_fails:
        with pytest.raises(
            PythonEnvironmentIdentityError, match="captured input changed"
        ):
            identity._main()
        assert not capsys.readouterr().out
    else:
        identity._main()
        assert json.loads(capsys.readouterr().out) == payload
    assert events == [
        "context-open",
        "captured",
        *(["intent"] if development else []),
        "admitted",
        "verified",
        "context-closed",
    ]


@pytest.mark.parametrize("fault", ["missing", "populated", "symlink"])
def test_identity_probe_refuses_unowned_cache_namespace(tmp_path, fault):
    prefix = tmp_path / "verifier"
    if fault == "populated":
        prefix.mkdir()
        (prefix / "cache.pyc").write_bytes(b"unadmitted")
    elif fault == "symlink":
        target = tmp_path / "target"
        target.mkdir()
        try:
            prefix.symlink_to(target, target_is_directory=True)
        except OSError:
            pytest.skip("directory symlinks unavailable")
    with pytest.raises(PythonEnvironmentIdentityError, match="fresh empty owned"):
        identity.python_identity_probe_arguments(
            ("--capture-runtime",), verifier_bytecode_prefix=prefix
        )


def test_image_cache_mapping_refuses_absent_verifier_prefix(tmp_path, monkeypatch):
    install_module_view(monkeypatch, "sys", sys, environment, pycache_prefix=None)
    with pytest.raises(PythonEnvironmentIdentityError, match="pre-start"):
        environment._image_cache_from_source(
            tmp_path / "demo.py", tmp_path / "image", 0
        )


@pytest.mark.parametrize("body_fails", [False, True])
def test_verifier_prefix_cleanup_keeps_primary_failure(
    tmp_path, monkeypatch, body_fails
):
    from molt import temporary_artifacts

    owners = []
    original_owner = temporary_artifacts.OwnedTemporaryDirectory
    original_delete = temporary_artifacts.delete_path

    class TrackedDirectory(original_owner):
        def __init__(self, *, prefix):
            super().__init__(prefix=prefix, dir=tmp_path)
            owners.append(self)

    def refuse_owned_cleanup(path):
        if owners and Path(path) == Path(owners[0].name):
            return False, "injected deletion refusal"
        return original_delete(path)

    primary = RuntimeError("original probe failure")
    try:
        with monkeypatch.context() as patch:
            patch.setattr(
                temporary_artifacts, "OwnedTemporaryDirectory", TrackedDirectory
            )
            patch.setattr(temporary_artifacts, "delete_path", refuse_owned_cleanup)
            with pytest.raises(RuntimeError if body_fails else OSError) as raised:
                with identity.PythonIdentityVerifier() as verifier:
                    prefix = verifier.prefix
                    assert prefix.is_dir()
                    if body_fails:
                        raise primary
            assert prefix.is_dir()
            if body_fails:
                assert raised.value is primary
                notes = "\n".join(primary.__notes__)
                assert str(prefix) in notes
                assert "OSError" in notes
                assert "injected deletion refusal" in notes
            else:
                assert "injected deletion refusal" in str(raised.value)
    finally:
        # Retry through the same allocation owner after restoring deletion;
        # the test never reconstructs ownership from the retained path.
        for owner in owners:
            owner.cleanup()


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
    disposable = tmp_path / "source"
    copy_python_capture_authority(disposable)
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
    for _ in range(2):
        with identity.PythonIdentityVerifier() as verifier:
            verifier_prefix = verifier.prefix
            command = [
                str(getattr(sys, "_base_executable", sys.executable)),
                *identity.python_identity_probe_arguments(
                    ("--capture-runtime",),
                    no_site=True,
                    verifier_bytecode_prefix=verifier_prefix,
                ),
            ]
            completed = verifier.run(
                command,
                cwd=disposable,
                env={**os.environ, "PYTHONDONTWRITEBYTECODE": "0"},
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=60,
                check=False,
                memory_guard_prefix=_COMMANDS.prefix,
            )
            assert not list(verifier_prefix.iterdir())
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
@pytest.mark.parametrize(
    "operating_system,py_debug,gil_disabled,archive_name,wrong_archive",
    [
        ("windows", False, False, "python314.zip", "python314t.zip"),
        ("windows", True, True, "python314_d.zip", "python314t_d.zip"),
        ("macos", True, True, "python314t.zip", "python314_d.zip"),
    ],
)
def test_runtime_capture_fences_import_and_startup_selection(
    tmp_path,
    monkeypatch,
    change,
    operating_system,
    py_debug,
    gil_disabled,
    archive_name,
    wrong_archive,
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
    archive = tmp_path / archive_name
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
        lambda: {
            "operating_system": operating_system,
            "architecture": "x86_64",
            "version": "3.14.4",
            "py_debug": py_debug,
            "gil_disabled": gil_disabled,
        },
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
    payload, roots, pool, _explicit = runtime._capture_runtime_with_context(
        capture_context=context
    )
    assert payload["import_roots"][-1] == {
        "kind": "absent-archive",
        "filename": archive_name,
    }
    environment_sys = SimpleNamespace(
        flags=SimpleNamespace(
            isolated=True, ignore_environment=True, no_user_site=True, safe_path=True
        ),
        path=[str(library), str(archive)],
    )
    monkeypatch.setattr(environment, "sys", environment_sys)
    active, _capabilities = environment._active_environment_import_roots(
        tmp_path / "environment", (), payload, roots, pool
    )
    assert active[-1] == {
        "role": "runtime-import-1",
        "owner": "runtime",
        "kind": "absent-archive",
        "filename": archive_name,
    }
    # Even a matching forged runtime row cannot admit the other OS/ABI name.
    wrong_payload = copy.deepcopy(payload)
    wrong_payload["import_roots"][-1]["filename"] = wrong_archive
    environment_sys.path[-1] = str(tmp_path / wrong_archive)
    with pytest.raises(PythonEnvironmentIdentityError, match="unsupported or absent"):
        environment._active_environment_import_roots(
            tmp_path / "environment", (), wrong_payload, roots, pool
        )
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
    foreign = tmp_path / "foreign.bin"
    foreign.write_bytes(b"unrelated descriptor traffic")
    opened = []
    open_stable = toolchain_identity.open_stable_regular_file

    def counted(path, **kwargs):
        if Path(path) in paths:
            opened.append(Path(path).name)
        return open_stable(path, **kwargs)

    monkeypatch.setattr(toolchain_identity, "open_stable_regular_file", counted)
    with toolchain_identity.open_stable_regular_file(
        foreign, label="foreign"
    ) as observed:
        assert observed.stream.read() == b"unrelated descriptor traffic"
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

    install_module_view(monkeypatch, "os", os, files, scandir=scan)
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
    assert environment["tree"]["file_count"] == 2 + 2 * len(packages)


@pytest.mark.parametrize(
    "kind,admitted",
    [
        ("copied", True),
        ("hardlink", True),
        ("same-root-symlink", True),
        ("base-runtime-symlink", True),
        ("different-content", False),
        ("nonexecutable", False),
        ("outside-scripts", False),
    ],
)
def test_environment_projects_only_captured_interpreter_aliases(
    tmp_path: Path, kind: str, admitted: bool
) -> None:
    from tests.python_environment_test_support import realized_environment_manifest

    payload = realized_environment_manifest(runtime_identity_manifest(), [])
    tree = payload["tree"]
    selected = payload["selected_executable"]
    selected_node = next(
        node for node in tree["file_nodes"] if node["id"] == selected["node"]
    )
    # Match the independently captured base image. An alias is justified by
    # captured content, not its filename or by a new walk of the live directory.
    selected_node["sha256"] = "a" * 64
    relative = (
        "library-mirror.exe"
        if kind == "outside-scripts"
        else f"{payload['scripts_root']}/retained-interpreter.exe"
    )
    entry = {
        "path": relative,
        "kind": "hardlink" if kind == "hardlink" else "file",
        "node": selected["node"],
        "access": {"readable": True, "writable": False, "executable": True},
    }
    if kind in {"copied", "different-content", "nonexecutable", "outside-scripts"}:
        node = {**selected_node, "id": f"file-node-{len(tree['file_nodes'])}"}
        if kind == "different-content":
            node["sha256"] = "b" * 64
        tree["file_nodes"].append(node)
        tree["node_ids"].append(node["id"])
        entry["node"] = node["id"]
    elif kind == "same-root-symlink":
        entry.update(kind="symlink", target_owner="same-root", target=selected["path"])
    elif kind == "base-runtime-symlink":
        entry.pop("node")
        entry.update(
            kind="symlink", target_owner="base-runtime", target_role="base-executable"
        )
    if kind == "nonexecutable":
        entry["access"]["executable"] = False
    tree["entries"].append(entry)
    tree["entries"].sort(key=lambda row: (row["path"].casefold(), row["path"]))
    tree["file_count"] += 1
    tree["manifest_sha256"] = canonical_json_sha256(tree["entries"])
    payload["environment_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in payload.items()
            if key != "environment_closure_sha256"
        }
    )
    base = tmp_path / "base.exe"
    base.write_bytes(b"synthetic base image; content is owned by the manifest")
    projected = identity.python_environment_executable_files(
        payload, tmp_path, base_executable=base, base_executable_selector=base
    )
    paths = {row["path"] for row in projected}
    assert (str(tmp_path / relative) in paths) is admitted
    assert str(tmp_path / selected["path"]) in paths
    assert str(base.resolve()) in paths


@pytest.mark.parametrize("target", ["captured", "foreign-samebytes", "wrong-node"])
def test_base_selector_projection_binds_the_exact_captured_target(tmp_path, target):
    from tests.python_environment_test_support import realized_environment_manifest

    data = b"captured base interpreter"
    physical = tmp_path / "physical" / "python.exe"
    foreign = tmp_path / "foreign" / "python.exe"
    for path in (physical, foreign):
        path.parent.mkdir()
        path.write_bytes(data)
    alias = tmp_path / "base-selector"
    try:
        alias.symlink_to(
            foreign.parent if target == "foreign-samebytes" else physical.parent,
            target_is_directory=True,
        )
    except OSError as exc:
        if os.name == "nt" and exc.winerror == 1314:
            pytest.skip("Windows host lacks symbolic-link creation privilege")
        raise
    selector = alias / "python.exe"
    assert selector.read_bytes() == physical.read_bytes() == foreign.read_bytes()
    runtime = runtime_identity_manifest()
    runtime["file_nodes"][0].update(
        size=len(data),
        sha256=hashlib.sha256(data if target != "wrong-node" else b"other").hexdigest(),
    )
    runtime["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in runtime.items()
            if key != "runtime_closure_sha256"
        }
    )
    payload = realized_environment_manifest(runtime, [])

    def project():
        return environment.python_environment_executable_files(
            payload,
            tmp_path,
            base_executable=physical,
            base_executable_selector=selector,
        )

    if target == "captured":
        rows = {row["role"]: row for row in project()}
        assert rows["base-interpreter"]["path"] == str(physical)
        assert rows["interpreter-alias:base-selector"] == {
            "role": "interpreter-alias:base-selector",
            "path": str(selector),
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    else:
        with pytest.raises(
            PythonEnvironmentIdentityError,
            match="captured physical base"
            if target == "foreign-samebytes"
            else "runtime node",
        ):
            project()


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
    wheel = environment["distributions"][0]["wheel"]
    wheel_node = next(
        row["node"]
        for row in environment["tree"]["entries"]
        if row["path"] == wheel["path"]
    )
    environment_pool = files._FileNodePool(capture_context=context)
    for index in range(len(environment["tree"]["file_nodes"])):
        path = tmp_path / f"file-{index}"
        if index:
            path.write_bytes(
                b"Wheel-Version: 1.0\nTag: py3-none-any\n"
                if environment["tree"]["file_nodes"][index]["id"] == wheel_node
                else f"environment-{index}".encode()
            )
        environment_pool.bind(path, path.stat(), label="environment")
    environment["tree"]["file_nodes"] = environment_pool.nodes
    wheel["sha256"] = next(
        row["sha256"] for row in environment_pool.nodes if row["id"] == wheel_node
    )
    environment["distribution_inventory_sha256"] = canonical_json_sha256(
        environment["distributions"]
    )
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
    assert capture.captured_file_for_node(envelope, runtime["file_nodes"][0])[
        "path"
    ] == str(shared)
    with pytest.raises(PythonEnvironmentIdentityError, match="unique capture custody"):
        capture.captured_file_for_node(envelope, dict(runtime["file_nodes"][0]))
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

    install_module_view(monkeypatch, "os", os, files, scandir=scan)
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
    payload = environment_location_manifest(
        prefix=prefix,
        selected_executable=executable,
        base_executable=executable,
        base_executable_selector=executable,
        roots=[prefix],
        external_roots=[],
        file_paths=[native],
    )
    assert location.validate_python_environment_location(payload) == payload
    from tools.proof_queue_pkg import execution_custody

    source = tmp_path / "source"
    source.mkdir()
    specs = execution_custody.watch_specs(
        source_root=source,
        tracked_paths=[],
        identities=[payload],
        broad_roots=[prefix],
    )
    exact = next(spec for spec in specs if spec.root == tmp_path)
    assert exact.owns(native)
    assert not exact.owns(tmp_path / "unrelated.dll")
    material = {
        key: value for key, value in payload.items() if key != "identity_sha256"
    }
    material["file_paths"] = ["relative-native.dll"]
    with pytest.raises(PythonEnvironmentIdentityError, match="canonical absolute"):
        location.validate_python_environment_location(
            {**material, "identity_sha256": canonical_json_sha256(material)}
        )


@pytest.mark.parametrize(
    "fault", ["missing", "v1", "relative", "parent", "nul", "extra"]
)
def test_location_v2_requires_an_explicit_absolute_base_selector(tmp_path, fault):
    prefix = tmp_path / "environment"
    executable = prefix / "python.exe"
    payload = environment_location_manifest(
        prefix=prefix,
        selected_executable=executable,
        base_executable=executable,
        base_executable_selector=executable,
        roots=[prefix],
        external_roots=[],
        file_paths=[executable],
    )
    payload.pop("identity_sha256")
    if fault == "missing":
        payload.pop("base_executable_selector")
    elif fault == "v1":
        payload["schema"] = "molt.python-environment-location.v1"
    elif fault == "relative":
        payload["base_executable_selector"] = "python.exe"
    elif fault == "parent":
        payload["base_executable_selector"] = str(prefix / ".." / "python.exe")
    elif fault == "nul":
        payload["base_executable_selector"] = str(executable) + "\x00"
    else:
        payload["implicit_base_selector"] = str(executable)
    payload["identity_sha256"] = canonical_json_sha256(payload)
    with pytest.raises(PythonEnvironmentIdentityError):
        location.validate_python_environment_location(payload)


def test_location_validation_never_resolves_a_declared_selector(tmp_path, monkeypatch):
    prefix = tmp_path / "absent-environment"
    executable = prefix / "python.exe"
    payload = environment_location_manifest(
        prefix=prefix,
        selected_executable=executable,
        base_executable=executable,
        base_executable_selector=tmp_path / "absent-base-selector" / "python.exe",
        roots=[prefix],
        external_roots=[],
        file_paths=[executable],
    )

    def forbidden(*_args, **_kwargs):
        raise AssertionError("pure locator validation consulted the filesystem")

    with monkeypatch.context() as patch:
        for name in ("resolve", "stat", "lstat"):
            patch.setattr(Path, name, forbidden)
        assert location.validate_python_environment_location(payload) == payload


@pytest.mark.parametrize("base_alias", [False, True])
def test_environment_locator_keeps_import_archive_as_exact_input(
    tmp_path, monkeypatch, base_alias
):
    prefix = tmp_path / "environment"
    prefix.mkdir()
    executable = prefix / "python.exe"
    executable.write_bytes(b"selected interpreter")
    base_selector = executable
    if base_alias:
        base_directory = tmp_path / "base-selector"
        try:
            base_directory.symlink_to(prefix, target_is_directory=True)
        except OSError as exc:
            if os.name == "nt" and exc.winerror == 1314:
                pytest.skip("Windows host lacks symbolic-link creation privilege")
            raise
        base_selector = base_directory / executable.name
    stdlib = prefix / "stdlib"
    stdlib.mkdir()
    archive = tmp_path / "system" / "python312.zip"
    archive.parent.mkdir()
    archive.write_bytes(b"selected import archive")
    native = archive.parent / "native.dll"
    native.write_bytes(b"selected native image")
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        location,
        executable=str(executable),
        _base_executable=str(base_selector),
        base_prefix=str(prefix),
        exec_prefix=str(prefix),
        base_exec_prefix=str(prefix),
        path=[str(archive)],
    )
    install_module_view(
        monkeypatch,
        "sysconfig",
        location.sysconfig,
        location,
        get_path=lambda _name: str(stdlib),
    )
    install_module_view(
        monkeypatch,
        "metadata",
        location.metadata,
        location,
        distributions=lambda **_kwargs: (),
    )
    monkeypatch.setattr(location, "_active_environment_prefix", lambda: prefix)
    monkeypatch.setattr(location, "_site_roots", lambda _prefix: (stdlib,))
    monkeypatch.setattr(location, "loaded_native_module_paths", lambda: (native,))

    located = location.locate_current_python_environment()

    assert located["roots"] == [str(prefix), str(stdlib)]
    assert located["file_paths"] == sorted([str(native), str(archive)])
    assert located["base_executable"] == str(executable.resolve(strict=True))
    assert located["base_executable_selector"] == str(base_selector)
    assert location.validate_python_environment_location(located) == located


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


@pytest.mark.parametrize("mutate", [False, True])
def test_installed_wheel_projection_uses_record_bound_bytes(tmp_path, mutate):
    _installed_fixture(tmp_path)
    wheel = tmp_path / "site/demo-1.0.dist-info/WHEEL"
    data = b"Wheel-Version: 1.0\nTag: cp313-cp313t-win_arm64\nBuild: 10a\n"
    wheel.write_bytes(data)
    record = wheel.with_name("RECORD")
    digest = (
        base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip("=")
    )
    with record.open("a") as stream:
        stream.write(f"demo-1.0.dist-info/WHEEL,sha256={digest},{len(data)}\n")
    tree, pool = _tree(tmp_path, files.PythonFileCaptureContext())
    if mutate:
        wheel.write_bytes(data.replace(b"cp313t", b"cp313d"))
    args = (
        ["site"],
        "scripts",
        {row["path"]: row for row in tree["entries"]},
        {row["id"]: row for row in pool.nodes},
        pool,
    )
    if mutate:
        with pytest.raises(PythonEnvironmentIdentityError):
            environment._installed_distributions(*args)
    else:
        distributions, owned, _scripts = environment._installed_distributions(*args)
        assert distributions[0]["wheel"] == {
            "path": "site/demo-1.0.dist-info/WHEEL",
            "sha256": hashlib.sha256(data).hexdigest(),
            "tags": ["cp313-cp313t-win_arm64"],
            "build": "10a",
        }
        assert "site/demo-1.0.dist-info/WHEEL" in owned


@pytest.mark.parametrize(
    "layout", ["normal", "pytest", "foreign-tag", "adjacent", "source", "unknown"]
)
def test_generated_cache_custody_matches_real_importability(tmp_path, layout):
    site = tmp_path / "site"
    site.mkdir()
    source = site / "retired.py"
    source.write_text("VALUE = 41\n", encoding="utf-8")
    tag = str(sys.implementation.cache_tag)
    filename = {
        "normal": f"retired.{tag}.pyc",
        "pytest": f"retired.{tag}-pytest-9.1.1.pyc",
        "foreign-tag": "retired.foreign-interpreter.pyc",
        "adjacent": "retired.pyc",
        "source": f"retired.{tag}.pyc",
        "unknown": "retired.pyc",
    }[layout]
    bytecode = (site if layout == "adjacent" else site / "__pycache__") / filename
    bytecode.parent.mkdir(exist_ok=True)
    py_compile.compile(str(source), cfile=str(bytecode), doraise=True)
    if layout != "source":
        source.unlink()

    # CPython is the independent oracle: orphan cache-directory bytecode is
    # ignored, whereas a legacy adjacent .pyc is importable without source.
    probe = _COMMANDS.run(
        [
            sys.executable,
            "-I",
            "-S",
            "-B",
            "-c",
            "import importlib.util,json,sys;sys.path.insert(0,sys.argv[1]);"
            "spec=importlib.util.find_spec('retired');"
            "print(json.dumps({'found':spec is not None,"
            "'value':__import__('retired').VALUE if spec else None}))",
            str(site),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
        check=False,
    )
    assert probe.returncode == 0, probe.stderr
    importable = layout in {"adjacent", "source"}
    assert json.loads(probe.stdout) == {
        "found": importable,
        "value": 41 if importable else None,
    }

    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    tree, _, metadata = files._stable_tree_inventory(
        tmp_path, root_id="fixture", label="fixture", pool=pool
    )
    entries = {row["path"]: row for row in tree["entries"]}
    relative = bytecode.relative_to(tmp_path).as_posix()
    result = environment._bytecode_disposition(
        relative,
        tmp_path,
        set(),
        entries,
        metadata,
        pool,
        pytest_version=None,
        source_paths_by_key={
            files._portable_path_key(p): p
            for p in entries
            if p.casefold().endswith(".py")
        },
        excluded_path_keys=frozenset(),
    )
    inert = layout in {"normal", "pytest", "foreign-tag"}
    assert result is (
        environment._BytecodeDisposition.INERT
        if inert
        else environment._BytecodeDisposition.UNOWNED
    )
    if inert:
        # Inert is still exact input, never an exclusion from file custody.
        node = next(row for row in pool.nodes if row["id"] == entries[relative]["node"])
        assert node["sha256"] == hashlib.sha256(bytecode.read_bytes()).hexdigest()
        context.verify()
        bytecode.write_bytes(bytecode.read_bytes() + b"changed inert bytes")
        with pytest.raises(ValueError, match="changed"):
            context.verify()


@pytest.mark.parametrize(
    "fault",
    ["none", "header", "code", "source", "pytest-none", "pytest-other", "pytest"],
)
def test_owned_active_bytecode_keeps_source_and_tool_guards(tmp_path, fault):
    source = _installed_fixture(tmp_path)
    rewrite = fault.startswith("pytest")
    suffix = "-pytest-9.1.1" if rewrite else ""
    bytecode = (
        source.parent
        / "__pycache__"
        / f"demo.{sys.implementation.cache_tag}{suffix}.pyc"
    )
    bytecode.parent.mkdir()
    py_compile.compile(str(source), cfile=str(bytecode), doraise=True)
    if fault == "header":
        bytecode.write_bytes(b"BAD!" + bytecode.read_bytes()[4:])
    elif fault == "code":
        bytecode.write_bytes(bytecode.read_bytes()[:-1] + b"!")
    elif fault == "source":
        stamp = source.stat()
        os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns + 2_000_000_000))
    pool = files._FileNodePool(capture_context=files.PythonFileCaptureContext())
    tree, _, metadata = files._stable_tree_inventory(
        tmp_path, root_id="fixture", label="fixture", pool=pool
    )
    entries = {row["path"]: row for row in tree["entries"]}
    # The independent installed RECORD owns source, never generated caches.
    _, owned, _ = environment._installed_distributions(
        ["site"], "scripts", entries, {row["id"]: row for row in pool.nodes}, pool
    )
    version = (
        None
        if fault == "pytest-none"
        else "different"
        if fault == "pytest-other"
        else "9.1.1"
    )

    def classify():
        return environment._bytecode_disposition(
            bytecode.relative_to(tmp_path).as_posix(),
            tmp_path,
            owned,
            entries,
            metadata,
            pool,
            pytest_version=version,
            source_paths_by_key={
                files._portable_path_key(p): p
                for p in entries
                if p.casefold().endswith(".py")
            },
            excluded_path_keys=frozenset(),
        )

    if fault == "none":
        assert classify() is environment._BytecodeDisposition.OWNED
    else:
        with pytest.raises(PythonEnvironmentIdentityError):
            classify()


@pytest.mark.parametrize("guard", ["code", "rewrite-tool"])
def test_active_cache_case_alias_is_not_inert(tmp_path, guard):
    source = _installed_fixture(tmp_path)
    original = source.read_bytes()
    stamp = source.stat()
    suffix = "-pytest-9.1.1" if guard == "rewrite-tool" else ""
    cache = (
        source.parent
        / "__pycache__"
        / f"DEMO.{sys.implementation.cache_tag}{suffix}.pyc"
    )
    cache.parent.mkdir()
    # Preserve source metadata but give the cache distinct observable behavior.
    source.write_bytes(b"VALUE = 2\n")
    os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    py_compile.compile(str(source), cfile=str(cache), doraise=True)
    source.write_bytes(original)
    os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    if guard == "code":
        aliases = cache.with_name(cache.name.replace("DEMO.", "demo.", 1)).exists()
        probe = _COMMANDS.run(
            [
                sys.executable,
                "-I",
                "-S",
                "-B",
                "-c",
                "import sys;sys.path.insert(0,sys.argv[1]);import demo;print(demo.VALUE)",
                str(source.parent),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
            check=False,
        )
        assert probe.returncode == 0, probe.stderr
        assert probe.stdout.strip() == ("2" if aliases else "1")
    pool = files._FileNodePool(capture_context=files.PythonFileCaptureContext())
    tree, _, metadata = files._stable_tree_inventory(
        tmp_path, root_id="fixture", label="fixture", pool=pool
    )
    entries = {row["path"]: row for row in tree["entries"]}
    _, owned, _ = environment._installed_distributions(
        ["site"], "scripts", entries, {row["id"]: row for row in pool.nodes}, pool
    )
    with pytest.raises(PythonEnvironmentIdentityError):
        environment._bytecode_disposition(
            cache.relative_to(tmp_path).as_posix(),
            tmp_path,
            owned,
            entries,
            metadata,
            pool,
            pytest_version=None,
            source_paths_by_key={
                files._portable_path_key(p): p
                for p in entries
                if p.casefold().endswith(".py")
            },
            excluded_path_keys=frozenset(),
        )


def test_explicitly_excluded_importable_source_is_not_an_orphan(tmp_path):
    source = tmp_path / "site" / "demo.py"
    source.parent.mkdir()
    source.write_text("VALUE = 41\n", encoding="utf-8")
    cache = source.parent / "__pycache__" / f"demo.{sys.implementation.cache_tag}.pyc"
    cache.parent.mkdir()
    py_compile.compile(str(source), cfile=str(cache), doraise=True)
    excluded = frozenset({"site/demo.py"})
    pool = files._FileNodePool(capture_context=files.PythonFileCaptureContext())
    tree, _, metadata = files._stable_tree_inventory(
        tmp_path, root_id="fixture", label="fixture", pool=pool, excluded=excluded
    )
    entries = {row["path"]: row for row in tree["entries"]}
    assert "site/demo.py" not in entries
    assert source.is_file()
    assert (
        environment._bytecode_disposition(
            cache.relative_to(tmp_path).as_posix(),
            tmp_path,
            set(),
            entries,
            metadata,
            pool,
            pytest_version=None,
            source_paths_by_key={},
            excluded_path_keys=frozenset(files._portable_path_key(p) for p in excluded),
        )
        is environment._BytecodeDisposition.UNOWNED
    )


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
    monkeypatch.setattr(
        runtime, "_runtime_import_candidates", lambda _base, _platform: ([], [])
    )
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


class _CustodyImportReached(Exception):
    """Stop the real facade at the first package import, after admission."""


def _execute_probe_to_package_import(source: Path, imports: list[str]) -> None:
    original_import = builtins.__import__

    def observe_import(name, *args, **kwargs):
        if name == "tools" or name.startswith("tools."):
            raise AssertionError("installed probe requires repository-only tools")
        if name == "molt" or name.startswith("molt."):
            imports.append(name)
            raise _CustodyImportReached(name)
        return original_import(name, *args, **kwargs)

    # Execute the complete, current facade, not a copied bootstrap algorithm or
    # AST-selected prefix. The import observer is only the stopping boundary;
    # package/source validation and standard helper loading are production code.
    exec(
        compile(source.read_bytes(), str(source), "exec"),
        {
            "__name__": "__main__",
            "__package__": "",
            "__file__": str(source),
            "__builtins__": {**vars(builtins), "__import__": observe_import},
        },
    )


@pytest.fixture
def standalone_probe_source(tmp_path, monkeypatch):
    installed = tmp_path / "installed" / "molt"
    installed.mkdir(parents=True)
    original = Path(identity.__file__).resolve().parent
    for name in ("python_environment_identity.py", "package_import_custody.py"):
        (installed / name).write_bytes((original / name).read_bytes())
    (installed / "__init__.py").write_text("", encoding="utf-8")
    for name in tuple(sys.modules):
        if name == "molt" or name.startswith("molt."):
            monkeypatch.delitem(sys.modules, name)
    forget_module(monkeypatch, "_molt_package_import_custody")
    monkeypatch.setattr(sys, "path", list(sys.path))
    return (installed / "python_environment_identity.py").resolve()


@pytest.mark.parametrize("package_state", ["absent", "selected", "namespace"])
def test_standalone_probe_admits_selected_package_without_repository_tools(
    standalone_probe_source, package_state, monkeypatch
):
    source = standalone_probe_source
    selected = source.parent
    if package_state != "absent":
        root = ModuleType("molt")
        root.__file__ = str(selected / "__init__.py")
        root.__path__ = [str(selected)]
        monkeypatch.setitem(sys.modules, "molt", root)
    if package_state == "namespace":
        nested = selected / "plugins"
        nested.mkdir()
        namespace = ModuleType("molt.plugins")
        namespace.__path__ = [str(nested)]
        spec = importlib.machinery.ModuleSpec("molt.plugins", None, is_package=True)
        spec.submodule_search_locations = list(namespace.__path__)
        namespace.__spec__ = spec
        monkeypatch.setitem(sys.modules, "molt.plugins", namespace)
        prior_path, prior_spec = namespace.__path__, namespace.__spec__
    imports = []
    with pytest.raises(_CustodyImportReached, match="molt.python_identity_common"):
        _execute_probe_to_package_import(source, imports)
    assert imports == ["molt.python_identity_common"]
    assert sys.path[0] == str(selected.parent)
    authority = sys.modules["_molt_package_import_custody"]
    assert authority.__file__ == str(selected / "package_import_custody.py")
    assert authority.__package__ == ""
    assert not (selected.parent / "tools").exists()
    if package_state == "namespace":
        assert namespace.__path__ is prior_path
        assert namespace.__spec__ is prior_spec


@pytest.mark.parametrize("encoding", ["ascii", "cp1252"])
def test_standalone_package_refusal_preserves_unicode_on_diagnostic_wire(
    standalone_probe_source, tmp_path, monkeypatch, encoding
):
    foreign = tmp_path / "foreign-λ" / "molt"
    foreign.mkdir(parents=True)
    package = ModuleType("molt")
    package.__file__ = str(foreign / "__init__.py")
    package.__path__ = [str(foreign)]
    monkeypatch.setitem(sys.modules, "molt", package)
    imports = []
    output = io.BytesIO()
    stdout = sys.stdout
    with io.TextIOWrapper(output, encoding=encoding, errors="backslashreplace") as wire:
        with monkeypatch.context() as streams:
            streams.setattr(sys, "stderr", wire)
            with pytest.raises(SystemExit) as caught:
                _execute_probe_to_package_import(standalone_probe_source, imports)
            wire.flush()
            diagnostic = output.getvalue().decode("utf-8", errors="strict")
        assert caught.value.code == 2
        assert str(foreign) in diagnostic
        assert str(standalone_probe_source.parent) in diagnostic
        assert "\\u03bb" not in diagnostic
        assert imports == []
        assert sys.stdout is stdout


@pytest.mark.parametrize("failure", ["arguments", "package"])
def test_standalone_refusal_uses_utf8_after_ascii_child_pipe_startup(tmp_path, failure):
    selected = tmp_path / "selected-λ"
    copy_python_capture_authority(selected)
    foreign = tmp_path / "foreign-λ" / "molt"
    foreign.mkdir(parents=True)
    (foreign / "__init__.py").write_bytes(
        (selected / "src/molt/__init__.py").read_bytes()
    )
    invalid_argument = "--unknown-λ"
    startup = (
        "sys.argv = [sys.argv[1], '--runtime-selection', sys.argv[2]]; "
        if failure == "arguments"
        else "sys.path.insert(0, sys.argv[2]); import molt; "
        "sys.argv = [sys.argv[1], '--runtime-selection']; "
    )
    script = (
        "import runpy, sys; "
        "sys.stderr.reconfigure(encoding='ascii', errors='backslashreplace'); "
        + startup
        + "runpy.run_path(sys.argv[0], run_name='__main__')"
    )
    with identity.PythonIdentityVerifier() as verifier:
        prefix = verifier.prefix
        completed = verifier.run(
            [
                str(getattr(sys, "_base_executable", sys.executable)),
                *identity.python_identity_verifier_arguments(prefix),
                "-S",
                "-c",
                script,
                str(selected / "src/molt/python_environment_identity.py"),
                invalid_argument if failure == "arguments" else str(foreign.parent),
            ],
            cwd=tmp_path,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="strict",
            timeout=30,
            check=False,
            memory_guard_prefix=_COMMANDS.prefix,
        )
        assert not list(prefix.iterdir())
    assert completed.returncode == 2
    if failure == "arguments":
        assert invalid_argument in completed.stderr
    else:
        assert "package source admission failed" in completed.stderr
        assert str(selected / "src/molt") in completed.stderr
        assert str(foreign) in completed.stderr
    assert "\\u03bb" not in completed.stderr
    assert completed.stdout == ""


@pytest.mark.parametrize("foreign_kind", ["root", "descendant", "namespace"])
@pytest.mark.parametrize("authority_preloaded", [False, True])
def test_standalone_probe_refuses_foreign_package_before_custody_imports(
    standalone_probe_source, foreign_kind, authority_preloaded, monkeypatch, capsys
):
    source = standalone_probe_source
    selected = source.parent
    foreign = selected.parent.parent / "foreign" / "molt"
    foreign.mkdir(parents=True)
    root = ModuleType("molt")
    root.__file__ = str(selected / "__init__.py")
    root.__path__ = [str(selected)]
    monkeypatch.setitem(sys.modules, "molt", root)
    poisoned = root
    if foreign_kind == "root":
        root.__file__ = str(foreign / "__init__.py")
        root.__path__ = [str(foreign)]
        # A descendant loaded through the foreign root (CI's custody hook can
        # import one mid-test) must not hide the root as the reported cause.
        stray = ModuleType("molt.stray")
        stray.__file__ = str(selected.parent.parent / "elsewhere" / "stray.py")
        monkeypatch.setitem(sys.modules, "molt.stray", stray)
    elif foreign_kind == "descendant":
        poisoned = ModuleType("molt.foreign")
        poisoned.__file__ = str(foreign / "foreign.py")
        monkeypatch.setitem(sys.modules, "molt.foreign", poisoned)
    else:
        nested = selected / "plugins"
        nested.mkdir()
        poisoned = ModuleType("molt.plugins")
        poisoned.__path__ = [str(nested), str(foreign)]
        spec = importlib.machinery.ModuleSpec("molt.plugins", None, is_package=True)
        spec.submodule_search_locations = list(poisoned.__path__)
        poisoned.__spec__ = spec
        monkeypatch.setitem(sys.modules, "molt.plugins", poisoned)
    prior_metadata = vars(poisoned).copy()
    prior_locations = tuple(getattr(poisoned, "__path__", ()))
    prior_spec_locations = tuple(
        getattr(getattr(poisoned, "__spec__", None), "submodule_search_locations", ())
        or ()
    )
    if authority_preloaded:
        prior_authority = identity._load_package_import_custody(
            selected / "package_import_custody.py"
        )
    before_path = list(sys.path)
    before_packages = {
        name: module
        for name, module in sys.modules.items()
        if name == "molt" or name.startswith("molt.")
    }
    imports = []
    with pytest.raises(SystemExit) as raised:
        _execute_probe_to_package_import(source, imports)
    assert raised.value.code == 2
    assert imports == []
    diagnostic = capsys.readouterr().err
    assert "package source admission failed" in diagnostic
    assert str(selected) in diagnostic
    assert str(foreign) in diagnostic
    assert sys.path == before_path
    assert vars(poisoned) == prior_metadata
    assert tuple(getattr(poisoned, "__path__", ())) == prior_locations
    assert (
        tuple(
            getattr(
                getattr(poisoned, "__spec__", None), "submodule_search_locations", ()
            )
            or ()
        )
        == prior_spec_locations
    )
    assert {
        name: module
        for name, module in sys.modules.items()
        if name == "molt" or name.startswith("molt.")
    } == before_packages
    if authority_preloaded:
        assert sys.modules["_molt_package_import_custody"] is prior_authority
    else:
        assert "_molt_package_import_custody" not in sys.modules


def test_standalone_probe_preserves_explicit_failed_authority_binding(
    standalone_probe_source, monkeypatch, capsys
):
    monkeypatch.setitem(sys.modules, "_molt_package_import_custody", None)
    before_path = list(sys.path)
    imports = []
    with pytest.raises(SystemExit) as raised:
        _execute_probe_to_package_import(standalone_probe_source, imports)
    assert raised.value.code == 2
    assert imports == []
    assert "already loaded from another authority" in capsys.readouterr().err
    assert sys.path == before_path
    assert "_molt_package_import_custody" in sys.modules
    assert sys.modules["_molt_package_import_custody"] is None


def test_capture_authority_preserves_installed_package_and_explicit_source_owners(
    tmp_path, monkeypatch
):
    installed = tmp_path / "installation" / "molt"
    selected = tmp_path / "compiler-source"
    monkeypatch.setattr(
        identity, "__file__", str(installed / "python_environment_identity.py")
    )
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(selected))

    captured = identity.python_capture_authority_paths()
    assert installed / "python_environment_identity.py" in captured
    assert installed / "package_import_custody.py" in captured
    assert selected / "src/molt/package_import_custody.py" not in captured

    projected = identity.python_capture_authority_paths(source_root=selected)
    assert selected / "src/molt/python_environment_identity.py" in projected
    assert selected / "src/molt/package_import_custody.py" in projected
    assert installed / "package_import_custody.py" not in projected


def test_probe_reader_selection_uses_explicit_source_without_ambient_fallback(
    tmp_path, monkeypatch
):
    selected = tmp_path / "selected"
    script = selected / "src/molt/python_environment_identity.py"
    script.parent.mkdir(parents=True)
    script.write_bytes(Path(identity.__file__).read_bytes())
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(tmp_path / "unrelated"))
    with identity.PythonIdentityVerifier() as verifier:
        prefix = verifier.prefix
        arguments = identity.python_identity_probe_arguments(
            ("--capture-runtime",),
            verifier_bytecode_prefix=prefix,
            source_root=selected,
        )
        assert str(script.resolve()) in arguments
        assert str(Path(identity.__file__).resolve()) not in arguments
        assert script in identity.python_capture_authority_paths(source_root=selected)
        script.unlink()
        with pytest.raises(FileNotFoundError):
            identity.python_identity_probe_arguments(
                ("--capture-runtime",),
                verifier_bytecode_prefix=prefix,
                source_root=selected,
            )


@pytest.mark.parametrize(
    "name", ["default_paths", "windows_process_api", "flag_strings"]
)
def test_promoted_leaf_belongs_to_loaded_and_projected_python_custody(
    tmp_path,
    name,
):
    captured = identity.python_capture_authority_paths()
    assert Path(identity.__file__).parent / f"{name}.py" in captured
    assert not any(path.as_posix().endswith(f"molt/cli/{name}.py") for path in captured)
    assert tmp_path / f"src/molt/{name}.py" in identity.python_capture_authority_paths(
        source_root=tmp_path
    )


@pytest.mark.parametrize(
    "fault", ["none", "forged-code", "rewrite", "image-prefix", "populated-prefix"]
)
def test_sealed_ordinary_prefix_derives_code_and_rejects_rewrite(tmp_path, fault):
    # Interpreter-wide cache policy belongs only to spawned children, never the
    # pytest process with concurrently active guard and sentinel threads.
    with identity.PythonIdentityVerifier() as verifier:
        verifier_prefix = verifier.prefix
        command = [
            sys.executable,
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={verifier_prefix}",
            "-c",
            "import sys; from pathlib import Path; "
            f"sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r}); "
            "from tests.test_python_environment_identity import _sealed_ordinary_prefix_control; "
            "_sealed_ordinary_prefix_control(Path(sys.argv[1]), sys.argv[2])",
            str(tmp_path),
            fault,
        ]
        result = verifier.run(
            command,
            cwd=tmp_path,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
            check=False,
            memory_guard_prefix=_COMMANDS.prefix,
        )
        assert result.returncode == 0, result.stderr


def _sealed_ordinary_prefix_control(tmp_path: Path, fault: str):
    """py_compile and real pytest rewriting are independent producer controls."""
    import ast
    import importlib.util
    import marshal
    import struct
    from _pytest.assertion.rewrite import rewrite_asserts

    root = tmp_path / "image"
    source = _installed_fixture(root)
    prefix = location.environment_bytecode_prefix(root)
    prefix.mkdir()
    verifier_prefix = sys.pycache_prefix
    assert verifier_prefix is not None
    assert Path(verifier_prefix) != prefix
    tree, pool = _tree(root, files.PythonFileCaptureContext())
    entries = {row["path"]: row for row in tree["entries"]}
    _, owned, _ = environment._installed_distributions(
        ["site"], "scripts", entries, {row["id"]: row for row in pool.nodes}, pool
    )
    environment._ordinary_bytecode_projection(
        root,
        entries,
        owned,
        pool,
        {"runtime_roots": []},
        (),
        pool,
        {"trees": []},
        (),
        publish=True,
    )
    assert sys.pycache_prefix == verifier_prefix
    assert not list(Path(verifier_prefix).iterdir())
    # Ask a separate interpreter selecting the image namespace for its native
    # paths; expected names do not use the receiver's suffix-rebasing helper.
    mapped = _COMMANDS.run(
        [
            sys.executable,
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-c",
            "import importlib.util,json,sys; "
            "print(json.dumps([importlib.util.cache_from_source(sys.argv[1], "
            "optimization=o) for o in ('',1,2)]))",
            str(source),
        ],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
        check=False,
    )
    assert mapped.returncode == 0, mapped.stderr
    cache_paths = [Path(value) for value in json.loads(mapped.stdout)]
    for optimization in (0, 1, 2):
        cache = cache_paths[optimization]
        oracle = tmp_path / f"oracle-{optimization}.pyc"
        py_compile.compile(
            str(source),
            cfile=str(oracle),
            doraise=True,
            invalidation_mode=py_compile.PycInvalidationMode.CHECKED_HASH,
            optimize=optimization,
        )
        assert cache.read_bytes() == oracle.read_bytes()
    cache = cache_paths[0]
    if fault == "forged-code":
        # Keep the complete valid checked-hash header. A filename/header waiver
        # would accept executable bytes that have never derived from this source.
        cache.write_bytes(
            cache.read_bytes()[:16]
            + marshal.dumps(compile("VALUE = 99\n", str(source), "exec"))
        )
    elif fault == "rewrite":
        tree = ast.parse("assert 1 == 2\n", filename=str(source))
        rewrite_asserts(tree, b"assert 1 == 2\n", str(source))
        rewritten = compile(tree, str(source), "exec", dont_inherit=True)
        rewrite_cache = cache.with_name(
            cache.stem + f"-pytest-{pytest.__version__}.pyc"
        )
        stamp = source.stat()
        rewrite_cache.write_bytes(
            importlib.util.MAGIC_NUMBER
            + struct.pack("<III", 0, int(stamp.st_mtime), stamp.st_size)
            + marshal.dumps(rewritten)
        )
    elif fault == "image-prefix":
        sys.pycache_prefix = str(prefix)
    elif fault == "populated-prefix":
        (Path(verifier_prefix) / "unadmitted.pyc").write_bytes(b"foreign cache")
    captured, captured_pool = _tree(root, files.PythonFileCaptureContext())

    def admit():
        environment._ordinary_bytecode_projection(
            root,
            {row["path"]: row for row in captured["entries"]},
            owned,
            captured_pool,
            {"runtime_roots": []},
            (),
            captured_pool,
            {"trees": []},
            (),
            publish=False,
        )

    if fault == "none":
        admit()
        assert sys.pycache_prefix == verifier_prefix
        assert not list(Path(verifier_prefix).iterdir())
        # A cache hit must remain usable under the mandatory pre-start policy.
        # Making source compilation fail distinguishes an actual hit from a
        # source execution that happens to return the same value.
        command = [
            sys.executable,
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={prefix}",
            "-c",
            "import importlib.machinery,sys; "
            "importlib.machinery.SourceFileLoader.source_to_code = lambda *a,**k: (_ for _ in ()).throw(AssertionError('cache miss')); "
            f"sys.path.insert(0, {str(source.parent)!r}); import demo; print(demo.VALUE)",
        ]
        result = _COMMANDS.run(
            command,
            cwd=tmp_path,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        assert result.stdout.strip() == "1"
    else:
        with pytest.raises(PythonEnvironmentIdentityError):
            admit()


def test_immutable_image_excludes_only_mutable_editable_source_bytes():
    from tests.python_environment_test_support import realized_environment_manifest

    payload = realized_environment_manifest(runtime_identity_manifest(), [])
    payload["external_import_custody"]["trees"] = [
        {
            "id": "external-tree-0",
            "source_root": "source-0",
            "source_path": "src",
            "roles": [{"role": "sys-path", "path": "."}],
            "entries": [{"path": "demo.py", "node": "old"}],
            "file_nodes": [{"id": "old"}],
            "manifest_sha256": "old",
            "file_count": 1,
        }
    ]
    original = environment.immutable_environment_image(payload)
    changed = copy.deepcopy(payload)
    changed["external_import_custody"]["trees"][0]["entries"][0]["node"] = "new"
    changed["environment_closure_sha256"] = "new"
    assert environment.immutable_environment_image(changed) == original
    changed["external_import_custody"]["trees"][0]["source_path"] = "other"
    assert environment.immutable_environment_image(changed) != original


@pytest.mark.parametrize(
    "failure_kind", ["timeout", "guard", "interrupt", "receiptless"]
)
@pytest.mark.parametrize("closed", [False, True])
def test_verifier_owner_preserves_guard_fact_and_poisoned_namespace(
    tmp_path, monkeypatch, failure_kind, closed
):
    import gc
    import subprocess
    import weakref
    from types import SimpleNamespace
    from molt import process_guard, temporary_artifacts
    from tests.process_guard_common import guard_result_for_test

    # Receiptless exceptions cannot authorize cleanup, even if a test labels
    # the other branch closed. The producer must prove closure explicitly.
    proven_closed = closed and failure_kind != "receiptless"
    receipt = SimpleNamespace(
        descendants_closed=closed,
        child_returncode=None,
        infrastructure_failure=SimpleNamespace(
            phase="drain", details=("independent failure",)
        ),
    )
    if failure_kind == "timeout":
        failure = subprocess.TimeoutExpired(["identity"], 1, output=b"preserved")
    elif failure_kind == "guard":
        failure = process_guard.GuardInfrastructureError(["identity"], receipt)
    elif failure_kind == "interrupt":
        failure = KeyboardInterrupt()
    else:
        failure = FileNotFoundError("spawn raced with executable retirement")
    if failure_kind != "receiptless":
        failure.guarded_result = receipt
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        assert kwargs["memory_guard_prefix"] == "MOLT_PYTHON_IDENTITY"
        if len(calls) == 1:
            raise failure
        return guard_result_for_test(command, 0, "", "", descendants_closed=True)

    monkeypatch.setattr(process_guard, "run_completed_command", run)

    def exercise():
        nonlocal path, allocation_identity, allocation_ref
        with identity.PythonIdentityVerifier() as owner:
            path = owner.prefix
            allocation_identity = temporary_artifacts._identity(path)
            allocation_ref = weakref.ref(owner._allocation)
            with pytest.raises(type(failure)) as caught:
                owner.run(
                    ["identity", "--help"], memory_guard_prefix="MOLT_PYTHON_IDENTITY"
                )
            assert caught.value is failure
            if failure_kind != "receiptless":
                assert caught.value.guarded_result is receipt
            assert path.is_dir()
            if proven_closed:
                owner.run(
                    ["second-identity"], memory_guard_prefix="MOLT_PYTHON_IDENTITY"
                )
            else:
                with pytest.raises(type(failure)) as repeated:
                    owner.run(
                        ["second-identity"], memory_guard_prefix="MOLT_PYTHON_IDENTITY"
                    )
                assert repeated.value is failure

    path = allocation_identity = allocation_ref = None
    if proven_closed:
        exercise()
    else:
        with pytest.raises(type(failure)) as terminal:
            exercise()
        assert terminal.value is failure
        del terminal
    assert len(calls) == (2 if proven_closed else 1)
    assert path.exists() is (not proven_closed)
    failure.__traceback__ = None
    gc.collect()
    assert allocation_ref() is None
    if not proven_closed:
        assert path.is_dir()
        temporary_artifacts._cleanup_temporary_directory(path, allocation_identity)


@pytest.mark.parametrize("outcome", ["timeout", "unclosed", "absent-closure"])
def test_verifier_owner_unknown_result_retains_after_swallowed_error_and_gc(
    monkeypatch, outcome
):
    import gc
    import subprocess
    import weakref
    from types import SimpleNamespace
    from molt import process_guard, temporary_artifacts
    from tests.process_guard_common import guard_result_for_test

    if outcome == "timeout":
        receipt = SimpleNamespace(descendants_closed=False)
        failure = subprocess.TimeoutExpired(["first"], 1)
        failure.guarded_result = receipt
    elif outcome == "unclosed":
        receipt = guard_result_for_test(
            ["first"], 0, "apparent success", "", descendants_closed=False
        )
        failure = None
    else:
        receipt = SimpleNamespace(returncode=0, stdout="apparent success", stderr="")
        failure = None
    error_type = subprocess.TimeoutExpired if outcome == "timeout" else RuntimeError
    calls = []

    def run(command, **_kwargs):
        calls.append(command)
        if failure is not None:
            raise failure
        return receipt

    monkeypatch.setattr(process_guard, "run_completed_command", run)
    with pytest.raises(error_type) as outer:
        with identity.PythonIdentityVerifier() as owner:
            path = owner.prefix
            allocation_identity = temporary_artifacts._identity(path)
            allocation_ref = weakref.ref(owner._allocation)
            with pytest.raises(error_type) as inner:
                owner.run(["first"], memory_guard_prefix="MOLT_PYTHON_IDENTITY")
            original = inner.value
            if failure is not None:
                assert original is failure
            assert original.guarded_result is receipt
            del inner
            with pytest.raises(error_type) as repeated:
                owner.run(
                    ["would-mask-first"], memory_guard_prefix="MOLT_PYTHON_IDENTITY"
                )
            assert repeated.value is original
            del repeated
    assert outer.value is original and outer.value.guarded_result is receipt
    assert len(calls) == 1 and path.is_dir()
    original.__traceback__ = None
    del outer, owner
    gc.collect()
    assert allocation_ref() is None and path.is_dir()
    temporary_artifacts._cleanup_temporary_directory(path, allocation_identity)
