#!/usr/bin/env python3
"""Check or rewrite every manifest generator in one interpreter.

``tools/generator_manifest.toml`` is the authority for what is generated, and
every check-mode generator implements the ``tools/generator_io.py`` contract
(``generated_outputs()``). This runner imports each generator once, renders
it and compares or publishes the result, so the repository pays one
interpreter start for all generators instead of one per generator.

``write`` follows ``upstream_generators``, so a generator renders only after
the generators it reads from have published. Targeted writes also include
every declared downstream consumer; targeted checks examine only the named
generators. The proof plan, which hashes every authority input, is last in the
default full run. Generators with
``ci_checkable = false`` read inputs a checkout cannot reproduce (gitignored
corpora, external checkouts, live interpreters); they run only when named
with ``--only``.

Usage:
    python tools/generators.py check            # exit 1 if any output is stale
    python tools/generators.py write            # rewrite stale outputs
    python tools/generators.py check --only tools/gen_op_kinds.py
"""

from __future__ import annotations

import argparse
from contextlib import AbstractContextManager, ExitStack, nullcontext
import importlib
import inspect
import os
import sys
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


from molt.python_runtime_admission import BuildPythonAdmission  # noqa: E402
from tools import harness_memory_guard  # noqa: E402
from tools.generator_io import display_path, stale_outputs, write_outputs  # noqa: E402
from tools.import_file import tool_module_name  # noqa: E402
from tools.scan_records import ScanRecords, open_scan_records  # noqa: E402
from tools.proof_plan import (  # noqa: E402
    admit_toolchains,
    load_toolchain_policies,
    toolchain_closure,
)

MANIFEST = ROOT / "tools" / "generator_manifest.toml"
PROOF_PLAN = "tools/gen_proof_plan.py"


@dataclass(frozen=True)
class Generator:
    tool: str
    upstream: tuple[str, ...]
    reproducible: bool
    toolchains: tuple[str, ...]


def manifest_generators(manifest: Path = MANIFEST) -> list[Generator]:
    """Check-mode generators in dependency order, the proof plan last."""
    with manifest.open("rb") as handle:
        rows = tomllib.load(handle).get("generator", ())
    generators = {
        str(row["tool"]): Generator(
            tool=str(row["tool"]),
            upstream=tuple(str(item) for item in row.get("upstream_generators", ())),
            reproducible=bool(row.get("ci_checkable", True)),
            toolchains=tuple(row["toolchains"]),
        )
        for row in rows
        if row.get("check_mode") and not row.get("discovery_only")
    }
    order: list[Generator] = []
    placed: set[str] = set()
    pending = sorted(generators.values(), key=lambda g: (g.tool == PROOF_PLAN, g.tool))
    while pending:
        ready = next(
            (
                g
                for g in pending
                if all(u in placed or u not in generators for u in g.upstream)
            ),
            None,
        )
        if ready is None:
            raise SystemExit(
                "generator_manifest upstream_generators form a cycle: "
                + ", ".join(g.tool for g in pending)
            )
        order.append(ready)
        placed.add(ready.tool)
        pending.remove(ready)
    return order


def load_generator(tool: str) -> ModuleType:
    name = tool_module_name(ROOT / tool)
    if name is None:
        raise SystemExit(f"generator {tool} is not a repository tool module")
    module = importlib.import_module(name)
    if not callable(getattr(module, "generated_outputs", None)):
        raise SystemExit(f"{tool} does not define generated_outputs()")
    return module


@dataclass(frozen=True)
class GeneratorAdmission:
    """Admitted generator prerequisites, their runtime owners, and the
    projection their tools need.

    Generators run in the process that admits them and start their tools with
    its environment, so the command that owns the process applies
    ``environment`` to it (``os.environ.update``). Admission itself never
    writes the process environment: a long-lived caller must not carry one
    admission's selections into the next.
    """

    fingerprints: dict[str, dict[str, Any]]
    bindings: dict[str, dict[str, tuple[dict[str, Any], BuildPythonAdmission]]]
    environment: dict[str, str]


def admit_generators(
    selected: list[Generator], *, root: Path, runtime_stack: ExitStack | None = None
) -> GeneratorAdmission:
    """Reject all prerequisites before import; retain demanded runtime owners."""
    if runtime_stack is None:
        with ExitStack() as stack:
            admission = admit_generators(selected, root=root, runtime_stack=stack)
            return GeneratorAdmission(admission.fingerprints, {}, admission.environment)
    names = tuple(dict.fromkeys(name for row in selected for name in row.toolchains))
    with (root / "tools/proof_plan.toml").open("rb") as handle:
        policies = load_toolchain_policies(tomllib.load(handle), root=root, names=names)
    owners: dict[str, list[str]] = {}
    for row in selected:
        for name in toolchain_closure(policies, row.toolchains):
            owners.setdefault(name, []).append(row.tool)
    admissions = {
        policy.name: runtime_stack.enter_context(
            BuildPythonAdmission(with_custody=True)
        )
        for policy in policies
        if policy.python_identity_role == "runtime"
    }
    admitted = admit_toolchains(
        policies,
        names,
        environ=os.environ,
        owners=owners,
        python_admissions=admissions or None,
        root=root,
        python_executable=Path(sys.executable),
    )
    fingerprints = admitted.fingerprints
    bindings: dict[str, dict[str, tuple[dict[str, Any], BuildPythonAdmission]]] = {
        row.tool: {} for row in selected
    }
    for policy in policies:
        if policy.python_identity_role == "runtime":
            for tool in owners[policy.name]:
                bindings[tool][policy.data["python_version"]] = (
                    fingerprints[policy.name]["python_runtime"],
                    admissions[policy.name],
                )
    return GeneratorAdmission(fingerprints, bindings, admitted.environment)


def _generator_records(
    module: ModuleType, *, enabled: bool
) -> AbstractContextManager[ScanRecords | None]:
    """Scan records for a generator whose ``generated_outputs`` declares them.

    The runner is the command entry that owns them, as each scanner's CLI is.
    """
    accepts = "scan_records" in inspect.signature(module.generated_outputs).parameters
    if not (enabled and accepts):
        return nullcontext(None)
    source = Path(module.__file__ or "")
    return open_scan_records(f"generator_{source.stem}", owner=source, enabled=True)


def main(argv: list[str] | None = None, *, scan_records: bool = False) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("action", choices=("check", "write"))
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="TOOL",
        help="check only named tools; write named tools and declared consumers "
        "(repeatable); host-input tools must be named explicitly",
    )
    args = parser.parse_args(argv)

    generators = manifest_generators()
    unknown = sorted(set(args.only) - {g.tool for g in generators})
    if unknown:
        parser.error("not check-mode manifest generators: " + ", ".join(unknown))
    if args.only:
        pending = list(dict.fromkeys(args.only))
        requested = set(pending)
        selected_names = requested.copy()
        if args.action == "write":
            consumers: dict[str, list[str]] = {}
            for generator in generators:
                for upstream in generator.upstream:
                    consumers.setdefault(upstream, []).append(generator.tool)
            for tool in pending:
                for consumer in consumers.get(tool, ()):
                    if consumer not in selected_names:
                        selected_names.add(consumer)
                        pending.append(consumer)
            host_inputs = [
                generator.tool
                for generator in generators
                if generator.tool in selected_names
                and not generator.reproducible
                and generator.tool not in requested
            ]
            if host_inputs:
                parser.error(
                    "downstream host-input generators require explicit --only: "
                    + ", ".join(host_inputs)
                )
        selected = [g for g in generators if g.tool in selected_names]
    else:
        selected = [g for g in generators if g.reproducible]
        skipped = [g.tool for g in generators if not g.reproducible]
        if skipped:
            print("host-input generators skipped (use --only): " + ", ".join(skipped))

    with ExitStack() as runtime_stack:
        try:
            admission = admit_generators(
                selected, root=ROOT, runtime_stack=runtime_stack
            )
        except (ValueError, OSError, KeyError) as exc:
            print(f"generator prerequisite admission failed: {exc}", file=sys.stderr)
            return 1
        # This command owns its process; generators start their tools with it.
        os.environ.update(admission.environment)
        runtime_bindings = admission.bindings

        stale_count = 0
        started = time.perf_counter()
        # One repository sentinel watches the whole run. Every guarded child the
        # generators start (rustfmt, mostly) would otherwise open and scan its own
        # sentinel, which cost ~2.4 s per call against milliseconds of formatting.
        with harness_memory_guard.repo_process_sentinel(
            repo_root=ROOT,
            # The same root the per-command automatic sentinel would use.
            artifact_root=harness_memory_guard._artifact_root_from_env(os.environ),
            label="generators",
            limits=harness_memory_guard.limits_from_env("MOLT_GENERATOR", os.environ),
            drain_on_exit=False,
        ):
            for generator in selected:
                tick = time.perf_counter()
                module = load_generator(generator.tool)
                bindings = runtime_bindings[generator.tool]
                arguments: dict[str, Any] = {}
                if bindings:
                    arguments["python_runtimes"] = bindings
                with _generator_records(module, enabled=scan_records) as records:
                    if records is not None:
                        arguments["scan_records"] = records
                    outputs = module.generated_outputs(**arguments)
                if args.action == "check":
                    stale = stale_outputs(outputs)
                    for path in stale:
                        print(
                            f"stale: {display_path(path)} ({generator.tool})",
                            file=sys.stderr,
                        )
                    stale_count += len(stale)
                else:
                    for path in write_outputs(outputs):
                        print(f"wrote {display_path(path)} ({generator.tool})")
                print(f"  {time.perf_counter() - tick:6.2f}s {generator.tool}")
        elapsed = time.perf_counter() - started
        if stale_count:
            print(
                f"{stale_count} stale output(s); run `python3 tools/generators.py write`",
                file=sys.stderr,
            )
            return 1
        verb = "current" if args.action == "check" else "written"
        print(f"{len(selected)} generator(s) {verb} in {elapsed:.1f}s")
        return 0


if __name__ == "__main__":
    raise SystemExit(main(scan_records=True))
