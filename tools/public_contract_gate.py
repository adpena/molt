#!/usr/bin/env python3
"""Molt v1 public stable contract: declared tiers plus a drift-gated surface snapshot.

The public contract is two files under config/:

* `public_contract_v1.toml` is the reviewed declaration: the stability tier of
  every CLI command, the public artifact/receipt schema identifiers, and the
  compatibility policy constants. Every command the CLI exposes must be tiered
  here; an untiered command is a contract violation, so a new command cannot
  ship without a stability decision.
* `public_contract_v1.surface.json` is the generated snapshot of the surface
  those declarations cover: the full argparse tree of every command, the
  supported target Python versions, the release-target matrix, the exact
  verified-subset matrix digest, and the native callable ABI tokens.

`--check` regenerates the snapshot from the live tree and fails on any
difference. Changing a `stable` command's surface therefore requires an explicit
`--update` in the same landing, which is the reviewable act of changing the
contract. `preview` commands are snapshotted too (so drift is visible) but
their tier documents that their surface may change between minor releases.
`internal` commands exist for the repository's own apparatus (proof queue,
build servers, harnesses) and carry no compatibility promise.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from collections.abc import Mapping
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.exact_json import canonical_json_bytes, loads_exact  # noqa: E402
from molt.release_matrix import RELEASE_TARGETS, SUPPORTED_CPYTHON_VERSIONS  # noqa: E402
from molt.cli.entrypoint_parser import _build_entrypoint_parser  # noqa: E402
from tools import phase_exit_manifest as pem  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
DECLARATION_PATH = ROOT / "config" / "public_contract_v1.toml"
SURFACE_PATH = ROOT / "config" / "public_contract_v1.surface.json"
NATIVE_CALLABLE_ABI_PATH = ROOT / "runtime" / "native_callable_abi.toml"
DECLARATION_SCHEMA = "molt.public-contract.v1"
SURFACE_SCHEMA = "molt.public-contract-surface.v2"
TIERS = frozenset({"stable", "preview", "internal"})
_SCALAR = (str, int, float, bool, type(None))


# --- declaration --------------------------------------------------------------


def load_declaration(path: Path = DECLARATION_PATH) -> dict[str, Any]:
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != DECLARATION_SCHEMA:
        raise ValueError(f"{path}: unsupported public contract schema")
    commands = document.get("command")
    if not isinstance(commands, list) or not commands:
        raise ValueError(f"{path}: command tiers must be a non-empty array of tables")
    tiers: dict[str, str] = {}
    for index, raw in enumerate(commands):
        if not isinstance(raw, Mapping) or set(raw) != {"name", "tier", "since"}:
            raise ValueError(
                f"{path}: command[{index}] needs exactly name, tier, since"
            )
        name, tier, since = raw["name"], raw["tier"], raw["since"]
        if not isinstance(name, str) or not name or name in tiers:
            raise ValueError(f"{path}: command[{index}] name must be a unique string")
        if tier not in TIERS:
            raise ValueError(
                f"{path}: command {name} tier must be one of {sorted(TIERS)}"
            )
        if not isinstance(since, str) or not since:
            raise ValueError(f"{path}: command {name} needs a since version")
        tiers[name] = str(tier)
    schemas = document.get("public_schemas")
    if not isinstance(schemas, list) or not all(
        isinstance(s, str) and s for s in schemas
    ):
        raise ValueError(f"{path}: public_schemas must be a list of identifiers")
    if len(set(schemas)) != len(schemas):
        raise ValueError(f"{path}: public_schemas must be unique")
    policy = document.get("policy")
    if not isinstance(policy, Mapping):
        raise ValueError(f"{path}: policy table is required")
    return {"tiers": tiers, "public_schemas": sorted(schemas), "policy": dict(policy)}


# --- live surface -------------------------------------------------------------


def live_public_schemas() -> list[str]:
    """Read schema identifiers from their executable owners, not the declaration."""
    from molt import compiler_distribution, verified_subset
    from tools import (
        gen_release_matrix,
        legacy_inventory,
        pact_witness_receipt,
        release_exit_gate,
    )
    from tools.release import release_authority, release_model

    return sorted(
        (
            release_model.CONFIG_SCHEMA,
            release_authority.CANDIDATE_SCHEMA,
            release_model.MANIFEST_SCHEMA,
            release_authority.CONSUMER_SCHEMA,
            compiler_distribution.MANIFEST_SCHEMA,
            compiler_distribution.RUNTIME_INVENTORY_SCHEMA,
            gen_release_matrix.SCHEMA,
            f"{release_exit_gate.KIND}/{release_exit_gate.SCHEMA_VERSION}",
            f"{pact_witness_receipt.KIND}/{pact_witness_receipt.SCHEMA_VERSION}",
            pem.MANIFEST_SCHEMA,
            pem.REQUIREMENTS_SCHEMA,
            legacy_inventory.SCHEMA,
            DECLARATION_SCHEMA,
            SURFACE_SCHEMA,
            verified_subset.SCHEMA,
        )
    )


# Positionals that may consume no values. argparse's own `required` flag for
# them changed across CPython patch releases (gh-72795: "*" and REMAINDER
# became non-required in 3.12.7/3.13.1), so the contract derives it from nargs.
_ZERO_VALUE_NARGS = (argparse.OPTIONAL, argparse.ZERO_OR_MORE, argparse.REMAINDER, 0)


def _argument_value(value: Any) -> Any:
    if isinstance(value, _SCALAR):
        return value
    if isinstance(value, list):
        return [_argument_value(item) for item in value]
    raise ValueError(f"CLI contract value must be a JSON scalar or list: {value!r}")


def _action_record(action: argparse.Action) -> dict[str, Any]:
    record: dict[str, Any] = {
        "dest": action.dest,
        "flags": list(action.option_strings),
        "kind": type(action).__name__.lstrip("_"),
        "required": (
            bool(action.required)
            if action.option_strings
            else action.nargs not in _ZERO_VALUE_NARGS
        ),
    }
    if action.nargs is not None:
        record["nargs"] = (
            action.nargs if isinstance(action.nargs, (int, str)) else str(action.nargs)
        )
    if action.choices is not None:
        record["choices"] = sorted(
            (_argument_value(choice) for choice in action.choices),
            key=canonical_json_bytes,
        )
    if action.type is not None:
        module = getattr(action.type, "__module__", None)
        name = getattr(action.type, "__qualname__", None)
        if not module or not name or "<" in name:
            raise ValueError(
                f"CLI converter needs a stable named identity: {action.type!r}"
            )
        record["type"] = f"{module}.{name}"
    record["const"] = _argument_value(action.const)
    if action.default != argparse.SUPPRESS:
        record["default"] = _argument_value(action.default)
    return record


def _parser_record(parser: argparse.ArgumentParser) -> dict[str, Any]:
    arguments: list[dict[str, Any]] = []
    subcommands: dict[str, dict[str, Any]] = {}
    argument_indices: dict[argparse.Action, int] = {}
    record: dict[str, Any] = {
        "arguments": arguments,
        "allow_abbrev": parser.allow_abbrev,
        "prefix_chars": parser.prefix_chars,
        "fromfile_prefix_chars": parser.fromfile_prefix_chars,
        "exit_on_error": parser.exit_on_error,
        "defaults": {
            key: _argument_value(value) for key, value in parser._defaults.items()
        },
    }
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            record["subcommand"] = {
                "dest": action.dest,
                "required": bool(action.required),
            }
            for name, subparser in sorted(action.choices.items()):
                subcommands[name] = _parser_record(subparser)
            continue
        argument = _action_record(action)
        argument_indices[action] = len(arguments)
        arguments.append(argument)
    if parser._mutually_exclusive_groups:
        record["mutually_exclusive"] = [
            {
                "required": bool(group.required),
                "arguments": [
                    argument_indices[action] for action in group._group_actions
                ],
            }
            for group in parser._mutually_exclusive_groups
        ]
    if subcommands:
        record["subcommands"] = subcommands
    return record


def cli_surface() -> dict[str, Any]:
    return _parser_record(_build_entrypoint_parser())


def native_callable_abi_tokens(
    path: Path = NATIVE_CALLABLE_ABI_PATH,
) -> list[dict[str, Any]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    tokens = []
    for entry in data.get("abi", []):
        tokens.append(
            {
                "token": str(entry["token"]),
                "browser_params": list(entry["browser_params"]),
                "browser_result": str(entry["browser_result"]),
                "wasm_params": list(entry["wasm_params"]),
                "wasm_results": list(entry["wasm_results"]),
                "native_params": list(entry["native_params"]),
                "native_results": list(entry["native_results"]),
            }
        )
    return sorted(tokens, key=lambda item: item["token"])


def live_surface(declaration: Mapping[str, Any]) -> dict[str, Any]:
    cli = cli_surface()
    for name, command in cli["subcommands"].items():
        command["tier"] = declaration["tiers"].get(name)
    return {
        "schema": SURFACE_SCHEMA,
        "cli": cli,
        "target_python_versions": list(SUPPORTED_CPYTHON_VERSIONS),
        "release_targets": [
            dict(sorted(dict(target).items())) for target in RELEASE_TARGETS
        ],
        "verified_subset_matrix_digest": pem.generated_matrix_digest(),
        "native_callable_abi": native_callable_abi_tokens(),
        "public_schemas": live_public_schemas(),
    }


# --- checks -------------------------------------------------------------------


def declaration_problems(
    declaration: Mapping[str, Any], commands: Mapping[str, Any]
) -> list[str]:
    problems: list[str] = []
    tiers: Mapping[str, str] = declaration["tiers"]
    for name in sorted(commands):
        if name not in tiers:
            problems.append(
                f"command {name!r} is exposed by the CLI but has no stability tier"
            )
    for name in sorted(tiers):
        if name not in commands:
            problems.append(f"declared command {name!r} does not exist in the CLI")
    declared = set(declaration["public_schemas"])
    produced = set(live_public_schemas())
    for schema in sorted(produced - declared):
        problems.append(f"public schema {schema!r} has no reviewed declaration")
    for schema in sorted(declared - produced):
        problems.append(f"declared public schema {schema!r} has no matching producer")
    return problems


def surface_problems(expected_path: Path, actual: Mapping[str, Any]) -> list[str]:
    if not expected_path.is_file():
        return [f"surface snapshot is missing: {expected_path}"]
    expected = loads_exact(expected_path.read_text(encoding="utf-8"))
    if canonical_json_bytes(expected) == canonical_json_bytes(actual):
        return []
    problems: list[str] = []
    for key in sorted(set(expected) | set(actual)):
        if key == "cli":
            continue
        if expected.get(key) != actual.get(key):
            problems.append(f"{key} changed")
    expected_cli = expected.get("cli", {})
    actual_cli = actual.get("cli", {})
    if {key: value for key, value in expected_cli.items() if key != "subcommands"} != {
        key: value for key, value in actual_cli.items() if key != "subcommands"
    }:
        problems.append("entrypoint surface changed")
    expected_commands = expected_cli.get("subcommands", {})
    actual_commands = actual_cli.get("subcommands", {})
    for name in sorted(set(expected_commands) | set(actual_commands)):
        if expected_commands.get(name) != actual_commands.get(name):
            tier = (actual_commands.get(name) or expected_commands.get(name) or {}).get(
                "tier"
            )
            problems.append(f"command {name!r} ({tier}) surface changed")
    return problems or ["surface snapshot differs"]


def write_surface(path: Path, surface: Mapping[str, Any]) -> None:
    path.write_bytes(canonical_json_bytes(surface) + b"\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "--check",
        action="store_true",
        help="fail on undeclared commands or surface drift",
    )
    group.add_argument(
        "--update",
        action="store_true",
        help="rewrite the surface snapshot from the live tree",
    )
    args = parser.parse_args(argv)
    declaration = load_declaration()
    surface = live_surface(declaration)
    commands = surface["cli"]["subcommands"]
    problems = declaration_problems(declaration, commands)
    if args.update:
        if problems:
            for problem in problems:
                print(f"[public-contract] {problem}")
            return 1
        write_surface(SURFACE_PATH, surface)
        print(f"[public-contract] surface snapshot written: {SURFACE_PATH.as_posix()}")
        return 0
    problems.extend(surface_problems(SURFACE_PATH, surface))
    for problem in problems:
        print(f"[public-contract] {problem}")
    counts = {
        tier: sum(1 for value in declaration["tiers"].values() if value == tier)
        for tier in sorted(TIERS)
    }
    rendered = " ".join(f"{tier}={count}" for tier, count in counts.items())
    print(
        f"[public-contract] {'OK' if not problems else 'DRIFT'} "
        f"commands={len(commands)} {rendered}"
    )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
