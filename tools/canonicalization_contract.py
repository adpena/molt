#!/usr/bin/env python3
"""Crate-layer canonicalization contract -- a machine-checkable schema for how
Molt's crates are organized, mirroring CPython's layer axis optimized for Rust's
crate / incremental-compile model.

WHY THIS EXISTS (Lattner thesis): an architecture that lives only in prose drifts.
Encode it as a contract a test enforces and drift cannot land. This is the
op_kinds exhaustive-match idea applied to *crate layering*: the layers, the
allowed dependency direction between them, and where each kind of implementation
must live become DATA, and a `--check` ratchet fails CI on any regression.

AUTHORITY BOUNDARY (this module is itself allergic to duplicate authority):
  * THIS tool owns exactly one invariant: crate-LAYER ORGANIZATION -- layer
    assignment, allowed inter-layer dependency direction, single-home
    implementation (no facade / no duplicate authority across crates), and
    module placement.
  * tools/structural_audit.py owns the ORTHOGONAL invariant of structural DEBT
    (god-files, semantic fallthroughs, semantic duplicate authorities inside a
    crate). It does NOT decide layering; this does.
  Two tools, two disjoint invariants, no overlapping authority.

THE LAYER SCHEMA (mirrors CPython core / builtins / stdlib / third-party):
  core        -- primitives: the MoltObject value repr + object protocol +
                 compiler intrinsics. Depends on NOTHING above; everything
                 depends on it. (CPython Objects/ + core Python/.)
  stdlib      -- one crate per stdlib module. Depends on `core` ONLY -- never on
                 the `runtime` god-crate (that back-edge is the cycle that keeps
                 the god-crate un-splittable). (CPython Lib/ + Modules/.)
  third_party -- the CPython C-API/ABI surface + source-recompiled extension
                 custody. Depends on `core`. (CPython third-party C extensions.)
  runtime     -- the builtins namespace + call + object-protocol remainder still
                 inside the `molt-runtime` god-crate (being split down). May
                 depend on all lower layers.

Modes (mirrors tools/gen_op_kinds.py / structural_audit.py CI convention):
  canonicalization_contract.py                 human-readable violation board
  canonicalization_contract.py --json          machine-readable findings
  canonicalization_contract.py --check         exit 1 if any metric regressed vs baseline
  canonicalization_contract.py --update-baseline  re-pin the baseline
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from dataclasses import asdict, dataclass
from pathlib import Path

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

ROOT = bind_repository_imports(__file__)

from molt.cargo_workspace import (  # noqa: E402
    workspace_manifest_facts,
    workspace_member_manifests,
)
from tools import release_criterion_receipt as release_receipt  # noqa: E402

BASELINE_REL = "tools/canonicalization_contract_baseline.json"

# --- THE SCHEMA (declarative authority) -----------------------------------

# rank orders the layers; a layer may depend only on layers listed in
# `may_depend_on` (plus its own layer). Lower rank = more foundational.
LAYERS: dict[str, dict] = {
    "core": {"rank": 0, "may_depend_on": set()},
    "stdlib": {"rank": 2, "may_depend_on": {"core"}},
    "third_party": {"rank": 2, "may_depend_on": {"core"}},
    "runtime": {"rank": 3, "may_depend_on": {"core", "stdlib", "third_party"}},
}

# Package assignments and stdlib domains live only in runtime/crate_graph.toml.
# Its numeric build-DAG layer is a separate axis from this semantic contract.

# Stdlib-domain implementations that MUST live in their own crate, never as a
# large non-bridge module inside the god-crate `builtins/`. A large builtins
# module named for one of these domains is a DUPLICATE AUTHORITY (facade crate +
# impl left behind). `min_lines` filters thin bridges from real implementations.
FACADE_MIN_LINES = 400

# Code that is simply in the WRONG crate. domain-prefix -> where it belongs.
MISPLACED_IN_BUILTINS = {
    "gpu": "molt-gpu",
    "tensor": "molt-gpu",
}


@dataclass
class Violation:
    kind: str  # layer_dependency | duplicate_authority | misplaced_module
    severity: str  # high | medium
    crate: str
    detail: str
    metric: float = 0.0

    def line(self) -> str:
        return f"[{self.severity:<6}] {self.kind:<20} {self.crate:<26} {self.detail}"


# --- workspace loading ----------------------------------------------------


def _load_toml(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


@dataclass(frozen=True)
class RuntimeSemantics:
    layer: str
    domain: str | None


def load_runtime_semantics(
    root: Path, crates: dict[str, Path] | None = None
) -> dict[str, RuntimeSemantics]:
    """Project typed package metadata to actual directory aliases, fail closed."""
    graph = root / "runtime" / "crate_graph.toml"
    entries = _load_toml(graph).get("crate")
    if not isinstance(entries, list) or not entries:
        raise ValueError(f"{graph}: expected nonempty [[crate]] entries")
    packages: dict[str, RuntimeSemantics] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError(f"{graph}: crate entry must be a table")
        name = entry.get("name")
        layer = entry.get("runtime_layer")
        domain = entry.get("stdlib_domain")
        if not isinstance(name, str) or not name or name != name.strip():
            raise ValueError(f"{graph}: crate requires a package name")
        if name in packages:
            raise ValueError(f"{graph}: duplicate crate package {name!r}")
        if not isinstance(layer, str) or layer not in {*LAYERS, "outside"}:
            raise ValueError(f"{graph}: {name}: invalid runtime_layer {layer!r}")
        if layer == "stdlib":
            if (
                not isinstance(domain, str)
                or re.fullmatch(r"[a-z][a-z0-9_]*", domain) is None
            ):
                raise ValueError(
                    f"{graph}: {name}: stdlib requires a valid stdlib_domain"
                )
        elif "stdlib_domain" in entry:
            raise ValueError(
                f"{graph}: {name}: stdlib_domain requires runtime_layer='stdlib'"
            )
        packages[name] = RuntimeSemantics(layer, domain)

    if crates is None:
        crates = discover_crates(root)
    declared = {manifest.resolve() for manifest in workspace_member_manifests(root)}
    out: dict[str, RuntimeSemantics] = {}
    seen: dict[str, Path] = {}
    for cid, directory in crates.items():
        manifest = directory / "Cargo.toml"
        package = _load_toml(manifest).get("package")
        name = package.get("name") if isinstance(package, dict) else None
        if not isinstance(name, str) or not name or name != name.strip():
            raise ValueError(f"{manifest}: missing package.name")
        if name in seen:
            raise ValueError(
                f"{manifest}: duplicate package.name {name!r}; also {seen[name]}"
            )
        seen[name] = manifest
        if name in packages:
            out[cid] = packages[name]
        elif manifest.resolve() in declared:
            raise ValueError(
                f"{graph}: missing runtime semantics for workspace package {name!r}"
            )
    missing = packages.keys() - seen.keys()
    if missing:
        raise ValueError(
            f"{graph}: crate packages have no manifest authority: {sorted(missing)!r}"
        )
    return out


def workspace_members(root: Path) -> list[Path]:
    """Project crate directories from the sole root membership authority."""
    return [manifest.parent for manifest in workspace_member_manifests(root)]


def discover_crates(root: Path) -> dict[str, Path]:
    """All runtime crate directories plus the declared workspace members.

    The directory scan intentionally includes isolated and unregistered crates
    so the membership audit can report missing registrations. It does not
    determine which crates Cargo or the harness executes."""
    dirs: dict[str, Path] = {}
    for cargo in sorted(root.glob("runtime/*/Cargo.toml")):
        dirs[cargo.parent.name] = cargo.parent
    for d in workspace_members(root):
        if (d / "Cargo.toml").exists():
            dirs[d.name] = d
    return dirs


def crate_id(crate_dir: Path) -> str:
    return crate_dir.name


def layer_of(
    cid: str,
    semantics: dict[str, RuntimeSemantics] | None = None,
    *,
    root: Path = ROOT,
) -> str | None:
    item = (load_runtime_semantics(root) if semantics is None else semantics).get(cid)
    return item.layer if item is not None and item.layer != "outside" else None


def _stdlib_domain(
    cid: str,
    semantics: dict[str, RuntimeSemantics] | None = None,
    *,
    root: Path = ROOT,
) -> str | None:
    item = (load_runtime_semantics(root) if semantics is None else semantics).get(cid)
    return item.domain if item is not None else None


# --- checks ---------------------------------------------------------------


def check_dependency_direction(
    crates: dict[str, Path],
    semantics: dict[str, RuntimeSemantics] | None = None,
    *,
    root: Path = ROOT,
) -> list[Violation]:
    """A crate in layer L may depend only on crates in L or in
    LAYERS[L].may_depend_on. The load-bearing rule: a `stdlib` crate depending on
    `runtime` is the cycle that keeps the god-crate un-splittable."""
    if semantics is None:
        semantics = load_runtime_semantics(root, crates)
    known = {
        (directory / "Cargo.toml").resolve(): cid for cid, directory in crates.items()
    }
    adjacency: dict[str, set[str]] = {}
    for edge in workspace_manifest_facts(root).dependencies:
        source = known.get(edge.source_manifest)
        dependency = known.get(edge.dependency_manifest)
        if source is not None and dependency is not None:
            adjacency.setdefault(source, set()).add(dependency)
    out = []
    for cid in sorted(crates):
        L = layer_of(cid, semantics)
        if L is None:
            continue
        allowed = LAYERS[L]["may_depend_on"] | {L}
        for dep in sorted(adjacency.get(cid, ())):
            dL = layer_of(dep, semantics)
            if dL is None:
                continue
            if dL not in allowed:
                sev = (
                    "high"
                    if (L in ("stdlib", "third_party") and dL == "runtime")
                    else "medium"
                )
                out.append(
                    Violation(
                        kind="layer_dependency",
                        severity=sev,
                        crate=cid,
                        detail=f"layer '{L}' depends on '{dep}' (layer '{dL}') -- not allowed; "
                        f"{L} may depend only on {sorted(allowed)}",
                        metric=1,
                    )
                )
    return out


def _rs_line_count(path: Path) -> int:
    try:
        return sum(1 for _ in path.open("r", encoding="utf-8", errors="replace"))
    except OSError:
        return 0


def _builtins_domain_impl(root: Path, domain: str) -> list[tuple[Path, int]]:
    """Non-bridge builtins modules implementing `domain` (by name prefix)."""
    b = root / "runtime" / "molt-runtime" / "src" / "builtins"
    hits = []
    for p in sorted(b.glob(f"{domain}*.rs")):
        if "bridge" in p.name:
            continue
        hits.append((p, _rs_line_count(p)))
    sub = b / domain
    if sub.is_dir():
        for p in sorted(sub.rglob("*.rs")):
            if "bridge" not in p.name:
                hits.append((p, _rs_line_count(p)))
    return hits


def check_duplicate_authority(
    root: Path,
    crates: dict[str, Path],
    semantics: dict[str, RuntimeSemantics] | None = None,
) -> list[Violation]:
    """A stdlib crate exists AND a large non-bridge builtins module for the same
    domain also exists -> the implementation has two homes (facade crate + impl
    left behind). Single-home is the contract."""
    if semantics is None:
        semantics = load_runtime_semantics(root, crates)
    out = []
    for cid, cdir in sorted(crates.items()):
        if layer_of(cid, semantics) != "stdlib":
            continue
        domain = _stdlib_domain(cid, semantics)
        if domain is None:
            continue
        crate_lines = (
            sum(_rs_line_count(p) for p in (cdir / "src").rglob("*.rs"))
            if (cdir / "src").is_dir()
            else 0
        )
        impl = _builtins_domain_impl(root, domain)
        god_lines = sum(n for _p, n in impl)
        if god_lines >= FACADE_MIN_LINES and god_lines > crate_lines:
            files = ", ".join(f"{p.name}({n})" for p, n in impl)
            out.append(
                Violation(
                    kind="duplicate_authority",
                    severity="high",
                    crate=cid,
                    detail=f"crate impl={crate_lines} lines but builtins/ still holds "
                    f"{god_lines} lines for domain '{domain}' [{files}] -- move impl into the crate, "
                    f"leave a thin bridge",
                    metric=god_lines,
                )
            )
    return out


def check_misplaced_modules(root: Path) -> list[Violation]:
    """Code in `builtins/` that belongs to another crate entirely."""
    out = []
    for domain, home in sorted(MISPLACED_IN_BUILTINS.items()):
        impl = _builtins_domain_impl(root, domain)
        god_lines = sum(n for _p, n in impl)
        if god_lines > 0:
            files = ", ".join(f"{p.name}({n})" for p, n in impl)
            out.append(
                Violation(
                    kind="misplaced_module",
                    severity="medium",
                    crate="molt-runtime/builtins",
                    detail=f"{god_lines} lines of '{domain}' code in builtins/ belongs in {home} "
                    f"[{files}]",
                    metric=god_lines,
                )
            )
    return out


def check_workspace_membership(
    root: Path,
    crates: dict[str, Path],
    semantics: dict[str, RuntimeSemantics] | None = None,
) -> list[Violation]:
    """Every ordinary layer crate must declare its membership explicitly.

    Cargo can auto-enroll local path dependencies, but that implicit graph must
    not be a competing membership authority for tools and documentation.
    """
    if semantics is None:
        semantics = load_runtime_semantics(root, crates)
    members = {crate_id(d) for d in workspace_members(root)}
    out = []
    for cid in sorted(crates):
        if layer_of(cid, semantics) in LAYERS and cid not in members:
            out.append(
                Violation(
                    kind="not_workspace_member",
                    severity="medium",
                    crate=cid,
                    detail="layer crate is NOT explicitly declared in [workspace].members",
                    metric=1,
                )
            )
    return out


def run_all(root: Path) -> list[Violation]:
    crates = discover_crates(root)
    semantics = load_runtime_semantics(root, crates)
    vs = []
    vs += check_dependency_direction(crates, semantics, root=root)
    vs += check_duplicate_authority(root, crates, semantics)
    vs += check_misplaced_modules(root)
    vs += check_workspace_membership(root, crates, semantics)
    vs.sort(
        key=lambda v: (-{"high": 2, "medium": 1}.get(v.severity, 0), -v.metric, v.crate)
    )
    return vs


def ratchet_metrics(vs: list[Violation]) -> dict[str, float]:
    """Scalars that may only decrease; --check fails CI on any increase."""
    return {
        "layer_dependency_violations": float(
            sum(1 for v in vs if v.kind == "layer_dependency")
        ),
        "critical_layer_violations": float(
            sum(1 for v in vs if v.kind == "layer_dependency" and v.severity == "high")
        ),
        "duplicate_authority_domains": float(
            sum(1 for v in vs if v.kind == "duplicate_authority")
        ),
        "duplicate_authority_recoverable_lines": float(
            sum(int(v.metric) for v in vs if v.kind == "duplicate_authority")
        ),
        "misplaced_module_lines": float(
            sum(int(v.metric) for v in vs if v.kind == "misplaced_module")
        ),
        "non_member_layer_crates": float(
            sum(1 for v in vs if v.kind == "not_workspace_member")
        ),
    }


def main(argv: list[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--json", action="store_true")
    p.add_argument("--check", action="store_true")
    p.add_argument("--update-baseline", action="store_true")
    p.add_argument("--root", default=str(ROOT))
    release_receipt.add_receipt_arguments(p)
    args = p.parse_args(raw_argv)
    root = Path(args.root).resolve()
    if args.receipt is not None and args.update_baseline:
        p.error("--receipt cannot be combined with --update-baseline")
    try:
        receipt_destination = release_receipt.prepare_receipt_destination(
            repo_root=root,
            receipt_path=args.receipt,
            source_sha=args.source_sha,
        )
    except ValueError as exc:
        p.error(str(exc))

    try:
        vs = run_all(root)
    except (ValueError, OSError, UnicodeError) as exc:
        print(
            f"canonicalization contract: invalid crate authority: {exc}",
            file=sys.stderr,
        )
        return 2
    metrics = ratchet_metrics(vs)

    baseline: dict[str, float] | None = None
    regressed: list[tuple[str, float, float]] = []
    improved: list[str] = []
    if args.check or receipt_destination is not None:
        base_path = root / BASELINE_REL
        if not base_path.is_file():
            print(f"ERROR: no baseline at {base_path}", file=sys.stderr)
            return 2
        try:
            raw_baseline = json.loads(base_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            print(f"ERROR: invalid baseline at {base_path}: {exc}", file=sys.stderr)
            return 2
        if not isinstance(raw_baseline, dict):
            print(
                f"ERROR: baseline root is not an object: {base_path}", file=sys.stderr
            )
            return 2
        baseline = raw_baseline
        regressed = [
            (key, baseline.get(key, 0), metrics.get(key, 0))
            for key in metrics
            if metrics.get(key, 0) > baseline.get(key, 0)
        ]
        improved = sorted(
            key for key in metrics if metrics.get(key, 0) < baseline.get(key, 0)
        )

    if receipt_destination is not None:
        assert baseline is not None
        status = (
            release_receipt.STATUS_PASS
            if not regressed
            else release_receipt.STATUS_FAIL
        )
        try:
            receipt = release_receipt.build_receipt(
                kind=release_receipt.KIND_CANONICALIZATION_CONTRACT,
                source_sha=receipt_destination.source_sha,
                status=status,
                argv=raw_argv,
                tool_path=Path(__file__),
                facts={
                    "baseline_metrics": baseline,
                    "baseline_path": BASELINE_REL,
                    "improved_metrics": improved,
                    "metrics": metrics,
                    "open_violations": len(vs),
                    "regressed_metrics": sorted(key for key, _was, _now in regressed),
                },
                input_paths=[root / BASELINE_REL],
                repo_root=root,
            )
            release_receipt.write_receipt(receipt, receipt_destination)
        except ValueError as exc:
            print(f"canonicalization contract receipt: ERROR: {exc}", file=sys.stderr)
            return 2

    if args.json:
        print(
            json.dumps(
                {"violations": [asdict(v) for v in vs], "metrics": metrics}, indent=2
            )
        )
        return 1 if (args.check or receipt_destination is not None) and regressed else 0

    if args.update_baseline:
        (root / BASELINE_REL).write_text(
            json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"baseline re-pinned: {BASELINE_REL}")
        return 0

    if args.check or receipt_destination is not None:
        if regressed:
            print("CANONICALIZATION CONTRACT REGRESSED -- new layer/organization debt:")
            for k, was, now in regressed:
                print(f"  {k}: {was} -> {now}")
            print(
                "\nFix the violation or, if intentional, re-pin with --update-baseline."
            )
            return 1
        print(
            f"canonicalization contract OK ({len(vs)} open violations; improved: {improved or 'none'})"
        )
        if receipt_destination is not None:
            print(
                "canonicalization contract receipt written: "
                f"{receipt_destination.output_path}"
            )
        return 0

    # human board
    print(f"# Canonicalization contract -- {len(vs)} violations\n")
    for v in vs:
        print(v.line())
    print("\n## Ratchet metrics")
    for k, val in sorted(metrics.items()):
        print(f"  {k}: {int(val)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
