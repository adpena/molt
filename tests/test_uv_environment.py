"""One shared uv environment per key and lock (``molt.uv_environment``).

The oracle is ``tests/uv_environment_test_support.py``'s fake ``uv``.
"""

from __future__ import annotations

import platform
from pathlib import Path

import pytest

from molt import uv_environment
from molt.uv_environment import EnvironmentKey, UvEnvironmentError
from tests.uv_environment_test_support import fake_uv_environ, make_checkout, uv_calls


@pytest.fixture
def fake_uv(tmp_path: Path) -> tuple[dict[str, str], Path]:
    return fake_uv_environ(tmp_path)


_checkout = make_checkout
_calls = uv_calls


KEY = EnvironmentKey(platform.python_version(), ("dev",))


def test_checkouts_with_one_lock_share_one_environment_and_one_sync(
    tmp_path: Path, fake_uv: tuple[dict[str, str], Path]
) -> None:
    environ, log = fake_uv
    first = _checkout(tmp_path / "first")
    second = _checkout(tmp_path / "second")

    root = uv_environment.environment(first, KEY, environ=environ)
    assert uv_environment.environment(second, KEY, environ=environ) == root
    assert len(_calls(log)) == 1
    assert uv_environment.interpreter_identity(root)["version"] == KEY.python
    assert uv_environment.selected(second, environ, KEY) == root


def test_the_sync_installs_the_key_groups_without_the_project(
    tmp_path: Path, fake_uv: tuple[dict[str, str], Path]
) -> None:
    environ, log = fake_uv
    uv_environment.environment(_checkout(tmp_path / "c"), KEY, environ=environ)
    (call,) = _calls(log)
    for token in (
        '"sync"',
        '"--locked"',
        '"--no-install-project"',
        '"--no-default-groups"',
    ):
        assert token in call
    assert f'"--python", "{KEY.python}"' in call
    assert '"--group", "dev"' in call


def test_a_lock_change_selects_another_environment_and_keeps_the_old_one(
    tmp_path: Path, fake_uv: tuple[dict[str, str], Path]
) -> None:
    environ, log = fake_uv
    checkout = _checkout(tmp_path / "c")
    old = uv_environment.environment(checkout, KEY, environ=environ)
    (checkout / "uv.lock").write_text("version = 2\n", encoding="utf-8")
    new = uv_environment.environment(checkout, KEY, environ=environ)
    assert new != old and new.parent == old.parent
    assert (old / uv_environment.RECORD).is_file()
    assert len(_calls(log)) == 2


@pytest.mark.parametrize(
    ("edit", "same"),
    [
        (('description = "first"', 'description = "second"'), True),
        (("line-length = 88", "line-length = 100"), True),
        (('dev = ["pytest"]', 'dev = ["pytest", "ruff"]'), False),
        (("dependencies = []", 'dependencies = ["packaging"]'), False),
    ],
    ids=["description", "tool-setting", "group", "dependency"],
)
def test_only_pyproject_entries_that_decide_contents_change_the_directory(
    tmp_path: Path, edit: tuple[str, str], same: bool
) -> None:
    checkout = _checkout(tmp_path / "c")
    environ = {"MOLT_EXT_ROOT": str(tmp_path / "artifacts")}
    before = uv_environment.environment_root(checkout, KEY, environ)
    text = (checkout / "pyproject.toml").read_text(encoding="utf-8")
    (checkout / "pyproject.toml").write_text(text.replace(*edit), encoding="utf-8")
    after = uv_environment.environment_root(checkout, KEY, environ)
    assert (before == after) is same


def test_a_failed_sync_publishes_no_record_and_the_next_use_syncs_again(
    tmp_path: Path, fake_uv: tuple[dict[str, str], Path]
) -> None:
    environ, log = fake_uv
    checkout = _checkout(tmp_path / "c")
    with pytest.raises(UvEnvironmentError, match="uv sync failed"):
        uv_environment.environment(
            checkout, KEY, environ={**environ, "FAKE_UV_FAIL": "1"}
        )
    assert uv_environment.selected(checkout, environ, KEY) is None
    root = uv_environment.environment(checkout, KEY, environ=environ)
    assert (root / uv_environment.RECORD).is_file()
    assert len(_calls(log)) == 2


def test_an_interpreter_other_than_the_key_release_is_refused(
    tmp_path: Path, fake_uv: tuple[dict[str, str], Path]
) -> None:
    environ, _log = fake_uv
    major, minor, micro = platform.python_version_tuple()
    other = EnvironmentKey(f"{major}.{minor}.{int(micro) + 1}", ("dev",))
    checkout = _checkout(tmp_path / "c")
    with pytest.raises(UvEnvironmentError, match="uv selected CPython"):
        uv_environment.environment(checkout, other, environ=environ)
    assert uv_environment.selected(checkout, environ, other) is None


def test_an_explicit_project_environment_wins_over_the_key(tmp_path: Path) -> None:
    checkout = _checkout(tmp_path / "c")
    explicit = tmp_path / "elsewhere"
    assert (
        uv_environment.selected(
            checkout, {"UV_PROJECT_ENVIRONMENT": str(explicit)}, KEY
        )
        == explicit
    )


@pytest.mark.parametrize(
    ("python", "groups"),
    [("3.12", ()), ("3.12.x", ()), ("3.12.16", ("dev", "bench")), ("3.12.16", ("A",))],
    ids=["minor", "malformed", "unsorted", "uppercase"],
)
def test_a_key_names_an_exact_release_and_sorted_group_names(
    python: str, groups: tuple[str, ...]
) -> None:
    with pytest.raises(UvEnvironmentError):
        EnvironmentKey(python, groups)


def test_an_exact_release_resolves_to_itself_without_uv(tmp_path: Path) -> None:
    assert (
        uv_environment.resolve_release("3.13.18", tmp_path, {"PATH": ""}) == "3.13.18"
    )
    with pytest.raises(UvEnvironmentError, match="release or minor"):
        uv_environment.resolve_release("latest", tmp_path, {"PATH": ""})
