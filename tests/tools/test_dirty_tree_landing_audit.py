from __future__ import annotations

import importlib.util
import json
import os
import subprocess
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
TOOL = REPO_ROOT / "tools" / "dirty_tree_landing_audit.py"


def _load_tool():
    spec = importlib.util.spec_from_file_location("dirty_tree_landing_audit", TOOL)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _git(root: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(root),
        text=True,
        capture_output=True,
        check=True,
        encoding="utf-8",
    )
    return proc.stdout


def _init_repo(root: Path) -> None:
    root.mkdir()
    _git(root, "init")
    _git(root, "config", "user.email", "test@example.invalid")
    _git(root, "config", "user.name", "Test User")


def _commit_all(root: Path, message: str) -> str:
    _git(root, "add", "-A")
    _git(root, "commit", "-m", message)
    return _git(root, "rev-parse", "HEAD").strip()


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_build_report_fails_when_dirty_source_path_is_missing() -> None:
    tool = _load_tool()

    report = tool.build_report(
        source_root=Path("source"),
        landed_root=Path("landed"),
        base_ref="base",
        head_ref="HEAD",
        source_paths=["runtime/a.rs", "runtime/missed.rs", "docs/note.md"],
        landed_paths=["runtime/a.rs", "runtime/extra.rs"],
        owned=["runtime/"],
    )

    assert not report.ok
    assert report.source_only == ("runtime/missed.rs",)
    assert report.landed_only == ("runtime/extra.rs",)
    assert "dirty landing audit: FAIL" in tool.render_text(report)


def test_ignored_paths_do_not_trip_coverage() -> None:
    tool = _load_tool()

    report = tool.build_report(
        source_root=Path("source"),
        landed_root=Path("landed"),
        base_ref="base",
        head_ref="HEAD",
        source_paths=["src/owned.py", "tmp/local.txt"],
        landed_paths=["src/owned.py"],
        ignored=["tmp/"],
    )

    assert report.ok
    assert report.source_dirty_paths == ("src/owned.py",)


def test_fail_on_landed_only_catches_extra_range_paths() -> None:
    tool = _load_tool()

    report = tool.build_report(
        source_root=Path("source"),
        landed_root=Path("landed"),
        base_ref="base",
        head_ref="HEAD",
        source_paths=["src/owned.py"],
        landed_paths=["src/owned.py", "src/extra.py"],
        fail_on_landed_only=True,
    )

    assert not report.ok
    assert report.source_only == ()
    assert report.landed_only == ("src/extra.py",)


def test_main_audits_real_git_dirty_source_against_landed_range(
    tmp_path: Path,
    capsys,
) -> None:
    tool = _load_tool()
    source = tmp_path / "source"
    landed = tmp_path / "landed"
    _init_repo(source)
    _init_repo(landed)

    for root in (source, landed):
        _write(root / "src" / "changed.py", "old\n")
        _write(root / "src" / "deleted.py", "delete me\n")
        _write(root / "docs" / "unrelated.md", "local\n")
        _commit_all(root, "base")

    base = _git(landed, "rev-parse", "HEAD").strip()

    _write(source / "src" / "changed.py", "dirty source\n")
    (source / "src" / "deleted.py").unlink()
    _write(source / "src" / "new.py", "new dirty file\n")
    _write(source / "docs" / "unrelated.md", "unrelated dirty work\n")

    _write(landed / "src" / "changed.py", "landed conflict resolution\n")
    (landed / "src" / "deleted.py").unlink()
    _write(landed / "src" / "new.py", "landed new file\n")
    _commit_all(landed, "land dirty source subset")

    rc = tool.main(
        [
            "--source-root",
            str(source),
            "--landed-root",
            str(landed),
            "--base-ref",
            base,
            "--head-ref",
            "HEAD",
            "--owned",
            "src/",
        ]
    )

    assert rc == 0
    out = capsys.readouterr().out
    assert "dirty landing audit: PASS" in out
    assert "source-only paths: 0" in out

    rc = tool.main(
        [
            "--source-root",
            str(source),
            "--landed-root",
            str(landed),
            "--base-ref",
            base,
            "--head-ref",
            "HEAD",
        ]
    )

    assert rc == 1
    out = capsys.readouterr().out
    assert "docs/unrelated.md" in out


def test_real_git_preserves_index_worktree_and_untracked_path_families(tmp_path):
    tool = _load_tool()
    source = tmp_path / "source"
    _init_repo(source)
    initial = {"unstaged.py", "staged.py", "cancelled.py", "deleted.py", "old café.py"}
    for name in initial:
        _write(source / name, "original\n")
    _commit_all(source, "base")

    _write(source / "unstaged.py", "working tree only\n")
    _write(source / "staged.py", "index only\n")
    _write(source / "cancelled.py", "retained index generation\n")
    _write(source / "new staged.py", "index addition\n")
    _git(source, "add", "staged.py", "cancelled.py", "new staged.py")
    _write(source / "cancelled.py", "original\n")
    _git(source, "rm", "deleted.py")
    _git(source, "mv", "old café.py", "intermediate café.py")
    (source / "intermediate café.py").rename(source / "final café.py")
    _write(source / "untracked café.py", "untracked\n")

    # The working copy equals HEAD here; a net HEAD-to-worktree diff would
    # erase the distinct index generation, which remains owned source work.
    assert (source / "cancelled.py").read_text(encoding="utf-8") == "original\n"
    assert _git(source, "show", ":cancelled.py") == "retained index generation\n"
    assert _git(source, "show", ":new staged.py") == "index addition\n"
    assert set(tool.dirty_source_paths(source)) == initial | {
        "new staged.py",
        "intermediate café.py",
        "final café.py",
        "untracked café.py",
    }


def test_index_only_omission_fails_actual_audit_until_path_is_landed(tmp_path):
    tool = _load_tool()
    source, landed = tmp_path / "source", tmp_path / "landed"
    name = "src/staged café.py"
    for root in (source, landed):
        _init_repo(root)
        _write(root / name, "original\n")
        _commit_all(root, "base")
    base = _git(landed, "rev-parse", "HEAD").strip()
    _write(source / name, "only in index\n")
    _git(source, "add", name)
    report = tool.audit_dirty_landing(
        source_root=source, landed_root=landed, base_ref=base, head_ref="HEAD"
    )
    assert not report.ok
    assert report.source_only == (name,)

    # Content reconciliation is deliberately different; the audit owns path
    # coverage rather than falsely claiming source-byte equivalence.
    _write(landed / name, "reviewed conflict resolution\n")
    _commit_all(landed, "land the missing path")
    report = tool.audit_dirty_landing(
        source_root=source, landed_root=landed, base_ref=base, head_ref="HEAD"
    )
    assert report.ok
    assert report.source_dirty_paths == report.landed_range_paths == (name,)


@pytest.mark.parametrize(
    ("platform", "selected", "other"),
    [
        ("posix", r"src\owned.py", "src/owned.py"),
        ("nt", "src/owned.py", r"src\owned.py"),
    ],
)
def test_filters_use_native_patterns_without_rewriting_paths(
    monkeypatch, platform, selected, other
):
    tool = _load_tool()
    # Model the platform at the pattern boundary without changing the host's
    # filesystem or the canonical Git path identities supplied to the audit.
    monkeypatch.setattr(tool, "os", SimpleNamespace(name=platform))
    paths = (r"src\owned.py", "src/owned.py")
    assert tool.filter_paths(paths) == tuple(sorted(paths))
    assert tool.filter_paths(paths, owned=[r"src\owned.py"]) == (selected,)
    assert tool.filter_paths(paths, ignored=[r"src\owned.py"]) == (other,)
    report = tool.build_report(
        source_root=Path("source"),
        landed_root=Path("landed"),
        base_ref="base",
        head_ref="HEAD",
        source_paths=paths,
        landed_paths=[other],
        owned=[r"src\owned.py"],
    )
    assert not report.ok
    assert report.owned == (selected,)
    assert report.source_only == (selected,)
    assert report.landed_range_paths == ()


@pytest.mark.skipif(
    os.name == "nt", reason="Windows filenames exclude these POSIX characters"
)
def test_real_git_and_reports_preserve_literal_path_identities(tmp_path, capsys):
    tool = _load_tool()
    source = tmp_path / "source"
    _init_repo(source)
    names = (
        "src/tab\tname.py",
        "src/line\nname.py",
        "src/line\rname.py",
        r"src/literal\name.py",
        "src/literal/name.py",
    )
    for name in names:
        _write(source / name, "original\n")
    base = _commit_all(source, "base")
    for name in names:
        _write(source / name, "index only\n")
    _git(source, "add", "-A")
    expected = tuple(sorted(names))
    assert tool.dirty_source_paths(source) == expected

    # An empty landed range cannot cover any of these staged-only names. In
    # particular, CR/LF and backslash/slash pairs are distinct identities.
    args = [
        "--source-root",
        str(source),
        "--landed-root",
        str(source),
        "--base-ref",
        base,
        "--head-ref",
        base,
    ]
    report_path = tmp_path / "report.json"
    assert tool.main([*args, "--json", "--json-output", str(report_path)]) == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload == json.loads(report_path.read_text(encoding="utf-8"))
    assert payload["source_dirty_paths"] == payload["source_only"] == list(expected)
    assert payload["counts"]["source_only"] == len(names)

    assert tool.main(args) == 1
    text = capsys.readouterr().out
    path_lines = text.split("source-only:\n", 1)[1].splitlines()
    assert len(path_lines) == len(names)
    assert tuple(json.loads(line.strip()) for line in path_lines) == expected

    head = _commit_all(source, "land each exact path")
    assert tool.landed_range_paths(source, base, head) == expected


@pytest.mark.skipif(
    os.name == "nt", reason="Windows filenames exclude these POSIX characters"
)
@pytest.mark.parametrize(
    ("missing", "different"),
    [
        ("src/line\rname.py", "src/line\nname.py"),
        (r"src/literal\name.py", "src/literal/name.py"),
    ],
)
def test_real_git_rejects_landing_a_different_colliding_path(
    tmp_path, missing, different
):
    tool = _load_tool()
    source, landed = tmp_path / "source", tmp_path / "landed"
    for root in (source, landed):
        _init_repo(root)
        for name in (missing, different):
            _write(root / name, "original\n")
        _commit_all(root, "base")
    base = _git(landed, "rev-parse", "HEAD").strip()
    _write(source / missing, "index-only source change\n")
    _git(source, "add", "--", missing)
    _write(landed / different, "a different file was landed\n")
    _commit_all(landed, "wrong path")
    args = dict(source_root=source, landed_root=landed, base_ref=base, head_ref="HEAD")
    for filters in (
        {},
        {"owned": ["src/"]},
        {"owned": [missing]},
        {"ignored": [different]},
    ):
        report = tool.audit_dirty_landing(**args, **filters)
        assert not report.ok
        assert report.source_dirty_paths == report.source_only == (missing,)
    report = tool.audit_dirty_landing(**args)
    assert report.landed_range_paths == report.landed_only == (different,)

    _write(landed / missing, "reviewed conflict resolution\n")
    _commit_all(landed, "land the exact missing path")
    report = tool.audit_dirty_landing(**args)
    assert report.ok
    assert report.source_only == ()
    assert report.landed_range_paths == tuple(sorted((missing, different)))


@pytest.mark.skipif(
    os.name == "nt", reason="POSIX Git paths can contain non-UTF-8 bytes"
)
def test_real_git_index_path_bytes_round_trip_through_json(tmp_path):
    tool = _load_tool()
    root = tmp_path / "repo"
    _init_repo(root)
    _write(root / "base.py", "base\n")
    base = _commit_all(root, "base")
    blob = subprocess.run(
        ["git", "hash-object", "-w", "--stdin"],
        cwd=root,
        input=b"indexed contents\n",
        capture_output=True,
        check=True,
    ).stdout.strip()
    # Use the real index protocol so this POSIX Git byte-name control does not
    # depend on whether the host filesystem accepts non-UTF-8 filenames.
    subprocess.run(
        ["git", "update-index", "-z", "--index-info"],
        cwd=root,
        input=b"100644 " + blob + b"\tsrc/non-utf8-\xff.py\0",
        capture_output=True,
        check=True,
    )
    expected = ("src/non-utf8-\udcff.py",)
    assert tool.dirty_source_paths(root) == expected
    _git(root, "commit", "-m", "land indexed byte name")
    assert tool.landed_range_paths(root, base, "HEAD") == expected
    report = tool.audit_dirty_landing(
        source_root=root, landed_root=root, base_ref=base, head_ref="HEAD"
    )
    # The intentionally absent working file remains a deletion. Its indexed
    # addition in the committed range still covers that exact path identity.
    assert report.ok
    assert report.source_dirty_paths == report.landed_range_paths == expected
    payload = json.loads(json.dumps(report.to_json()))
    assert payload["source_dirty_paths"] == list(expected)
