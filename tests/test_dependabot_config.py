"""Dependabot watches exactly the dependency manifests the repository tracks."""

from __future__ import annotations

from pathlib import Path

import yaml

from tests.process_guard_common import check_output_custody_subject_process

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / ".github" / "dependabot.yml"

# Fixture projects keep their pins on purpose; automation must not move them.
FIXTURE_PREFIX = "tests/fixtures/"


def _tracked(*patterns: str) -> list[str]:
    output = check_output_custody_subject_process(
        ["git", "ls-files", "-z", "--", *patterns],
        cwd=ROOT,
        text=True,
        encoding="utf-8",
    )
    assert isinstance(output, str)
    return [path for path in output.split("\0") if path]


def _manifest_dirs(*patterns: str) -> set[str]:
    dirs = set()
    for path in _tracked(*patterns):
        if path.startswith(FIXTURE_PREFIX):
            continue
        parent = Path(path).parent.as_posix()
        dirs.add("/" if parent == "." else f"/{parent}")
    return dirs


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
    assert configured["cargo"] == _manifest_dirs("*Cargo.lock", "Cargo.lock")
    assert configured["uv"] == _manifest_dirs("*uv.lock", "uv.lock")
    assert configured["npm"] == _manifest_dirs("*package-lock.json")


def test_workflows_and_every_composite_action_are_watched() -> None:
    actions = _manifest_dirs(".github/actions/*/action.yml")
    assert actions, "expected composite actions"
    assert all(path.startswith("/.github/actions/") for path in actions)
    assert _configured()["github-actions"] == {"/", "/.github/actions/*"}
