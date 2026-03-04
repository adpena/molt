from __future__ import annotations

from pathlib import Path

import tools.symphony_bootstrap as symphony_bootstrap


def test_parse_env_file_reads_key_values(tmp_path: Path) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        "# comment\nLINEAR_API_KEY=abc123\nMOLT_LINEAR_PROJECT_SLUG = molt-runtime\n",
        encoding="utf-8",
    )
    parsed = symphony_bootstrap._parse_env_file(env_file)
    assert parsed["LINEAR_API_KEY"] == "abc123"
    assert parsed["MOLT_LINEAR_PROJECT_SLUG"] == "molt-runtime"


def test_sync_env_defaults_fills_external_paths(monkeypatch, tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    ext_root = tmp_path / "ext"
    ext_root.mkdir()
    env_file = tmp_path / "symphony.env"

    monkeypatch.setattr(
        symphony_bootstrap,
        "_git_origin",
        lambda _: "git@github.com:org/molt.git",
    )

    summary = symphony_bootstrap._sync_env_defaults(
        repo_root=repo_root,
        ext_root=ext_root,
        env_file=env_file,
        project_slug="molt-project",
        source_repo_url=None,
    )
    loaded = symphony_bootstrap._parse_env_file(env_file)

    assert summary["missing_required"] == ["LINEAR_API_KEY"]
    assert loaded["MOLT_LINEAR_PROJECT_SLUG"] == "molt-project"
    assert loaded["MOLT_SOURCE_REPO_URL"] == "git@github.com:org/molt.git"
    assert loaded["MOLT_EXT_ROOT"] == str(ext_root)
