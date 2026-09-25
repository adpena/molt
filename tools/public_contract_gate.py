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

from molt.exact_json import canonical_json_bytes, loads_exact
from molt.release_matrix import RELEASE_TARGETS, SUPPORTED_CPYTHON_VERSIONS
from molt.cli.entrypoint_parser import _build_entrypoint_parser
from tools import phase_exit_manifest as pem

ROOT = Path(__file__).resolve().parents[1]
DECLARATION_PATH = ROOT / "config" / "public_contract_v1.toml"
SURFACE_PATH = ROOT / "config" / "public_contract_v1.surface.json"
NATIVE_CALLABLE_ABI_PATH = ROOT / "runtime" / "native_callable_abi.toml"
DECLARATION_SCHEMA = "molt.public-contract.v1"
SURFACE_SCHEMA = "molt.public-contract-surface.v1"
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


def _action_record(action: argparse.Action) -> dict[str, Any] | None:
    if isinstance(action, argparse._HelpAction) or action.dest == "help":
        return None
    if isinstance(action, argparse._SubParsersAction):
        return None
    record: dict[str, Any] = {
        "dest": action.dest,
        "flags": list(action.option_strings),
        "kind": type(action).__name__.lstrip("_"),
        "required": bool(action.required),
    }
    if action.nargs is not None:
        record["nargs"] = (
            action.nargs if isinstance(action.nargs, (int, str)) else str(action.nargs)
        )
    if action.choices is not None:
        record["choices"] = sorted(str(choice) for choice in action.choices)
    if isinstance(action.default, _SCALAR) and action.default is not argparse.SUPPRESS:
        record["default"] = action.default
    return record


def _parser_record(parser: argparse.ArgumentParser) -> dict[str, Any]:
    arguments: list[dict[str, Any]] = []
    subcommands: dict[str, dict[str, Any]] = {}
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            for name, subparser in sorted(action.choices.items()):
                subcommands[name] = _parser_record(subparser)
            continue
        record = _action_record(action)
        if record is not None:
            arguments.append(record)
    arguments.sort(key=lambda item: (item["dest"], item["flags"]))
    record: dict[str, Any] = {"arguments": arguments}
    if subcommands:
        record["subcommands"] = subcommands
    return record


def cli_surface() -> dict[str, Any]:
    root = _parser_record(_build_entrypoint_parser())
    return root.get("subcommands", {})


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
    commands = cli_surface()
    return {
        "schema": SURFACE_SCHEMA,
        "commands": {
            name: {"tier": declaration["tiers"].get(name), **record}
            for name, record in sorted(commands.items())
        },
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
        if key == "commands":
            continue
        if expected.get(key) != actual.get(key):
            problems.append(f"{key} changed")
    expected_commands = expected.get("commands", {})
    actual_commands = actual.get("commands", {})
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
    problems = declaration_problems(declaration, surface["commands"])
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
        f"commands={len(surface['commands'])} {rendered}"
    )
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
