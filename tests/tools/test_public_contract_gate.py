"""Teeth for tools/public_contract_gate.py: the v1 public stable contract gate."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from molt.exact_json import canonical_json_bytes
from tools import public_contract_gate as pcg

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def matrix_digest() -> str:
    return pcg.pem.generated_matrix_digest()


@pytest.fixture(autouse=True)
def immutable_matrix_projection(monkeypatch: pytest.MonkeyPatch, matrix_digest: str):
    # These cases change CLI/schema declarations, never the subset matrix. Reuse
    # its real immutable projection instead of rebuilding it for every assertion.
    monkeypatch.setattr(pcg.pem, "generated_matrix_digest", lambda: matrix_digest)


def test_every_cli_command_carries_a_declared_tier() -> None:
    declaration = pcg.load_declaration()
    commands = pcg.cli_surface()
    assert pcg.declaration_problems(declaration, commands) == []
    assert set(declaration["tiers"]) == set(commands)
    assert set(declaration["tiers"].values()) <= pcg.TIERS
    # The core product surface is stable; repository apparatus never is.
    for name in ("build", "run", "test", "check", "config", "doctor"):
        assert declaration["tiers"][name] == "stable"
    for name in ("queue", "harness", "internal-batch-build-server"):
        assert declaration["tiers"][name] == "internal"


def test_snapshot_matches_the_live_tree_and_is_canonical() -> None:
    declaration = pcg.load_declaration()
    surface = pcg.live_surface(declaration)
    assert pcg.surface_problems(pcg.SURFACE_PATH, surface) == []
    assert pcg.SURFACE_PATH.read_bytes() == canonical_json_bytes(surface) + b"\n"
    assert surface["schema"] == pcg.SURFACE_SCHEMA
    assert surface["target_python_versions"][0] == "3.12"
    assert {target["id"] for target in surface["release_targets"]} >= {
        "linux-x86_64",
        "windows-x86_64",
        "macos-arm64",
    }
    assert len(surface["verified_subset_matrix_digest"]) == 64
    assert any(
        item["token"] == "molt.forward_f32_v1"
        for item in surface["native_callable_abi"]
    )
    text = pcg.SURFACE_PATH.read_text(encoding="utf-8")
    assert "adpen" not in text and "C:\\\\" not in text


def test_surface_drift_names_the_command_and_tier(tmp_path: Path) -> None:
    declaration = pcg.load_declaration()
    surface = pcg.live_surface(declaration)
    snapshot = tmp_path / "surface.json"
    pcg.write_surface(snapshot, surface)
    assert pcg.surface_problems(snapshot, surface) == []

    drifted = json.loads(json.dumps(surface))
    drifted["commands"]["build"]["arguments"].append(
        {
            "dest": "turbo",
            "flags": ["--turbo"],
            "kind": "StoreTrueAction",
            "required": False,
        }
    )
    problems = pcg.surface_problems(snapshot, drifted)
    assert problems == ["command 'build' (stable) surface changed"]

    drifted = json.loads(json.dumps(surface))
    drifted["target_python_versions"] = ["3.13"]
    assert "target_python_versions changed" in pcg.surface_problems(snapshot, drifted)


def test_undeclared_and_phantom_commands_are_problems() -> None:
    declaration = pcg.load_declaration()
    commands = dict(pcg.cli_surface())
    commands["shiny"] = {"arguments": []}
    problems = pcg.declaration_problems(declaration, commands)
    assert problems == [
        "command 'shiny' is exposed by the CLI but has no stability tier"
    ]
    del commands["shiny"]
    del commands["build"]
    assert pcg.declaration_problems(declaration, commands) == [
        "declared command 'build' does not exist in the CLI"
    ]


def test_producer_schema_drift_cannot_be_blessed_by_snapshot_update(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    from molt import compiler_distribution

    declaration = pcg.load_declaration()
    previous = compiler_distribution.MANIFEST_SCHEMA
    changed = "molt.release-compiler-source.unreviewed"
    monkeypatch.setattr(compiler_distribution, "MANIFEST_SCHEMA", changed)
    surface = pcg.live_surface(declaration)
    assert changed in surface["public_schemas"]
    assert previous not in surface["public_schemas"]
    assert pcg.declaration_problems(declaration, surface["commands"]) == [
        f"public schema {changed!r} has no reviewed declaration",
        f"declared public schema {previous!r} has no matching producer",
    ]
    snapshot = tmp_path / "surface.json"
    snapshot.write_bytes(b"unchanged snapshot")
    monkeypatch.setattr(pcg, "SURFACE_PATH", snapshot)
    assert pcg.main(["--update"]) == 1
    assert snapshot.read_bytes() == b"unchanged snapshot"
    assert "no reviewed declaration" in capsys.readouterr().out


def test_duplicate_schema_declarations_are_rejected(tmp_path: Path) -> None:
    path = tmp_path / "public_contract.toml"
    path.write_text(
        pcg.DECLARATION_PATH.read_text(encoding="utf-8").replace(
            "public_schemas = [", 'public_schemas = ["molt.public-contract.v1",'
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="public_schemas must be unique"):
        pcg.load_declaration(path)


def test_declaration_rejects_untiered_or_malformed_rows(tmp_path: Path) -> None:
    path = tmp_path / "public_contract_v1.toml"
    path.write_text(
        'schema = "molt.public-contract.v1"\npublic_schemas = []\n[policy]\nversioning = "semver"\n'
        '[[command]]\nname = "build"\ntier = "gold"\nsince = "1.0.0"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="tier must be one of"):
        pcg.load_declaration(path)


def test_check_and_update_cli(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    snapshot = tmp_path / "surface.json"
    monkeypatch.setattr(pcg, "SURFACE_PATH", snapshot)
    assert pcg.main(["--check"]) == 1
    assert "surface snapshot is missing" in capsys.readouterr().out
    assert pcg.main(["--update"]) == 0
    assert pcg.main(["--check"]) == 0
    assert "[public-contract] OK" in capsys.readouterr().out
