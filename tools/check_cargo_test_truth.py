#!/usr/bin/env python3
"""Fail closed when Cargo test binaries can be masked or silently unexecuted.

A ``[[test]]`` target with ``required-features`` builds only when a Cargo
invocation enables those features for its package; the canonical workspace
run uses default features and skips it silently (HF-565). Every such target
must be run by a plan ``cargo test`` invocation that enables its features, or
sit in ``tools/cargo_gated_test_targets_baseline.json`` under the open finding
that owns joining it. The baseline may only shrink.
"""

from __future__ import annotations

import json
import shlex
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT / "src") not in sys.path:
    sys.path.insert(0, str(ROOT / "src"))

from molt.cargo_workspace import workspace_member_packages  # noqa: E402

PROOF_PLAN = ROOT / "tools" / "proof_plan.toml"
GATED_TARGET_BASELINE = ROOT / "tools" / "cargo_gated_test_targets_baseline.json"
_BASELINE_SCHEMA = 1
RUNNER = ROOT / "tools" / "run_cargo_test_truth.py"
_CANONICAL = "cargo test --locked --workspace --tests --no-fail-fast"
CANONICAL_ARGUMENTS = ("--locked", "--workspace", "--tests", "--no-fail-fast")
_RUNNER_ID = "rust.test.default-truth"
_RUNNER_ARGV = ["uv", "run", "--frozen", "python3", "tools/run_cargo_test_truth.py"]
_CARGO_EXECUTABLES = frozenset({"cargo", "cargo.exe"})
_COMPILE_SUBCOMMANDS = frozenset({"build", "check", "clippy", "test"})
# Each selects exactly one test executable when it appears once.
_SINGLE_TARGET_SELECTORS = frozenset({"--lib", "--doc", "--bin", "--test", "--bench"})
_MULTI_TARGET_SELECTORS = frozenset(
    {"--workspace", "--all", "--all-targets", "--tests", "--bins", "--benches"}
)


def _cargo_arguments(argv: Sequence[str]) -> tuple[str, list[str]] | None:
    """Return (subcommand, cargo's own arguments) for a Cargo compile invocation.

    Arguments after `--` belong to the compiled binaries, not to Cargo.
    """
    for index, token in enumerate(argv):
        if Path(token).name not in _CARGO_EXECUTABLES:
            continue
        rest = list(argv[index + 1 :])
        while rest and rest[0].startswith("+"):
            rest.pop(0)
        if not rest or rest[0] not in _COMPILE_SUBCOMMANDS:
            return None
        subcommand, arguments = rest[0], rest[1:]
        if "--" in arguments:
            arguments = arguments[: arguments.index("--")]
        return subcommand, arguments
    return None


def _cargo_invocations(
    plan: Mapping[str, object],
) -> Iterator[tuple[str, str, list[str]]]:
    """Yield (owner, subcommand, arguments) for every Cargo compile in the plan.

    Commands carry `argv` lists; local rules carry shell `gates` strings. The
    TOML is parsed, so comments and prose never count as commands.
    """
    for table_name, entries in plan.items():
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            owner = f"{table_name} {entry.get('id') or entry.get('name')!r}"
            argv = entry.get("argv")
            if isinstance(argv, list):
                found = _cargo_arguments([str(item) for item in argv])
                if found is not None:
                    yield owner, *found
            for gate in entry.get("gates") or ():
                found = _cargo_arguments(shlex.split(str(gate)))
                if found is not None:
                    yield owner, *found


def _selects_one_executable(arguments: Sequence[str]) -> bool:
    if any(argument in _MULTI_TARGET_SELECTORS for argument in arguments):
        return False
    packages = sum(argument in {"-p", "--package"} for argument in arguments)
    selectors = sum(argument in _SINGLE_TARGET_SELECTORS for argument in arguments)
    return packages <= 1 and selectors == 1


# Cargo target selectors; with none, ``cargo test`` runs every test target.
_TARGET_SELECTORS = frozenset(
    {
        "--lib",
        "--bin",
        "--bins",
        "--example",
        "--examples",
        "--test",
        "--tests",
        "--bench",
        "--benches",
        "--doc",
        "--all-targets",
    }
)


def _option_values(arguments: Sequence[str], *names: str) -> list[str]:
    values = []
    for index, argument in enumerate(arguments):
        for name in names:
            if argument == name and index + 1 < len(arguments):
                values.append(arguments[index + 1])
            elif argument.startswith(name + "="):
                values.append(argument.partition("=")[2])
    return values


def gated_test_targets(root: Path) -> dict[str, tuple[str, frozenset[str]]]:
    """``package::target`` -> (package, required features) of gated test targets."""
    targets: dict[str, tuple[str, frozenset[str]]] = {}
    for package, manifest in workspace_member_packages(root).items():
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for target in data.get("test", ()):
            required = target.get("required-features") or ()
            if required:
                targets[f"{package}::{target['name']}"] = (package, frozenset(required))
    return targets


def _features(root: Path) -> dict[str, dict[str, tuple[str, ...]]]:
    tables = {}
    for package, manifest in workspace_member_packages(root).items():
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        tables[package] = {
            str(name): tuple(entries)
            for name, entries in data.get("features", {}).items()
        }
    return tables


def _closure(table: Mapping[str, tuple[str, ...]], enabled: set[str]) -> set[str]:
    """The package's own features that ``enabled`` turns on (Cargo's rules)."""
    pending = list(enabled)
    reached: set[str] = set()
    while pending:
        feature = pending.pop()
        if feature in reached:
            continue
        reached.add(feature)
        for entry in table.get(feature, ()):
            if entry.startswith("dep:"):
                continue
            name = entry.partition("/")[0].rstrip("?")
            if "/" not in entry or (
                name in table and not entry.partition("/")[0].endswith("?")
            ):
                pending.append(name)
    return reached


def _runs(
    arguments: Sequence[str],
    package: str,
    target: str,
    packages: Sequence[str],
    features: Mapping[str, Mapping[str, tuple[str, ...]]],
) -> frozenset[str] | None:
    """The features a ``cargo test`` invocation enables for ``target``, if it runs it."""
    if "--no-run" in arguments:
        return None
    selected = _option_values(arguments, "-p", "--package")
    if selected and package not in selected:
        return None
    if not selected and package not in packages:
        return None
    if any(argument in _TARGET_SELECTORS for argument in arguments) and not (
        "--tests" in arguments
        or "--all-targets" in arguments
        or target in _option_values(arguments, "--test")
    ):
        return None
    table = features.get(package, {})
    if "--all-features" in arguments:
        return frozenset(table)
    enabled = set() if "--no-default-features" in arguments else {"default"}
    for value in _option_values(arguments, "--features", "-F"):
        for feature in value.replace(",", " ").split():
            owner, slash, name = feature.rpartition("/")
            if not slash:
                enabled.add(name)
            elif owner == package:
                enabled.add(name)
    return frozenset(_closure(table, {name for name in enabled if name in table}))


def unrun_gated_targets(
    plan: Mapping[str, object], root: Path = ROOT
) -> dict[str, frozenset[str]]:
    """Gated test targets that no plan ``cargo test`` runs with their features."""
    targets = gated_test_targets(root)
    features = _features(root)
    packages = tuple(features)
    invocations = [
        arguments
        for _owner, subcommand, arguments in _cargo_invocations(plan)
        if subcommand == "test"
    ]
    # The truth runner executes the canonical workspace run with defaults.
    invocations.append(list(CANONICAL_ARGUMENTS))
    unrun = {}
    for identity, (package, required) in targets.items():
        name = identity.partition("::")[2]
        if not any(
            (enabled := _runs(arguments, package, name, packages, features)) is not None
            and required <= enabled
            for arguments in invocations
        ):
            unrun[identity] = required
    return unrun


def gated_target_violations(plan: Mapping[str, object], root: Path = ROOT) -> list[str]:
    unrun = unrun_gated_targets(plan, root)
    baseline_path = root / GATED_TARGET_BASELINE.relative_to(ROOT)
    payload = json.loads(baseline_path.read_text(encoding="utf-8"))
    if payload.get("schema") != _BASELINE_SCHEMA:
        return [f"{_display_path(baseline_path)}: unsupported schema"]
    baseline = payload.get("unrun", {})
    failures = [
        f"{identity}: no proof cell runs this [[test]] target with its "
        f"required features {sorted(required)}; join it to a cell's cargo test"
        for identity, required in sorted(unrun.items())
        if identity not in baseline
    ]
    failures.extend(
        f"{identity}: now run with its features (or removed); delete it from "
        f"{_display_path(baseline_path)}"
        for identity in sorted(set(baseline) - set(unrun))
    )
    failures.extend(
        f"{identity}: a baseline entry must name its owning finding"
        for identity, entry in sorted(baseline.items())
        if not isinstance(entry, dict)
        or not str(entry.get("finding", "")).startswith("HF-")
    )
    return failures


def _display_path(path: Path) -> Path:
    try:
        return path.relative_to(ROOT)
    except ValueError:
        return path


def violations() -> list[str]:
    failures = []
    plan = tomllib.loads(PROOF_PLAN.read_text(encoding="utf-8"))
    runner_commands = [
        command
        for command in plan.get("command", [])
        if command.get("id") == _RUNNER_ID
    ]
    if len(runner_commands) != 1 or runner_commands[0].get("argv") != _RUNNER_ARGV:
        failures.append(
            f"{_display_path(PROOF_PLAN)} must contain exactly one {_RUNNER_ID!r} "
            f"command with argv {_RUNNER_ARGV!r}"
        )
    if (
        RUNNER.read_text(encoding="utf-8").count(
            '"test",\n    "--locked",\n    "--workspace",\n    "--tests",\n    "--no-fail-fast",'
        )
        != 1
    ):
        failures.append(
            f"{RUNNER.relative_to(ROOT)} must execute exactly {_CANONICAL!r}"
        )
    for owner, subcommand, arguments in _cargo_invocations(plan):
        command = " ".join(["cargo", subcommand, *arguments])
        if "--locked" not in arguments:
            failures.append(
                f"{_display_path(PROOF_PLAN)}: {owner}: Cargo compilation "
                f"command lacks --locked dependency authority: {command}"
            )
        if (
            subcommand == "test"
            and not _selects_one_executable(arguments)
            and "--no-fail-fast" not in arguments
        ):
            failures.append(
                f"{_display_path(PROOF_PLAN)}: {owner}: multi-executable Cargo "
                f"test command lacks --no-fail-fast: {command}"
            )
    return failures


def main() -> int:
    plan = tomllib.loads(PROOF_PLAN.read_text(encoding="utf-8"))
    failures = violations() + gated_target_violations(plan)
    if failures:
        print("cargo-test-truth: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("cargo-test-truth: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
