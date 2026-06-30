from __future__ import annotations

import importlib.util
import subprocess
from pathlib import Path
import sys


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
