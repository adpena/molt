from __future__ import annotations

from contextlib import contextmanager
from dataclasses import FrozenInstanceError
import hashlib
import os
from pathlib import Path
import stat
from types import SimpleNamespace

import pytest

from tools import verified_subset
from tools.compat import test_policy


ROOT = Path(__file__).resolve().parents[1]


def _source(tmp_path: Path, metadata: str, *, name: str = "case.py") -> Path:
    path = tmp_path / name
    path.write_text(f"# MOLT_META: {metadata}\nprint('ok')\n", encoding="utf-8")
    return path


def test_metadata_is_one_frozen_typed_value(tmp_path: Path) -> None:
    metadata = test_policy.parse_metadata(
        _source(
            tmp_path,
            "verified_subset_scope=capability_policy "
            "expect_fail=molt expect_fail_reason=requires_ffi "
            "min_py=3.15 max_py=3.100 platforms=posix "
            "architectures=aarch64,x86_64 backends=llvm,luau,native "
            "stdout=pyperformance stderr=exception_signature stdlib_profile=full",
        )
    )

    assert metadata == test_policy.TestMetadata(
        verification_scope="capability_policy",
        expect_molt_fail=True,
        expected_failure_reason="requires_ffi",
        min_python=(3, 15),
        max_python=(3, 100),
        platforms=frozenset({"posix"}),
        architectures=frozenset({"aarch64", "x86_64"}),
        backends=frozenset({"llvm", "luau", "native"}),
        stdout_mode="pyperformance",
        stderr_mode="exception_signature",
        stdlib_profile="full",
    )
    with pytest.raises(AttributeError):
        metadata.stdout_mode = "exact"  # type: ignore[misc]


@pytest.mark.parametrize("version", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "filename, minimum",
    [
        ("class_annotation_namespace.py", (3, 12)),
        ("class_annotation_namespace_313.py", (3, 13)),
    ],
)
def test_static_class_annotation_corpora_have_exact_version_admission(
    filename: str, minimum: tuple[int, int], version: tuple[int, int]
) -> None:
    metadata = test_policy.parse_metadata(
        ROOT / "tests" / "differential" / "basic" / filename
    )
    assert metadata.min_python == minimum
    reason = metadata.python_exclusion_reason(version)
    assert reason == (
        f"min_py {minimum[0]}.{minimum[1]}" if version < minimum else None
    )
    assert not metadata.expect_molt_fail


def test_version_projection_ignores_unselected_backend_coordinate() -> None:
    metadata = test_policy.TestMetadata(
        min_python=(3, 13), max_python=(3, 14), backends=frozenset({"wasm"})
    )
    assert metadata.python_exclusion_reason((3, 12)) == "min_py 3.13"
    assert metadata.python_exclusion_reason((3, 13)) is None
    assert metadata.python_exclusion_reason((3, 15)) == "max_py 3.14"


@pytest.mark.parametrize(
    "metadata, message",
    [
        ("unknown=value", "unknown MOLT_META key"),
        ("bare", "malformed MOLT_META token"),
        ("min_py=", "empty MOLT_META value"),
        ("platforms=linux,,macos", "empty MOLT_META value"),
        ("min_py=3.12 min_py=3.13", "duplicate MOLT_META key"),
        ("min_py=3.12,3.13", "must select exactly one value"),
        ("backends=native,native", "duplicate MOLT_META value"),
        ("backends=native,llvm", "values must be sorted and unique"),
        ("expect_fail=molt", "must be declared together"),
        ("expect_fail_reason=compiler_gap", "must be declared together"),
        ("expect_fail=python expect_fail_reason=compiler_gap", "exactly 'molt'"),
        ("expect_fail=molt expect_fail_reason=Bad-Reason", "lowercase identifier"),
        ("min_py=3.14 max_py=3.13", "must not exceed"),
        ("platforms=unix", "unknown values"),
        ("platforms=linux,posix", "platforms=posix must not duplicate"),
        ("architectures=amd64", "unknown values"),
        ("backends=python", "unknown values"),
        ("normalize=paths", "unknown MOLT_META key"),
        ("stdout=approximately", "unknown values"),
        ("stdout=relaxed", "unknown values"),
        ("stderr=traceback", "unknown values"),
        ("stdlib_profile=wide", "unknown values"),
        ("stdlib_profile=micro", "unknown values"),
        ("pep=312", "unknown MOLT_META key"),
        ("stdlib=urllib.request", "unknown MOLT_META key"),
    ],
)
def test_metadata_rejects_malformed_or_ambiguous_rows(
    tmp_path: Path, metadata: str, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        test_policy.parse_metadata(_source(tmp_path, metadata))


@pytest.mark.parametrize(
    "legacy",
    [
        "wasm=no",
        "xfail=molt",
        "xfail_reason=gap",
        "platform=windows",
        "architecture=x86_64",
        "arch=x86_64",
        "backend=native",
        "py=3.12",
        "python=3.12",
        "skip=true",
    ],
)
def test_metadata_rejects_deleted_legacy_keys(tmp_path: Path, legacy: str) -> None:
    with pytest.raises(ValueError, match="unknown MOLT_META key"):
        test_policy.parse_metadata(_source(tmp_path, legacy))


@pytest.mark.parametrize(
    "version",
    ["3.11", "3", "3.12.1", "3.12rc1", "03.12", "3.012", "3.１２"],
)
def test_metadata_rejects_noncanonical_python_minor(
    tmp_path: Path, version: str
) -> None:
    with pytest.raises(ValueError, match="exact 3.<minor>"):
        test_policy.parse_metadata(_source(tmp_path, f"min_py={version}"))


def test_metadata_uses_python_comment_tokens_not_string_contents(
    tmp_path: Path,
) -> None:
    path = tmp_path / "string_marker.py"
    path.write_text(
        'TEXT = """\n# MOLT_META: wasm=no\n"""\n# MOLT_META: backends=wasm\n',
        encoding="utf-8",
    )
    assert test_policy.parse_metadata(path).backends == frozenset({"wasm"})


def test_metadata_rejects_malformed_or_multiple_comments(tmp_path: Path) -> None:
    malformed = tmp_path / "malformed.py"
    malformed.write_text("# MOLT_META backends=wasm\n", encoding="utf-8")
    with pytest.raises(ValueError, match="malformed MOLT_META comment"):
        test_policy.parse_metadata(malformed)

    repeated = tmp_path / "repeated.py"
    repeated.write_text(
        "# MOLT_META: backends=wasm\n# MOLT_META: platforms=windows\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="multiple MOLT_META declarations"):
        test_policy.parse_metadata(repeated)


def test_metadata_fails_closed_on_missing_or_invalid_source(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="cannot read differential metadata source"):
        test_policy.parse_metadata(tmp_path / "missing.py")

    invalid = tmp_path / "invalid.py"
    invalid.write_bytes(b"# coding: utf-8\n# MOLT_META: backends=wasm\n\xff")
    with pytest.raises(ValueError, match="cannot decode differential metadata source"):
        test_policy.parse_metadata(invalid)


def test_backend_selector_replaces_wasm_gate_without_semantic_loss(
    tmp_path: Path,
) -> None:
    no_wasm = test_policy.parse_metadata(
        _source(tmp_path, "backends=llvm,luau,native", name="no_wasm.py")
    )
    only_wasm = test_policy.parse_metadata(
        _source(tmp_path, "backends=wasm", name="only_wasm.py")
    )
    tags = test_policy.coordinate_platform_tags(platform="linux")

    for backend in test_policy.ALL_BACKENDS:
        no_wasm_reason = no_wasm.exclusion_reason(
            python_version=(3, 12),
            platform_tags=tags,
            architecture="x86_64",
            backend=backend,
        )
        only_wasm_reason = only_wasm.exclusion_reason(
            python_version=(3, 12),
            platform_tags=tags,
            architecture="x86_64",
            backend=backend,
        )
        assert (no_wasm_reason is None) is (backend != "wasm")
        assert (only_wasm_reason is None) is (backend == "wasm")


def test_every_verified_physical_source_has_valid_typed_metadata() -> None:
    inventory = verified_subset.validate_manifest().inventory
    files = inventory.files
    parsed = tuple(source.metadata for source in inventory.sources)

    assert len(parsed) == len(files)
    assert all(isinstance(metadata, test_policy.TestMetadata) for metadata in parsed)
    assert not any("wasm" in metadata.as_record() for metadata in parsed)


def _inventory_fixture(tmp_path: Path):
    root = tmp_path / "repo"
    suite = root / "suite"
    suite.mkdir(parents=True)
    source = _source(suite, "backends=native")
    return root, suite, source


def test_inventory_is_frozen_and_retains_actual_suite_membership(
    tmp_path: Path,
) -> None:
    root, first, source = _inventory_fixture(tmp_path)
    nested = first / "nested"
    nested.mkdir()
    other = _source(nested, "backends=wasm")
    selectors = (("suite", False), ("suite/nested", True))
    inventory = test_policy.load_test_inventory(selectors, repo_root=root)
    assert inventory.repo_root == root
    assert inventory.suites == selectors
    assert inventory.files == (source, other)
    assert inventory.suite_members == (("suite/case.py",), ("suite/nested/case.py",))
    assert tuple(item.path for item in inventory.sources) == (
        "suite/case.py",
        "suite/nested/case.py",
    )
    inventory.verify_unchanged()
    with pytest.raises(FrozenInstanceError):
        inventory.files = ()


def test_inventory_reads_each_source_once_and_enumerates_each_suite_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, suite, first = _inventory_fixture(tmp_path)
    second = _source(suite, "backends=wasm", name="second.py")
    capture = test_policy.capture_stable_regular_file
    scandir = os.scandir
    read_paths = []
    listed_paths = []

    def counted_capture(path, **kwargs):
        read_paths.append(path)
        return capture(path, **kwargs)

    def counted_scandir(path):
        listed_paths.append(Path(path))
        return scandir(path)

    monkeypatch.setattr(test_policy, "capture_stable_regular_file", counted_capture)
    monkeypatch.setattr(test_policy.os, "scandir", counted_scandir)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    inventory.verify_unchanged()
    assert read_paths == [first, second]
    assert listed_paths == [suite]


@pytest.mark.parametrize("consumer", ["sources", "metadata"])
def test_metadata_and_digest_use_captured_bytes_not_a_second_path_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, consumer: str
) -> None:
    source = _source(tmp_path, "backends=native")
    original = source.read_bytes()
    capture = test_policy.capture_stable_regular_file
    reads = []

    def capture_then_change(path, **kwargs):
        result = capture(path, **kwargs)
        assert result[0].sha256 == hashlib.sha256(original).hexdigest()
        reads.append(path)
        source.write_text("# MOLT_META: backends=wasm\nprint('new')\n")
        return result

    monkeypatch.setattr(test_policy, "capture_stable_regular_file", capture_then_change)
    if consumer == "sources":
        with pytest.raises(ValueError, match="changed"):
            test_policy.load_test_sources((source,), repo_root=tmp_path)
    else:
        metadata = test_policy.parse_metadata(source)
        assert metadata.backends == frozenset({"native"})
    assert reads == [source]


@pytest.mark.parametrize(
    "mutation", ["add", "delete", "content", "metadata", "restored-mtime"]
)
def test_inventory_rejects_changes_and_fresh_capture_observes_them(
    tmp_path: Path, mutation: str
) -> None:
    root, suite, source = _inventory_fixture(tmp_path)
    selectors = (("suite", False),)
    inventory = test_policy.load_test_inventory(selectors, repo_root=root)
    before = source.stat()
    if mutation == "add":
        _source(suite, "backends=wasm", name="added.py")
    elif mutation == "delete":
        source.unlink()
    elif mutation == "metadata":
        source.write_text("# MOLT_META: backends=wasm\nprint('ok')\n")
    else:
        raw = source.read_bytes()
        source.write_bytes(raw.replace(b"'ok'", b"'no'"))
        if mutation == "restored-mtime":
            os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="changed|unavailable"):
        inventory.verify_unchanged()
    fresh = test_policy.load_test_inventory(selectors, repo_root=root)
    assert fresh.sources != inventory.sources
    if mutation == "metadata":
        assert fresh.sources[0].metadata.backends == frozenset({"wasm"})
    fresh.verify_unchanged()


def test_inventory_detects_addition_even_if_directory_mtime_is_restored(
    tmp_path: Path,
) -> None:
    root, suite, _source_path = _inventory_fixture(tmp_path)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    before = suite.stat()
    _source(suite, "backends=wasm", name="added.py")
    os.utime(suite, ns=(before.st_atime_ns, before.st_mtime_ns))
    with pytest.raises(ValueError, match="directory changed"):
        inventory.verify_unchanged()


@pytest.mark.parametrize("mutation", ["source", "directory", "ancestor"])
def test_inventory_rejects_replacement_with_identical_source_bytes(
    tmp_path: Path, mutation: str
) -> None:
    root, suite, source = _inventory_fixture(tmp_path)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    raw = source.read_bytes()
    selected = {"source": source, "directory": suite, "ancestor": root}[mutation]
    selected.rename(tmp_path / "retired")
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_bytes(raw)
    with pytest.raises(ValueError, match="changed"):
        inventory.verify_unchanged()


def test_inventory_does_not_bind_unrelated_ancestor_membership(tmp_path: Path) -> None:
    root, _suite, _source_path = _inventory_fixture(tmp_path)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    (root / "unrelated-output").mkdir()
    (tmp_path / "another-repository").mkdir()
    inventory.verify_unchanged()


@pytest.mark.parametrize("mutation", ["source", "addition", "ancestor"])
def test_inventory_capture_fences_mutation_during_source_loading(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    root, suite, source = _inventory_fixture(tmp_path)
    capture = test_policy.capture_stable_regular_file

    def capture_then_change(path, **kwargs):
        result = capture(path, **kwargs)
        if mutation == "source":
            source.write_text("# MOLT_META: backends=wasm\nprint('new')\n")
        elif mutation == "addition":
            _source(suite, "backends=wasm", name="added.py")
        else:
            root.rename(tmp_path / "retired")
            source.parent.mkdir(parents=True)
            source.write_bytes(result[1])
        return result

    monkeypatch.setattr(test_policy, "capture_stable_regular_file", capture_then_change)
    with pytest.raises(ValueError, match="changed"):
        test_policy.load_test_inventory((("suite", False),), repo_root=root)


def test_inventory_uses_physical_membership_and_rejects_overlapping_suites(
    tmp_path: Path,
) -> None:
    root, suite, source = _inventory_fixture(tmp_path)
    (suite / "TESTS.txt").write_text("# deliberately empty scheduling projection\n")
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    assert inventory.files == (source,)
    with pytest.raises(ValueError, match="selected by multiple suites"):
        test_policy.load_test_inventory(
            (("suite", True), ("suite", False)), repo_root=root
        )


def test_inventory_fails_closed_when_directory_change_time_is_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _suite, _source_path = _inventory_fixture(tmp_path)
    monkeypatch.setattr(test_policy, "content_change_time_ns", lambda *_args: None)
    with pytest.raises(ValueError, match="directory change time is unavailable"):
        test_policy.load_test_inventory((("suite", False),), repo_root=root)


def test_inventory_ignores_directory_allocation_and_access_time_noise(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, suite, _source_path = _inventory_fixture(tmp_path)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    lstat = Path.lstat

    def noisy_stat(path):
        current = lstat(path)
        if path != suite:
            return current
        return SimpleNamespace(
            st_mode=current.st_mode,
            st_dev=current.st_dev,
            st_ino=current.st_ino,
            st_mtime_ns=current.st_mtime_ns,
            st_ctime_ns=current.st_ctime_ns,
            st_file_attributes=getattr(current, "st_file_attributes", 0),
            st_size=current.st_size + 4096,
            st_atime_ns=current.st_atime_ns + 1,
        )

    monkeypatch.setattr(Path, "lstat", noisy_stat)
    inventory.verify_unchanged()


def test_inventory_rejects_ancestor_link_indirection_after_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _suite, _source_path = _inventory_fixture(tmp_path)
    inventory = test_policy.load_test_inventory((("suite", False),), repo_root=root)
    lstat = Path.lstat

    def linked_stat(path):
        current = lstat(path)
        if path == root:
            return SimpleNamespace(st_mode=stat.S_IFLNK | 0o777)
        return current

    monkeypatch.setattr(Path, "lstat", linked_stat)
    with pytest.raises(ValueError, match="directory is linked or invalid"):
        inventory.verify_unchanged()


def test_physical_collection_reuses_one_nofollow_entry_stat_without_leaf_queries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _suite, source = _inventory_fixture(tmp_path)
    scandir = os.scandir
    lstat = Path.lstat
    file_stat = Path.stat
    observed = []

    class Entry:
        def __init__(self, entry):
            self.entry = entry
            self.path, self.name = entry.path, entry.name

        def stat(self, *, follow_symlinks):
            assert follow_symlinks is False
            observed.append(Path(self.path))
            return self.entry.stat(follow_symlinks=False)

    @contextmanager
    def cached_entries(directory):
        with scandir(directory) as entries:
            yield [Entry(entry) for entry in entries]

    def no_leaf_lstat(path):
        assert path != source, "collector repeated a leaf lstat"
        return lstat(path)

    def no_leaf_stat(path, **kwargs):
        assert path != source, "collector followed a leaf with stat"
        return file_stat(path, **kwargs)

    monkeypatch.setattr(test_policy.os, "scandir", cached_entries)
    monkeypatch.setattr(Path, "lstat", no_leaf_lstat)
    monkeypatch.setattr(Path, "stat", no_leaf_stat)
    assert test_policy.collect_physical_test_files(
        (("suite", False),), repo_root=root
    ) == (source,)
    assert observed == [source]


@pytest.mark.parametrize("kind", ["symlink", "reparse", "special"])
def test_physical_collection_rejects_unsafe_nofollow_entry_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    from molt import file_publication

    root, _suite, source = _inventory_fixture(tmp_path)
    scandir = os.scandir
    monkeypatch.setattr(file_publication, "_WINDOWS_REPARSE_POINT", 0x400)

    @contextmanager
    def unsafe_entries(directory):
        with scandir(directory) as entries:
            captured = list(entries)
        assert len(captured) == 1 and Path(captured[0].path) == source
        metadata = SimpleNamespace(
            st_mode={
                "symlink": stat.S_IFLNK,
                "reparse": stat.S_IFREG,
                "special": stat.S_IFIFO,
            }[kind],
            st_file_attributes=0x400 if kind == "reparse" else 0,
        )
        yield [
            SimpleNamespace(
                path=str(source), name=source.name, stat=lambda **_kwargs: metadata
            )
        ]

    monkeypatch.setattr(test_policy.os, "scandir", unsafe_entries)
    with pytest.raises(ValueError, match="link or reparse point|special entry"):
        test_policy.collect_physical_test_files((("suite", False),), repo_root=root)


@pytest.mark.parametrize("consumer", ["sources", "projection"])
def test_explicit_sources_share_portable_collision_admission(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, consumer: str
) -> None:
    first = _source(tmp_path, "backends=native")
    second = _source(tmp_path, "backends=wasm", name="second.py")
    monkeypatch.setattr(
        test_policy, "portable_path_identity", lambda _path: "collision"
    )
    with pytest.raises(ValueError, match="collide on portable filesystems"):
        if consumer == "sources":
            test_policy.load_test_sources((first, second), repo_root=tmp_path)
        else:
            test_policy.project_coordinate(
                (first, second),
                repo_root=tmp_path,
                python="3.12",
                platform="linux",
                arch="x86_64",
                backend="native",
            )


def test_explicit_sources_capture_metadata_and_digest_from_same_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = _source(tmp_path, "backends=native")
    raw = source.read_bytes()
    parse = test_policy._parse_metadata_bytes
    parsed = []

    def observe_bytes(path, data):
        parsed.append(data)
        return parse(path, data)

    monkeypatch.setattr(test_policy, "_parse_metadata_bytes", observe_bytes)
    row = test_policy.load_test_sources((source,), repo_root=tmp_path)[0]
    assert parsed == [raw]
    assert row.source_sha256 == hashlib.sha256(raw).hexdigest()
    assert row.metadata.backends == frozenset({"native"})


def test_explicit_source_projection_fences_ancestor_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _suite, source = _inventory_fixture(tmp_path)
    capture = test_policy.capture_stable_regular_file

    def capture_then_replace(path, **kwargs):
        result = capture(path, **kwargs)
        root.rename(tmp_path / "retired")
        source.parent.mkdir(parents=True)
        source.write_bytes(result[1])
        return result

    monkeypatch.setattr(
        test_policy, "capture_stable_regular_file", capture_then_replace
    )
    with pytest.raises(ValueError, match="directory changed"):
        test_policy.project_coordinate(
            (source,),
            repo_root=root,
            python="3.12",
            platform="linux",
            arch="x86_64",
            backend="native",
        )
