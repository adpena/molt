#!/usr/bin/env python3
"""Check or rewrite every manifest generator in one interpreter.

``tools/generator_manifest.toml`` is the authority for what is generated, and
every check-mode generator implements the ``tools/generator_io.py`` contract
(``generated_outputs()``). This runner imports each generator once, renders
it and compares or publishes the result, so the repository pays one
interpreter start for all generators instead of one per generator.

``write`` follows ``upstream_generators``, so a generator renders only after
the generators it reads from have published; the proof plan, which hashes
every authority input, always runs last. Generators with
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
import importlib.util
import os
import sys
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
for _import_root in (ROOT, ROOT / "src", ROOT / "tools"):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))

from tools import harness_memory_guard  # noqa: E402
from tools.generator_io import display_path, stale_outputs, write_outputs  # noqa: E402

MANIFEST = ROOT / "tools" / "generator_manifest.toml"
PROOF_PLAN = "tools/gen_proof_plan.py"


@dataclass(frozen=True)
class Generator:
    tool: str
    upstream: tuple[str, ...]
    reproducible: bool


def manifest_generators(manifest: Path = MANIFEST) -> list[Generator]:
    """Check-mode generators in dependency order, the proof plan last."""
    with manifest.open("rb") as handle:
        rows = tomllib.load(handle).get("generator", ())
    generators = {
        str(row["tool"]): Generator(
            tool=str(row["tool"]),
            upstream=tuple(str(item) for item in row.get("upstream_generators", ())),
            reproducible=bool(row.get("ci_checkable", True)),
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
    path = ROOT / tool
    name = "_molt_generator_" + path.stem
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import generator {tool}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    if not callable(getattr(module, "generated_outputs", None)):
        raise SystemExit(f"{tool} does not define generated_outputs()")
    return module


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("action", choices=("check", "write"))
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="TOOL",
        help="limit to this manifest tool (repeatable); includes host-input ones",
    )
    args = parser.parse_args(argv)

    generators = manifest_generators()
    unknown = sorted(set(args.only) - {g.tool for g in generators})
    if unknown:
        parser.error("not check-mode manifest generators: " + ", ".join(unknown))
    if args.only:
        selected = [g for g in generators if g.tool in args.only]
    else:
        selected = [g for g in generators if g.reproducible]
        skipped = [g.tool for g in generators if not g.reproducible]
        if skipped:
            print("host-input generators skipped (use --only): " + ", ".join(skipped))

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
            outputs = load_generator(generator.tool).generated_outputs()
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
    raise SystemExit(main())
