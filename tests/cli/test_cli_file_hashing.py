from __future__ import annotations

import os
import time
from pathlib import Path

import pytest

from molt import file_hashing


def test_sha256_primitives_share_file_hashing_authority(tmp_path: Path) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"abc")
    expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

    assert file_hashing._sha256_bytes(b"abc") == expected
    assert file_hashing._sha256_file_with_size(path) == (expected, 3)
    assert file_hashing._sha256_file(path) == expected


def test_source_fingerprint_files_are_deterministic_and_filtered(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    source_root = root / "src"
    pycache = source_root / "__pycache__"
    pycache.mkdir(parents=True)
    (source_root / "b.rs").write_text("pub fn b() {}\n", encoding="utf-8")
    (source_root / "a.py").write_text("print('a')\n", encoding="utf-8")
    (source_root / "z.pyc").write_bytes(b"bytecode")
    (pycache / "a.pyc").write_bytes(b"bytecode")

    _git(root, "init", "-q")
    (root / ".gitignore").write_text("__pycache__/\n*.pyc\n*.pyo\n", encoding="utf-8")
    files = [
        path.relative_to(root).as_posix()
        for path in file_hashing._source_fingerprint_files(source_root)
    ]

    assert files == ["src/a.py", "src/b.rs"]
    metadata = file_hashing._hash_source_tree_metadata([source_root], root)
    assert metadata is not None
    assert metadata[1] == 2


def test_content_change_time_path_and_descriptor_agree_for_one_generation(
    tmp_path: Path,
) -> None:
    path = tmp_path / "content.bin"
    # A whole-second lower bound admits filesystem timestamp rounding, while
    # an independent clock catches an incorrect native epoch or unit.
    before_ns = time.time_ns() // 1_000_000_000 * 1_000_000_000
    path.write_bytes(b"before")
    metadata = path.stat()
    with path.open("rb") as handle:
        path_change = file_hashing.content_change_time_ns(path, metadata)
        handle_change = file_hashing.content_change_time_ns_from_fd(
            handle.fileno(), os.fstat(handle.fileno())
        )
    after_ns = time.time_ns()
    assert path_change is not None
    assert before_ns <= path_change <= after_ns
    assert handle_change == path_change


def test_content_change_time_has_one_cross_module_authority() -> None:
    assert callable(file_hashing.content_change_time_ns)
    assert not hasattr(file_hashing, "_content_change_time_ns")


def test_windows_change_time_fails_closed_when_api_is_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "content.bin"
    path.write_bytes(b"content")
    monkeypatch.setattr(file_hashing, "_windows_file_api", lambda: None)

    assert file_hashing._windows_change_time_ns(path) is None


def _git(root: Path, *arguments: str) -> bytes:
    from tests.process_guard_common import run_guarded_test_process

    result = run_guarded_test_process(
        ["git", "-C", str(root), *arguments],
        text=False,
        check=True,
    )
    return result.stdout


def test_git_admission_is_shared_by_every_identity_walk(tmp_path, monkeypatch):
    import hashlib

    from molt import input_report
    from molt.cli import backend_binary, build_inputs, cache_fingerprints
    from molt.cli.compiler_identity import CompilerSourceGeneration
    from molt.cli.link_fingerprints import _link_fingerprint
    from molt.cli.runtime_build_identity import (
        RuntimeTreeIndex,
        verify_runtime_source_trees,
    )
    from molt.source_admission import admitted_source_files
    from tests.compiler_identity_helper import (
        compiler_build_admission,
        write_compiler_lock,
    )

    root = tmp_path.resolve()
    _git(root, "init", "-q")
    (root / ".gitignore").write_text(
        ".molt-artifact-publication.lock\n.molt-artifact-stage-*\n.DS_Store\nignored.rs\n",
        encoding="utf-8",
    )
    directories = [
        root / "runtime/core",
        root / "src/molt/frontend",
        root / "src/molt/compiler_analysis",
    ]
    for directory in directories:
        directory.mkdir(parents=True)
        (directory / "source.rs").write_bytes(b"pub fn source() {}\n")
    write_compiler_lock(root)
    _git(root, "add", ".")
    monkeypatch.setattr(
        backend_binary, "_backend_source_paths", lambda *_args, **_kw: directories
    )
    monkeypatch.setattr(
        backend_binary,
        "_backend_source_identity_inputs",
        lambda _root, paths, **_kw: (paths, "a" * 64),
    )
    from molt.cli import runtime_build_identity

    monkeypatch.setattr(
        runtime_build_identity,
        "runtime_source_paths",
        lambda *_args: tuple(directories),
    )
    roots = [
        (f"source/{directory.relative_to(root).as_posix()}", directory)
        for directory in directories
    ]
    admission = compiler_build_admission(environment={"RUSTFLAGS": ""})

    def keys():
        runtime = RuntimeTreeIndex.capture(roots).identity(roots, require_all=True)
        compiler = backend_binary._backend_fingerprint(
            root,
            cargo_profile="dev-fast",
            build_admission=admission,
            backend_features=(),
        )
        frontend = cache_fingerprints._frontend_semantic_tooling_sources(root)
        signature = cache_fingerprints._source_tree_content_signature(
            root, tuple(map(str, frontend.paths)), frontend.source_sha256
        )
        frontend_key = cache_fingerprints._compute_source_tree_content_digest(
            signature, "frontend-semantic-tooling", ""
        )
        digest = hashlib.sha256()
        file_hashing._hash_source_tree_paths(directories, root, digest)
        return (
            runtime,
            compiler,
            frontend_key,
            digest.hexdigest(),
            file_hashing._hash_source_tree_metadata(directories, root),
            _link_fingerprint(project_root=root, inputs=directories, link_cmd=["link"]),
            build_inputs._latest_mtime(directories),
        )

    before = keys()
    generation = CompilerSourceGeneration.capture(directories)
    for directory in directories:
        for name in (
            ".molt-artifact-publication.lock",
            ".molt-artifact-stage-crash",
            ".DS_Store",
            "ignored.rs",
        ):
            (directory / name).write_bytes(b"debris")
        (directory / "empty").mkdir()
    assert keys() == before
    generation.verify()
    verify_runtime_source_trees(root, [((), before[0])])
    # The input report and every directory label are also derived from source.
    assert set(admitted_source_files(directories)) == {
        directory / "source.rs" for directory in directories
    }
    reported = []
    with monkeypatch.context() as patch:
        patch.setattr(input_report, "active", lambda: True)
        patch.setattr(
            input_report, "report_file", lambda path, *args: reported.append(path)
        )
        patch.setattr(input_report, "report_directory", lambda path: None)
        backend_binary._backend_fingerprint(
            root,
            cargo_profile="dev-fast",
            build_admission=admission,
            backend_features=(),
        )
    assert set(reported) == {
        root / "Cargo.lock",
        *(directory / "source.rs" for directory in directories),
    }
    for directory in directories:
        (directory / "new.rs").write_bytes(b"pub fn added() {}\n")
    after = keys()
    assert all(left != right for left, right in zip(before, after, strict=True))
    with pytest.raises(ValueError, match="membership/generation"):
        generation.verify()


def test_git_inventory_refreshes_untracked_and_ignore_edits_without_index_change(
    tmp_path,
):
    from molt.source_admission import admitted_source_files

    root = tmp_path.resolve()
    _git(root, "init", "-q")
    source = root / "source"
    source.mkdir()
    ignore = root / ".gitignore"
    ignore.write_text("hidden.rs\n", encoding="utf-8")
    first = source / "first.rs"
    first.write_bytes(b"first")
    _git(root, "add", ".")
    index = (root / ".git/index").stat()
    assert admitted_source_files((source,)) == (first,)
    empty = source / "empty"
    empty.mkdir()
    assert admitted_source_files((source,)) == (first,)
    new = empty / "new.rs"
    new.write_bytes(b"new")
    hidden = source / "hidden.rs"
    hidden.write_bytes(b"hidden")
    assert set(admitted_source_files((source,))) == {first, new}
    ignore.write_text("", encoding="utf-8")
    assert set(admitted_source_files((source,))) == {first, new, hidden}
    after = (root / ".git/index").stat()
    assert (after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) == (
        index.st_ino,
        index.st_size,
        index.st_mtime_ns,
        index.st_ctime_ns,
    )
    # Explicitly declared file inputs may be ignored; directory declarations
    # cannot silently admit them.
    ignore.write_text("hidden.rs\n", encoding="utf-8")
    assert hidden in admitted_source_files((source, hidden))


@pytest.mark.parametrize("git_checkout", [False, True])
def test_python_producer_admission_preserves_file_link_labels_and_target_bytes(
    tmp_path: Path, git_checkout: bool
) -> None:
    import hashlib

    from molt.cli import cache_fingerprints
    from molt.source_admission import admitted_source_files

    root = tmp_path.resolve()
    source = root / "src/molt/frontend"
    source.mkdir(parents=True)
    target = root / "target.py"
    target.write_bytes(b"VALUE = 1\n")
    alias = source / "producer.py"
    try:
        alias.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"file symlinks unavailable: {exc}")
    if git_checkout:
        _git(root, "init", "-q")
        _git(root, "add", "src/molt/frontend/producer.py")

    for roots in ((alias,), (source,)):
        with pytest.raises(ValueError, match="alias"):
            admitted_source_files(roots)
        files = admitted_source_files(roots, follow_file_symlinks=True)
        assert files == (alias,)
    before = cache_fingerprints._frontend_semantic_tooling_sources(root)
    assert before.paths == (alias,)
    assert dict(before.source_sha256) == {
        alias: hashlib.sha256(b"VALUE = 1\n").hexdigest()
    }

    target.write_bytes(b"VALUE = 2\n")
    after = cache_fingerprints._frontend_semantic_tooling_sources(root)
    assert after.paths == (alias,)
    assert dict(after.source_sha256) == {
        alias: hashlib.sha256(b"VALUE = 2\n").hexdigest()
    }


@pytest.mark.parametrize(
    "relative", ["source.rs", "nested/a b.rs", "../sibling/file.rs", "."]
)
def test_source_fingerprint_labels_preserve_component_boundaries(tmp_path, relative):
    root = tmp_path / "source"
    path = root / relative
    try:
        expected = str(path.relative_to(root))
    except ValueError:
        expected = str(path)
    assert file_hashing._source_fingerprint_label(path, root) == expected


def test_git_inventory_batches_all_roots_once_and_preserves_tracked_dot_files(
    tmp_path, monkeypatch
):
    from molt import source_admission

    root = tmp_path.resolve()
    _git(root, "init", "-q")
    directories = (root / "first", root / "second")
    for directory in directories:
        directory.mkdir()
        (directory / ".tracked.rs").write_bytes(b"admitted")
    _git(root, "add", ".")
    calls = []
    original = source_admission._read_git

    def read_git(cwd, arguments):
        calls.append(arguments)
        return original(cwd, arguments)

    monkeypatch.setattr(source_admission, "_read_git", read_git)
    assert set(source_admission.admitted_source_files(directories)) == {
        directory / ".tracked.rs" for directory in directories
    }
    assert len(calls) == 1 and calls[0][0] == "ls-files"
