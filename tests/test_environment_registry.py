"""The environment registry: loader, diagnostic, and generated projections.

Oracles: ``tomllib`` on the TOML authority (not the loader), the rendered
text of the generator (byte identity), and hand-built payloads for the
diagnostic. Retired names are taken from the registry at run time so this
file never spells one.
"""

from __future__ import annotations

import io
import os
import sys
import tomllib
from pathlib import Path

import pytest

from molt import environment_registry as R
from molt.dx import DxConfigError, RunContext
from tests.process_guard_common import install_module_os_view

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "tools") not in sys.path:
    sys.path.insert(0, str(ROOT / "tools"))

import gen_environment_registry as G  # noqa: E402

REGISTRY_TOML = ROOT / "src" / "molt" / "environment_registry.toml"


# --------------------------------------------------------------------------
# Generated projections stay in sync with the TOML authority
# --------------------------------------------------------------------------


def test_projection_and_reference_page_are_current() -> None:
    assert G.projection_is_current(), (
        "run `python3 tools/gen_environment_registry.py --write`; the projection or "
        "the reference page is stale"
    )


def test_generator_is_idempotent() -> None:
    payload = G.build_payload(G.load_source())
    assert G.render_python(payload) == G.render_python(payload)
    assert G.render_doc(payload) == G.render_doc(payload)


def test_authority_rows_are_sorted_unique_and_shipped_loader_agrees() -> None:
    data = tomllib.loads(REGISTRY_TOML.read_text(encoding="utf-8"))
    names = [row["name"] for row in data["variable"]]
    assert names == sorted(names) and len(set(names)) == len(names)
    stems = [row["name"] for row in data["stem"]]
    assert stems == sorted(stems) and len(set(stems)) == len(stems)
    registry = R.load_registry()
    assert [v.name for v in registry.variables] == names
    assert [s.name for s in registry.stems] == stems
    assert {f.suffix for f in registry.families} == {
        row["suffix"] for row in data["family"]
    }


def test_every_summary_is_one_sentence_and_every_audience_is_known() -> None:
    registry = R.load_registry()
    for row in (*registry.variables, *registry.stems, *registry.families):
        assert row.summary.endswith(".") and "\n" not in row.summary, row
    for row in (*registry.variables, *registry.families, *registry.prefix_families):
        assert row.audience in R.AUDIENCES, row
        assert row.kind in R.KINDS, row


# --------------------------------------------------------------------------
# Lookup semantics
# --------------------------------------------------------------------------


def test_family_lookup_resolves_stem_suffix_and_root_fallback() -> None:
    registry = R.load_registry()
    match = registry.lookup_family("MOLT_BENCH_TIMEOUT_SEC")
    assert match is not None
    assert match.stem is not None and match.stem.name == "MOLT_BENCH"
    assert match.family.suffix == "_TIMEOUT_SEC"
    root = registry.lookup_family("MOLT_TEST_PROCESS_TIMEOUT_SEC")
    assert root is not None and root.stem is None
    assert root.family.suffix == "_TIMEOUT_SEC"
    assert registry.lookup_family("MOLT_NOT_A_STEM_TIMEOUT_SEC") is None
    assert registry.is_registered("MOLT_BENCH_MAX_PROCESS_RSS_GB")
    assert not registry.is_registered("MOLT_NOT_A_STEM_MAX_PROCESS_RSS_GB")


def test_retired_suffix_resolves_for_every_stem_and_for_the_root() -> None:
    registry = R.load_registry()
    suffix = registry.retired_suffixes[0]
    stem = registry.stems[0].name
    retired = registry.lookup_retired(f"{stem}{suffix.suffix}")
    assert retired is not None
    assert retired.replacement == f"{stem}{suffix.replacement_suffix}"
    root = registry.lookup_retired(f"MOLT{suffix.suffix}")
    assert root is not None and root.replacement == f"MOLT{suffix.replacement_suffix}"
    assert registry.lookup_retired(f"MOLT_NOT_A_STEM{suffix.suffix}") is None


def test_retired_names_are_never_registered_and_replacements_are() -> None:
    registry = R.load_registry()
    assert registry.retired, "the registry documents its retired names"
    for row in registry.retired:
        assert not registry.is_registered(row.name), row.name
        if row.replacement:
            assert registry.is_registered(row.replacement), row
        else:
            assert row.note, row


# --------------------------------------------------------------------------
# The diagnostic
# --------------------------------------------------------------------------


def _retired_fixed_name() -> R.RetiredVariable:
    return next(row for row in R.load_registry().retired if row.replacement)


def test_unknown_name_warns_with_a_close_match() -> None:
    errors, warnings = R.inspect_environment({"MOLT_HOEM": "/x", "PATH": "/bin"})
    assert errors == []
    assert len(warnings) == 1
    assert warnings[0].name == "MOLT_HOEM"
    assert "MOLT_HOME" in warnings[0].message
    assert "ignored" in warnings[0].message


def test_registered_names_and_family_expansions_are_silent() -> None:
    env = {
        "MOLT_HOME": "/x",
        "MOLT_BENCH_TIMEOUT_SEC": "5",
        "MOLT_MAX_PROCESS_RSS_GB": "4",
        "HOME": "/h",
    }
    assert R.inspect_environment(env) == ([], [])


def test_registry_is_not_loaded_when_no_molt_name_is_set(monkeypatch) -> None:
    def explode() -> R.EnvironmentRegistry:
        raise AssertionError("registry loaded for an environment without MOLT_* keys")

    monkeypatch.setattr(R, "load_registry", explode)
    assert R.inspect_environment({"PATH": "/bin", "HOME": "/h"}) == ([], [])


def test_retired_name_is_an_error_that_names_the_replacement() -> None:
    row = _retired_fixed_name()
    errors, warnings = R.inspect_environment({row.name: "1"})
    assert warnings == []
    assert len(errors) == 1 and errors[0].name == row.name
    assert row.replacement in errors[0].message
    assert row.retired in errors[0].message


def test_retired_family_suffix_is_an_error_for_a_stem() -> None:
    registry = R.load_registry()
    suffix = registry.retired_suffixes[0]
    name = f"{registry.stems[0].name}{suffix.suffix}"
    errors, _warnings = R.inspect_environment({name: "1"})
    assert len(errors) == 1
    assert f"{registry.stems[0].name}{suffix.replacement_suffix}" in errors[0].message


def test_lower_case_spelling_is_reported_on_case_sensitive_platforms(
    monkeypatch,
) -> None:
    install_module_os_view(monkeypatch, R, name="posix")
    errors, warnings = R.inspect_environment({"molt_home": "/x"})
    assert errors == []
    assert len(warnings) == 1
    assert "case-sensitive" in warnings[0].message
    assert "MOLT_HOME" in warnings[0].message


def test_windows_matches_names_case_insensitively(monkeypatch) -> None:
    install_module_os_view(monkeypatch, R, name="nt")
    assert R.inspect_environment({"molt_home": "/x"}) == ([], [])
    row = _retired_fixed_name()
    errors, _warnings = R.inspect_environment({row.name.lower(): "1"})
    assert len(errors) == 1 and row.replacement in errors[0].message


def test_check_process_environment_prints_each_warning_once_and_raises(
    monkeypatch,
) -> None:
    monkeypatch.setattr(R, "_REPORTED_WARNINGS", set())
    stream = io.StringIO()
    R.check_process_environment({"MOLT_HOEM": "/x"}, stream=stream, program="molt")
    R.check_process_environment({"MOLT_HOEM": "/x"}, stream=stream, program="molt")
    lines = [line for line in stream.getvalue().splitlines() if line]
    assert len(lines) == 1
    assert lines[0].startswith("molt: warning: MOLT_HOEM")
    row = _retired_fixed_name()
    with pytest.raises(R.EnvironmentRegistryError, match=row.replacement):
        R.check_process_environment({row.name: "1"}, stream=stream)


def test_payload_validation_rejects_malformed_rows() -> None:
    payload = {
        "variables": [{"name": "MOLT_X", "audience": 1}],
        "stems": [],
        "families": [],
        "prefix_families": [],
        "retired": [],
        "retired_suffixes": [],
    }
    with pytest.raises(R.EnvironmentRegistryError, match="audience"):
        R.registry_from_payload(payload)
    with pytest.raises(R.EnvironmentRegistryError, match="must be a list"):
        R.registry_from_payload({"variables": {}})


# --------------------------------------------------------------------------
# Wiring: the CLI entry and the developer RunContext both run the diagnostic
# --------------------------------------------------------------------------


def test_cli_entry_fails_closed_on_a_retired_name(monkeypatch, capsys) -> None:
    from molt.cli import entrypoint

    row = _retired_fixed_name()
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setenv("MOLT_HASH_SEED", "0")
    monkeypatch.setenv(row.name, "1")
    monkeypatch.setattr(sys, "argv", ["molt", "--help"])
    assert entrypoint.main(build_fn=lambda *a, **k: 0) == 2
    captured = capsys.readouterr()
    assert "molt: error:" in captured.err
    assert row.replacement in captured.err


def test_run_context_rejects_a_retired_name_before_any_custody_work(tmp_path) -> None:
    row = _retired_fixed_name()
    context = RunContext(tmp_path)
    with pytest.raises(DxConfigError, match=row.replacement):
        context.canonical_env({row.name: "1", "PATH": os.environ.get("PATH", "")})
