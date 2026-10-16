"""Dependabot watches exactly the dependency manifests the repository tracks."""

from __future__ import annotations

from pathlib import Path, PurePosixPath

import yaml

from tools.dependency_manifests import cargo_workspace_dirs, tracked_manifest_dirs

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / ".github" / "dependabot.yml"


def _dependabot_dirs(dirs: list[PurePosixPath]) -> set[str]:
    return {"/" if d == PurePosixPath(".") else f"/{d}" for d in dirs}


def _manifest_dirs(*patterns: str) -> set[str]:
    return _dependabot_dirs(tracked_manifest_dirs(ROOT, *patterns))


def _configured() -> dict[str, set[str]]:
    config = yaml.safe_load(CONFIG.read_text(encoding="utf-8"))
    assert config["version"] == 2
    configured: dict[str, set[str]] = {}
    for update in config["updates"]:
        ecosystem = update["package-ecosystem"]
        assert ecosystem not in configured, f"{ecosystem} listed twice"
        directories = update.get("directories") or [update["directory"]]
        configured[ecosystem] = set(directories)
        assert update["schedule"]["interval"] == "weekly"
    return configured


def test_every_tracked_lockfile_is_watched_and_nothing_else() -> None:
    configured = _configured()
    assert configured["cargo"] == _dependabot_dirs(cargo_workspace_dirs(ROOT))
    assert configured["uv"] == _manifest_dirs("*uv.lock", "uv.lock")
    assert configured["npm"] == _manifest_dirs("*package-lock.json")


def test_workflows_and_every_composite_action_are_watched() -> None:
    actions = _manifest_dirs(".github/actions/*/action.yml")
    assert actions, "expected composite actions"
    assert all(path.startswith("/.github/actions/") for path in actions)
    assert _configured()["github-actions"] == {"/", "/.github/actions/*"}
