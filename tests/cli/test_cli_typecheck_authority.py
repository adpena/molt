from __future__ import annotations

import inspect
from pathlib import Path

import molt.cli as cli
from molt.cli import typecheck
from molt.type_facts import TypeFacts

_TYPECHECK_NAMES = (
    "_collect_py_files",
    "_collect_type_facts_for_build",
    "_read_cached_type_facts",
    "_run_ty_check",
    "_type_facts_cache_key",
    "_type_facts_cache_path",
    "_type_facts_cache_root",
    "_type_facts_source_identity",
    "_type_facts_tooling_identity",
    "_write_cached_type_facts",
    "check",
)


def test_cli_typecheck_authority_is_single_home() -> None:
    for name in _TYPECHECK_NAMES:
        assert hasattr(typecheck, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _TYPECHECK_NAMES:
        assert f"def {name}(" not in cli_source


def test_collect_type_facts_for_build_reuses_successful_ty_cache(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "main.py"
    source.write_text("VALUE: int = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    ty_checks: list[Path] = []
    collects: list[bool] = []

    def fake_ty_check(path: Path) -> tuple[bool, str]:
        ty_checks.append(path)
        return True, "ok"

    def fake_collect(paths, trust, infer=False):
        collects.append(infer)
        facts = TypeFacts(strict=(trust == "trusted"))
        facts.tool = "fresh"
        return facts

    monkeypatch.setattr(typecheck, "_run_ty_check", fake_ty_check)
    monkeypatch.setattr(typecheck, "collect_type_facts_from_paths", fake_collect)

    first, first_ok = typecheck._collect_type_facts_for_build(
        [source], "check", source
    )
    second, second_ok = typecheck._collect_type_facts_for_build(
        [source], "check", source
    )

    assert first_ok is True
    assert second_ok is True
    assert first is not None
    assert second is not None
    assert first.tool == "molt-check+ty+infer"
    assert second.tool == "molt-check+ty+infer"
    assert ty_checks == [source]
    assert collects == [True]


def test_collect_type_facts_cache_key_invalidates_on_source_edit(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "main.py"
    source.write_text("VALUE: int = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    ty_checks = 0

    def fake_ty_check(path: Path) -> tuple[bool, str]:
        nonlocal ty_checks
        ty_checks += 1
        return True, "ok"

    monkeypatch.setattr(typecheck, "_run_ty_check", fake_ty_check)

    typecheck._collect_type_facts_for_build([source], "check", source)
    typecheck._collect_type_facts_for_build([source], "check", source)
    source.write_text("VALUE: int = 2\n", encoding="utf-8")
    typecheck._collect_type_facts_for_build([source], "check", source)

    assert ty_checks == 2


def test_collect_type_facts_does_not_cache_failed_ty_result(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "main.py"
    source.write_text("VALUE: int = 1\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    ty_checks = 0

    def fake_ty_check(path: Path) -> tuple[bool, str]:
        nonlocal ty_checks
        ty_checks += 1
        return False, "ty failed"

    monkeypatch.setattr(typecheck, "_run_ty_check", fake_ty_check)

    first, first_ok = typecheck._collect_type_facts_for_build(
        [source], "check", source
    )
    second, second_ok = typecheck._collect_type_facts_for_build(
        [source], "check", source
    )

    assert first is not None
    assert second is not None
    assert first_ok is False
    assert second_ok is False
    assert ty_checks == 2
