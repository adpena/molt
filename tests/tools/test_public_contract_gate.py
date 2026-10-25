"""Teeth for tools/public_contract_gate.py: the v1 public stable contract gate."""

from __future__ import annotations

import argparse
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
    commands = pcg.cli_surface()["subcommands"]
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
    drifted["cli"]["subcommands"]["build"]["arguments"].append(
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
    commands = dict(pcg.cli_surface()["subcommands"])
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
    assert pcg.declaration_problems(declaration, surface["cli"]["subcommands"]) == [
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


def test_positional_requiredness_is_derived_from_nargs_not_argparse_version() -> None:
    import argparse

    parser = argparse.ArgumentParser()
    rest = parser.add_argument("rest", nargs=argparse.REMAINDER)
    some = parser.add_argument("some", nargs="+")
    flag = parser.add_argument("--flag", required=True)
    # Interpreters before gh-72795 marked REMAINDER positionals required.
    rest.required = True
    assert pcg._action_record(rest)["required"] is False
    assert pcg._action_record(some)["required"] is True
    assert pcg._action_record(flag)["required"] is True


@pytest.mark.parametrize("change", ["global", "converter", "constant", "subcommand"])
def test_snapshot_detects_changes_to_actual_cli_parsing(
    change: str,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    parser = pcg._build_entrypoint_parser()
    monkeypatch.setattr(pcg, "_build_entrypoint_parser", lambda: parser)
    declaration = pcg.load_declaration()
    snapshot = tmp_path / "surface.json"
    pcg.write_surface(snapshot, pcg.live_surface(declaration))
    commands = next(
        action
        for action in parser._actions
        if isinstance(action, argparse._SubParsersAction)
    ).choices

    if change == "global":
        with pytest.raises(SystemExit) as before:
            parser.parse_args(["--version"])
        assert before.value.code == 0
        action = parser._option_string_actions.pop("--version")
        parser._actions.remove(action)
        with pytest.raises(SystemExit) as after:
            parser.parse_args(["--version"])
        assert after.value.code == 2
    elif change == "converter":
        args = ["publish", "package.tar.gz", "--registry-timeout", "bogus"]
        with pytest.raises(SystemExit) as before:
            parser.parse_args(args)
        assert before.value.code == 2
        commands["publish"]._option_string_actions["--registry-timeout"].type = str
        assert parser.parse_args(args).registry_timeout == "bogus"
    elif change == "constant":
        args = ["build", "app.py", "--release"]
        assert parser.parse_args(args).release is True
        commands["build"]._option_string_actions["--release"].const = False
        assert parser.parse_args(args).release is False
    else:
        with pytest.raises(SystemExit) as before:
            parser.parse_args(["extension"])
        assert before.value.code == 2
        action = next(
            action
            for action in commands["extension"]._actions
            if isinstance(action, argparse._SubParsersAction)
        )
        action.required = False
        assert parser.parse_args(["extension"]).extension_command is None

    assert pcg.surface_problems(snapshot, pcg.live_surface(declaration))
    capsys.readouterr()


def test_parser_projection_preserves_positional_order_and_exclusion() -> None:
    parser = argparse.ArgumentParser()
    left = parser.add_argument("left")
    right = parser.add_argument("right")
    before = pcg._parser_record(parser)
    assert vars(parser.parse_args(["a", "b"])) == {"left": "a", "right": "b"}
    left_index, right_index = parser._actions.index(left), parser._actions.index(right)
    parser._actions[left_index], parser._actions[right_index] = right, left
    assert vars(parser.parse_args(["a", "b"])) == {"left": "b", "right": "a"}
    assert pcg._parser_record(parser) != before

    group = parser.add_mutually_exclusive_group()
    group.add_argument("--first", action="store_true")
    group.add_argument("--second", action="store_true")
    before = pcg._parser_record(parser)
    with pytest.raises(SystemExit) as rejected:
        parser.parse_args(["a", "b", "--first", "--second"])
    assert rejected.value.code == 2
    parser._mutually_exclusive_groups.remove(group)
    assert parser.parse_args(["a", "b", "--first", "--second"]).first is True
    assert pcg._parser_record(parser) != before


@pytest.mark.parametrize(
    "flags,expected",
    [
        ([], (False, "auto")),
        (["--precompile"], (True, "auto")),
        (["--platform=cloudflare"], (False, "pure")),
        (["--platform=cloudflare", "--wasm-profile=auto"], (False, "auto")),
        (
            ["--platform=cloudflare", "--precompile", "--wasm-profile=full"],
            (True, "full"),
        ),
    ],
)
def test_parser_omission_markers_preserve_effective_public_defaults(
    monkeypatch, flags, expected
):
    from molt.cli.build_request_resolution import resolve_build_request

    args = pcg._build_entrypoint_parser().parse_args(["build", "app.py", *flags])
    if "--precompile" not in flags:
        assert args.precompile is None
    if not any(flag.startswith("--wasm-profile=") for flag in flags):
        assert args.wasm_profile is None
    # The actual parsed request, not a different process invocation, owns presence.
    monkeypatch.setattr(
        "sys.argv", ["molt", "build", "--precompile", "--wasm-profile=full"]
    )
    request = resolve_build_request(args, build_cfg={}, cfg_capabilities=None, env={})
    assert (request.precompile, request.wasm_profile) == expected
    surface = pcg.cli_surface()["subcommands"]["build"]["arguments"]
    assert {
        row["dest"]: row["default"]
        for row in surface
        if row["dest"] in {"precompile", "wasm_profile"}
    } == {"precompile": None, "wasm_profile": None}
