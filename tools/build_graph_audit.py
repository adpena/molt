#!/usr/bin/env python3
"""Recompile-blast-radius ratchet  -  the crate-cut enforcement fact (doc 56 FACT-A).

The decomposition program (21b/21f) cut the monolith into a layered crate DAG so
that editing one layer does not recompile unrelated layers (dx_baseline sections 4-5:
"only a CRATE split buys build-cache isolation"). That win **regresses silently**
the moment someone adds a cross-crate dependency that re-couples two layers  -  a
new `use` from a backend into the IR vocabulary, or a sibling backend depending
on another. The 21b DAG is a *design*; nothing enforced it until this tool.

This is the machine-checked enforcement, the build-graph dual of
`tools/gen_op_kinds.py --check` (which makes a missing opcode classifier
*unexpressible*). It reads two FACTS  -  never a compile:

  * the DECLARED DAG: `runtime/crate_graph.toml` (each workspace crate's layer +
    the documented same-layer exceptions), and
  * the MEASURED graph: `cargo metadata --no-deps` over the ROOT workspace.

and computes, per crate, its **recompile blast radius** = the number of
workspace crates that transitively depend on it (its reverse-dependency cone).
When crate C's source changes, cargo must recompile C and every crate in that
cone, so the cone size IS the blast radius  -  derivable with zero compiles.

It then enforces two ratchet metrics (mirrors `structural_audit.py`'s
`--check`/`--write-board`/`--update-baseline` shape, so it composes with the
same CI machinery):

  * ``crate_layer_backedges``  -  dependency edges that violate the declared layer
    ordering (a crate depending on something at a higher-or-equal layer that is
    not a whitelisted same-layer exception). Must stay **0**: a new layering
    violation names the offending edge and fails CI.
  * ``max_crate_blast_radius`` plus a per-crate ``blast_radius`` baseline  -  a new
    edge that widens ANY crate's downstream cone beyond its recorded baseline
    fails CI, locking in the decomposition wins (a cut may only SHRINK radii).

Modes (mirrors tools/structural_audit.py / tools/gen_op_kinds.py CI convention):
  build_graph_audit.py                  human-readable board (stdout)
  build_graph_audit.py --json           machine-readable graph + metrics (stdout)
  build_graph_audit.py --write-board    regenerate docs/design/foundation/BUILD_GRAPH_BOARD.md
  build_graph_audit.py --check          exit 1 if a back-edge appears or any blast radius widened
  build_graph_audit.py --update-baseline  re-pin tools/build_graph_baseline.json

Wired into tools/ci_gate.py (tier 1) next to structural-audit-ratchet.

WHY `cargo metadata` and not `cargo build --timings`/`--build-plan`: the reverse
transitive dependency closure over the crate graph IS the lib-recompile set
(cargo rebuilds a crate and everything downstream of it when its source
changes). `cargo metadata` is the STABLE machine fact for that graph; it runs in
well under a second and never compiles. `--timings`/`--build-plan` would add a
full compile and an unstable-JSON dependency for no extra signal  -  exactly the
profile-inflation this project rejects (doc 56 sec 6 risk row 1).
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from collections import defaultdict, deque
from dataclasses import dataclass, field
from pathlib import Path

ROOT_DEFAULT = Path(__file__).resolve().parents[1]
GRAPH_TOML_REL = "runtime/crate_graph.toml"
BASELINE_PATH_REL = "tools/build_graph_baseline.json"
BOARD_PATH_REL = "docs/design/foundation/BUILD_GRAPH_BOARD.md"

# Dependency kinds, as reported per-dependency by `cargo metadata --no-deps`.
# A null kind is a normal (production) dependency; "build" is a build-script
# dependency. Both put the dependency in the depending crate's LIB recompile
# cone. "dev" is a dev-dependency: it only rebuilds the dependent's TEST target,
# never its lib, so it is excluded from the blast radius  -  but it is still
# subject to the layering rule (a test-only edge must never become the seed of a
# production cycle; 21f risk register "Re-introducing a passes<->lowering CYCLE
# via the test-only refs").
_LIB_CONE_KINDS = frozenset({"normal", "build"})


class BuildGraphError(RuntimeError):
    """Fail-loud condition: the declared graph and the measured graph disagree
    in a way that invalidates the ratchet (never silently pass)."""


# ---------------------------------------------------------------------------
# Declared graph (crate_graph.toml)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class DeclaredCrate:
    name: str
    layer: int
    role: str


@dataclass(frozen=True)
class DeclaredGraph:
    crates: dict[str, DeclaredCrate]
    # set of (from, to) same-layer edges that are explicitly permitted.
    allowed_same_layer_edges: frozenset[tuple[str, str]]

    def layer_of(self, name: str) -> int | None:
        crate = self.crates.get(name)
        return crate.layer if crate is not None else None


def load_declared_graph(root: Path) -> DeclaredGraph:
    path = root / GRAPH_TOML_REL
    if not path.is_file():
        raise BuildGraphError(
            f"declared crate graph not found at {path}; author it (doc 56 FACT-A)"
        )
    with path.open("rb") as fh:
        data = tomllib.load(fh)

    raw_crates = data.get("crate", [])
    if not isinstance(raw_crates, list) or not raw_crates:
        raise BuildGraphError(f"{GRAPH_TOML_REL}: no [[crate]] entries declared")

    crates: dict[str, DeclaredCrate] = {}
    for entry in raw_crates:
        name = entry.get("name")
        layer = entry.get("layer")
        if not isinstance(name, str) or not name:
            raise BuildGraphError(f"{GRAPH_TOML_REL}: [[crate]] missing string name")
        if not isinstance(layer, int) or isinstance(layer, bool) or layer < 0:
            raise BuildGraphError(
                f"{GRAPH_TOML_REL}: crate {name!r} has invalid layer {layer!r} "
                "(expected a non-negative integer)"
            )
        if name in crates:
            raise BuildGraphError(f"{GRAPH_TOML_REL}: duplicate crate {name!r}")
        role = entry.get("role", "")
        crates[name] = DeclaredCrate(
            name=name, layer=layer, role=role if isinstance(role, str) else ""
        )

    allowed: set[tuple[str, str]] = set()
    for entry in data.get("allowed_same_layer_edges", []):
        src = entry.get("from")
        dst = entry.get("to")
        if not isinstance(src, str) or not isinstance(dst, str):
            raise BuildGraphError(
                f"{GRAPH_TOML_REL}: [[allowed_same_layer_edges]] needs string from/to"
            )
        allowed.add((src, dst))

    return DeclaredGraph(crates=crates, allowed_same_layer_edges=frozenset(allowed))


# ---------------------------------------------------------------------------
# Measured graph (cargo metadata)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Edge:
    src: str
    dst: str
    kind: str  # "normal" | "build" | "dev"

    @property
    def in_lib_cone(self) -> bool:
        return self.kind in _LIB_CONE_KINDS


@dataclass(frozen=True)
class MeasuredGraph:
    crates: frozenset[str]
    edges: tuple[Edge, ...]


def _normalize_kind(raw: object) -> str:
    # cargo metadata reports a dependency's kind as null (normal), "build", or
    # "dev". Be strict: an unexpected value is a schema change we must not paper
    # over (doc 56 sec 6 risk row 1  -  fail loud, never silently pass).
    if raw is None:
        return "normal"
    if raw in ("build", "dev", "normal"):
        return str(raw)
    raise BuildGraphError(f"cargo metadata: unexpected dependency kind {raw!r}")


def run_cargo_metadata(root: Path) -> dict:
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=str(root),
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as exc:  # pragma: no cover - environment-specific
        raise BuildGraphError("cargo not found on PATH") from exc
    if proc.returncode != 0:
        raise BuildGraphError(
            "cargo metadata failed (rc="
            f"{proc.returncode}):\n{proc.stderr.strip()[-2000:]}"
        )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise BuildGraphError(f"cargo metadata emitted non-JSON: {exc}") from exc


def parse_measured_graph(metadata: dict) -> MeasuredGraph:
    packages = {pkg["id"]: pkg for pkg in metadata.get("packages", [])}
    member_ids = set(metadata.get("workspace_members", []))
    ws_names = {packages[pid]["name"] for pid in member_ids if pid in packages}
    if not ws_names:
        raise BuildGraphError("cargo metadata reported no workspace members")

    edges: list[Edge] = []
    seen: set[tuple[str, str, str]] = set()
    for pid in member_ids:
        pkg = packages.get(pid)
        if pkg is None:
            continue
        src = pkg["name"]
        for dep in pkg.get("dependencies", []):
            dst = dep.get("name")
            if dst not in ws_names or dst == src:
                continue
            kind = _normalize_kind(dep.get("kind"))
            key = (src, dst, kind)
            if key in seen:
                continue
            seen.add(key)
            edges.append(Edge(src=src, dst=dst, kind=kind))
    return MeasuredGraph(crates=frozenset(ws_names), edges=tuple(edges))


# ---------------------------------------------------------------------------
# Blast radius + layering analysis
# ---------------------------------------------------------------------------


@dataclass
class BackEdge:
    src: str
    dst: str
    src_layer: int
    dst_layer: int
    kind: str

    def describe(self) -> str:
        rel = "==" if self.src_layer == self.dst_layer else "<"
        return (
            f"{self.src} (L{self.src_layer}) -> {self.dst} (L{self.dst_layer}) "
            f"[{self.kind}]  (layer {self.src_layer} {rel} {self.dst_layer}: "
            "a crate may depend only on STRICTLY LOWER layers)"
        )


@dataclass
class GraphReport:
    declared: DeclaredGraph
    measured: MeasuredGraph
    # crate -> recompile blast radius (count of crates transitively depending on it)
    blast_radius: dict[str, int]
    # crate -> sorted list of downstream crate names (the cone), for the board
    downstream: dict[str, list[str]]
    back_edges: list[BackEdge]
    # crates present in metadata but not declared, and vice versa.
    undeclared_crates: list[str] = field(default_factory=list)
    stale_declarations: list[str] = field(default_factory=list)


def _reverse_lib_cone(measured: MeasuredGraph) -> dict[str, set[str]]:
    """For each crate, the set of crates that transitively depend on it through
    LIB-cone (normal/build) edges. This is the recompile blast radius set."""
    # forward[src] = {dst it depends on}; reverse[dst] = {src that depends on it}
    reverse: dict[str, set[str]] = defaultdict(set)
    for edge in measured.edges:
        if edge.in_lib_cone:
            reverse[edge.dst].add(edge.src)

    cones: dict[str, set[str]] = {}
    for crate in measured.crates:
        seen: set[str] = set()
        queue: deque[str] = deque([crate])
        while queue:
            node = queue.popleft()
            for dependent in reverse.get(node, ()):
                if dependent not in seen:
                    seen.add(dependent)
                    queue.append(dependent)
        cones[crate] = seen
    return cones


def analyze(declared: DeclaredGraph, measured: MeasuredGraph) -> GraphReport:
    declared_names = set(declared.crates)
    measured_names = set(measured.crates)
    undeclared = sorted(measured_names - declared_names)
    stale = sorted(declared_names - measured_names)

    cones = _reverse_lib_cone(measured)
    blast_radius = {crate: len(cone) for crate, cone in cones.items()}
    downstream = {crate: sorted(cone) for crate, cone in cones.items()}

    back_edges: list[BackEdge] = []
    for edge in measured.edges:
        src_layer = declared.layer_of(edge.src)
        dst_layer = declared.layer_of(edge.dst)
        if src_layer is None or dst_layer is None:
            # An undeclared crate is reported separately and fails --check; do
            # not also flag every one of its edges as a back-edge (noise).
            continue
        # Legal: depend STRICTLY downward (src layer strictly greater than dst).
        if src_layer > dst_layer:
            continue
        # Same-layer edge: legal only if explicitly whitelisted.
        if (
            src_layer == dst_layer
            and (edge.src, edge.dst) in declared.allowed_same_layer_edges
        ):
            continue
        back_edges.append(
            BackEdge(
                src=edge.src,
                dst=edge.dst,
                src_layer=src_layer,
                dst_layer=dst_layer,
                kind=edge.kind,
            )
        )
    back_edges.sort(key=lambda be: (be.src_layer, be.src, be.dst))

    return GraphReport(
        declared=declared,
        measured=measured,
        blast_radius=blast_radius,
        downstream=downstream,
        back_edges=back_edges,
        undeclared_crates=undeclared,
        stale_declarations=stale,
    )


# ---------------------------------------------------------------------------
# Ratchet metrics + baseline
# ---------------------------------------------------------------------------


def ratchet_metrics(report: GraphReport) -> dict[str, float]:
    """Aggregate scalars that may only improve (decrease). CI fails on regress.

    `crate_layer_backedges` is the structural-containment metric (a new layering
    violation); `max_crate_blast_radius` is the global cone-width metric. The
    per-crate radii live in the baseline's `blast_radius` map (checked
    separately) so a single crate widening is caught even when it does not beat
    the global maximum."""
    return {
        "crate_layer_backedges": float(len(report.back_edges)),
        "max_crate_blast_radius": float(max(report.blast_radius.values(), default=0)),
        # An undeclared workspace crate means the declared DAG is incomplete, so
        # its layering cannot be enforced  -  track it as a ratchet metric too.
        "undeclared_crates": float(len(report.undeclared_crates)),
    }


def baseline_payload(report: GraphReport) -> dict:
    """The full pinned fact: scalar ratchet metrics + the per-crate radius map.

    Storing per-crate radii (not just the max) is what makes a re-coupling that
    widens a non-maximal crate's cone detectable: e.g. coupling two backends
    raises a backend's radius from 1 without touching the global max of 29."""
    return {
        "metrics": ratchet_metrics(report),
        "blast_radius": dict(sorted(report.blast_radius.items())),
    }


# Metrics where a HIGHER value is worse (ratchet direction is "down").
_RATCHET_DOWN = ("crate_layer_backedges", "max_crate_blast_radius", "undeclared_crates")


@dataclass
class CheckOutcome:
    ok: bool
    regressions: list[str]
    improvements: list[str]


def check_against_baseline(report: GraphReport, baseline: dict) -> CheckOutcome:
    metrics = ratchet_metrics(report)
    base_metrics = baseline.get("metrics", {})
    base_radius = baseline.get("blast_radius", {})

    regressions: list[str] = []
    improvements: list[str] = []

    # 1. A back-edge is always a hard failure, with the offending edge named.
    for back_edge in report.back_edges:
        regressions.append(f"LAYER BACK-EDGE: {back_edge.describe()}")

    # 2. An undeclared workspace crate means the DAG is unenforceable for it.
    for crate in report.undeclared_crates:
        regressions.append(
            f"UNDECLARED CRATE: {crate} is in the workspace but not in "
            f"{GRAPH_TOML_REL}; declare its layer so its cuts are enforced"
        )

    # 3. Scalar ratchet metrics may only go down.
    for key in _RATCHET_DOWN:
        cur = metrics.get(key, 0.0)
        base = float(base_metrics.get(key, 0.0))
        if cur > base:
            regressions.append(
                f"RATCHET REGRESSED: {key}: {base:g} -> {cur:g} (must not increase)"
            )
        elif cur < base:
            improvements.append(f"{key}: {base:g} -> {cur:g}")

    # 4. Per-crate blast radius may only go down (locks in each decomposition
    #    win individually  -  the key anti-re-coupling check).
    for crate, radius in sorted(report.blast_radius.items()):
        base = base_radius.get(crate)
        if base is None:
            continue  # newly added crate handled by undeclared/metrics paths
        if radius > base:
            regressions.append(
                f"BLAST RADIUS WIDENED: {crate}: {int(base)} -> {radius} "
                "downstream crates (a new edge re-coupled this crate's cone)"
            )
        elif radius < base:
            improvements.append(f"radius[{crate}]: {int(base)} -> {radius}")

    return CheckOutcome(
        ok=not regressions, regressions=regressions, improvements=improvements
    )


# ---------------------------------------------------------------------------
# Board rendering
# ---------------------------------------------------------------------------


def format_board(report: GraphReport) -> str:
    metrics = ratchet_metrics(report)
    declared = report.declared
    radii = report.blast_radius

    lines = [
        "<!-- @generated by tools/build_graph_audit.py --write-board. DO NOT EDIT. -->",
        "# Build-graph board  -  the recompile blast-radius ratchet",
        "",
        "Product board for the crate-cut enforcement fact (doc 56 FACT-A / "
        "Phase 1a). Generated by `tools/build_graph_audit.py` from "
        "`runtime/crate_graph.toml` (the declared DAG) + `cargo metadata` (the "
        "measured graph)  -  NO compile. The `--check` ratchet (CI) fails if a "
        "layer back-edge appears or any crate's recompile blast radius widens "
        "beyond the recorded baseline, so the 21b/21f decomposition wins cannot "
        "silently regress.",
        "",
        "> **Blast radius** of crate C = the number of workspace crates that "
        "transitively depend on C through normal/build edges. When C's source "
        "changes, cargo recompiles C and that entire downstream cone, so the "
        "cone size IS the recompile blast radius. Dev-dependencies are excluded "
        "from the cone (a dev edge rebuilds only the dependent's test target) "
        "but are still held to the layer ordering.",
        "",
        "## Ratchet metrics (may only go DOWN)",
        "",
        "| metric | value |",
        "| --- | --- |",
    ]
    for key in _RATCHET_DOWN:
        value = metrics.get(key, 0.0)
        lines.append(f"| {key} | {int(value) if value == int(value) else value} |")
    lines.append("")

    # Layer back-edges (the layering-violation surface).
    lines.append(f"## Layer back-edges ({len(report.back_edges)})")
    lines.append("")
    if report.back_edges:
        lines.append("| offending edge |")
        lines.append("| --- |")
        for back_edge in report.back_edges:
            lines.append(f"| `{back_edge.describe()}` |")
    else:
        lines.append(
            "None  -  every dependency edge respects the declared layer ordering."
        )
    lines.append("")

    # Undeclared / stale reconciliation.
    if report.undeclared_crates or report.stale_declarations:
        lines.append("## Declaration reconciliation")
        lines.append("")
        if report.undeclared_crates:
            lines.append(
                "**Undeclared workspace crates** (add to `crate_graph.toml`): "
                + ", ".join(f"`{c}`" for c in report.undeclared_crates)
            )
            lines.append("")
        if report.stale_declarations:
            lines.append(
                "**Stale declarations** (in `crate_graph.toml` but not the "
                "workspace): " + ", ".join(f"`{c}`" for c in report.stale_declarations)
            )
            lines.append("")

    # Blast radius ranked (the headline DX-1 fact).
    lines.append("## Recompile blast radius (ranked)")
    lines.append("")
    lines.append("| radius | crate | layer | downstream cone (first 6) |")
    lines.append("| ---: | --- | ---: | --- |")
    for crate in sorted(radii, key=lambda c: (-radii[c], c)):
        layer = declared.layer_of(crate)
        layer_str = "?" if layer is None else f"L{layer}"
        cone = report.downstream.get(crate, [])
        shown = ", ".join(cone[:6]) + ("..." if len(cone) > 6 else "")
        lines.append(f"| {radii[crate]} | `{crate}` | {layer_str} | {shown} |")
    lines.append("")

    # Layer map (the declared DAG, for review of intentional edges).
    lines.append("## Declared layer map (`runtime/crate_graph.toml`)")
    lines.append("")
    by_layer: dict[int, list[DeclaredCrate]] = defaultdict(list)
    for crate in declared.crates.values():
        by_layer[crate.layer].append(crate)
    for layer in sorted(by_layer):
        names = ", ".join(
            f"`{c.name}`" for c in sorted(by_layer[layer], key=lambda c: c.name)
        )
        lines.append(f"- **Layer {layer}:** {names}")
    lines.append("")
    if declared.allowed_same_layer_edges:
        lines.append("**Allowed same-layer edges:** ")
        for src, dst in sorted(declared.allowed_same_layer_edges):
            lines.append(f"- `{src}` -> `{dst}`")
        lines.append("")

    return "\n".join(lines).rstrip("\n")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def build_report(root: Path) -> GraphReport:
    declared = load_declared_graph(root)
    metadata = run_cargo_metadata(root)
    measured = parse_measured_graph(metadata)
    return analyze(declared, measured)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--root",
        type=Path,
        default=ROOT_DEFAULT,
        help="repo root to audit (default: this tool's repo)",
    )
    ap.add_argument("--json", action="store_true", help="emit machine-readable graph")
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if a back-edge appears or any blast radius regressed",
    )
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="re-pin tools/build_graph_baseline.json to current facts",
    )
    ap.add_argument(
        "--write-board",
        action="store_true",
        help="regenerate docs/design/foundation/BUILD_GRAPH_BOARD.md",
    )
    args = ap.parse_args(argv)

    root: Path = args.root.resolve()
    try:
        report = build_report(root)
    except BuildGraphError as exc:
        print(f"build_graph_audit: {exc}", file=sys.stderr)
        return 2

    baseline_path = root / BASELINE_PATH_REL
    wrote_artifact = False

    if args.update_baseline:
        baseline_path.parent.mkdir(parents=True, exist_ok=True)
        baseline_path.write_text(
            json.dumps(baseline_payload(report), indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(f"baseline updated: {baseline_path}")
        wrote_artifact = True

    if args.write_board:
        board_path = root / BOARD_PATH_REL
        board_path.parent.mkdir(parents=True, exist_ok=True)
        board_path.write_text(format_board(report) + "\n", encoding="utf-8")
        print(f"board written: {board_path}")
        wrote_artifact = True

    if wrote_artifact and not args.json and not args.check:
        return 0

    if args.json:
        print(
            json.dumps(
                {
                    "metrics": ratchet_metrics(report),
                    "blast_radius": dict(sorted(report.blast_radius.items())),
                    "back_edges": [
                        {
                            "src": be.src,
                            "dst": be.dst,
                            "src_layer": be.src_layer,
                            "dst_layer": be.dst_layer,
                            "kind": be.kind,
                        }
                        for be in report.back_edges
                    ],
                    "undeclared_crates": report.undeclared_crates,
                    "stale_declarations": report.stale_declarations,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    if args.check:
        if not baseline_path.is_file():
            print(
                f"ERROR: no baseline at {baseline_path}; run --update-baseline",
                file=sys.stderr,
            )
            return 2
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        outcome = check_against_baseline(report, baseline)
        if not outcome.ok:
            print(
                "BUILD-GRAPH RATCHET REGRESSED  -  the crate-cut decomposition "
                "re-coupled:",
                file=sys.stderr,
            )
            for regression in outcome.regressions:
                print(f"  {regression}", file=sys.stderr)
            print(
                "Remove the offending cross-crate dependency, or if the coupling "
                f"is intentional, justify it in {GRAPH_TOML_REL} (move a layer / "
                "add an allowed_same_layer_edge) and re-pin with --update-baseline.",
                file=sys.stderr,
            )
            return 1
        print(
            f"build-graph ratchet OK ({len(report.measured.crates)} crates; "
            f"max blast radius {int(max(report.blast_radius.values(), default=0))}; "
            f"{len(outcome.improvements)} metric(s) improved)"
        )
        return 0

    # default: human board to stdout
    print(format_board(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
