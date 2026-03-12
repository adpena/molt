from __future__ import annotations

from pathlib import Path

from molt.symphony.paths import (
    default_symphony_env_file,
    legacy_symphony_env_file,
    resolve_symphony_env_file,
)


def test_resolve_symphony_env_file_prefers_explicit_alias(monkeypatch, tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    explicit = repo_root / "ops" / "linear" / "runtime" / "custom.env"
    monkeypatch.setenv("FLEET_MOLT_SYMPHONY_ENV_FILE", str(explicit))

    resolved = resolve_symphony_env_file(repo_root=repo_root)

    assert resolved == explicit


def test_resolve_symphony_env_file_prefers_canonical_when_present(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    canonical = default_symphony_env_file(repo_root)
    canonical.parent.mkdir(parents=True, exist_ok=True)
    canonical.write_text("LINEAR_API_KEY=test\n", encoding="utf-8")

    resolved = resolve_symphony_env_file(repo_root=repo_root, env={})

    assert resolved == canonical


def test_resolve_symphony_env_file_falls_back_to_legacy_path(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"

    resolved = resolve_symphony_env_file(repo_root=repo_root, env={})

    assert resolved == legacy_symphony_env_file(repo_root)
