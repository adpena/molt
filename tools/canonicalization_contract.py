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
import sys
import tomllib
from dataclasses import asdict, dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
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

# Explicit crate (directory-name) -> layer. Stdlib crates are matched by the
# rule below rather than enumerated, so a new stdlib crate is governed
# automatically.
CRATE_LAYER: dict[str, str] = {
    "molt-obj-model": "core",
    "molt-codegen-abi": "core",
    "molt-runtime-core": "core",  # the API-surface facade (rename pending -> molt-runtime-api)
    "molt-cpython-abi": "third_party",
    "molt-runtime": "runtime",
}
# Any workspace crate whose dir name starts with this prefix and is not already
# assigned above is a STDLIB-layer crate. (Rename target: molt-stdlib-*.)
STDLIB_PREFIX = "molt-runtime-"

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


def workspace_members(root: Path) -> list[Path]:
    data = _load_toml(root / "Cargo.toml")
    members = data.get("workspace", {}).get("members", []) or data.get("members", [])
    out = []
    for m in members:
        # members may contain globs like "runtime/*"; expand simply.
        if "*" in m:
            out.extend(p.parent for p in root.glob(m + "/Cargo.toml"))
        else:
            p = root / m
            if (p / "Cargo.toml").exists():
                out.append(p)
    return out


def discover_crates(root: Path) -> dict[str, Path]:
    """All runtime crates by directory scan UNION the workspace members. Scanning
    `runtime/*/Cargo.toml` is authoritative because not every crate is listed in
    `[workspace].members` (e.g. molt-runtime-asyncio is a path-dep only) -- a
    members-only view silently misses those crates and their violations."""
    dirs: dict[str, Path] = {}
    for cargo in sorted(root.glob("runtime/*/Cargo.toml")):
        dirs[cargo.parent.name] = cargo.parent
    for d in workspace_members(root):
        if (d / "Cargo.toml").exists():
            dirs[d.name] = d
    return dirs


def crate_id(crate_dir: Path) -> str:
    return crate_dir.name


def _dep_dirs(crate_dir: Path) -> set[str]:
    """Directory names of workspace-internal (path) dependencies, from every
    dependencies table including target-specific ones."""
    data = _load_toml(crate_dir / "Cargo.toml")
    dirs: set[str] = set()

    def scan(tbl: dict) -> None:
        for _name, spec in tbl.items():
            if isinstance(spec, dict) and isinstance(spec.get("path"), str):
                p = spec["path"]
                if p.startswith("../") or p.startswith("..\\"):
                    dirs.add(Path(p).name)

    scan(data.get("dependencies", {}))
    scan(data.get("dev-dependencies", {}))
    for tgt in data.get("target", {}).values():
        if isinstance(tgt, dict):
            scan(tgt.get("dependencies", {}))
    return dirs


def layer_of(cid: str) -> str | None:
    if cid in CRATE_LAYER:
        return CRATE_LAYER[cid]
    if cid.startswith(STDLIB_PREFIX):
        return "stdlib"
    return None  # unclassified (backends/ir/passes/tooling) -- not layer-governed


# --- checks ---------------------------------------------------------------


def check_dependency_direction(crates: dict[str, Path]) -> list[Violation]:
    """A crate in layer L may depend only on crates in L or in
    LAYERS[L].may_depend_on. The load-bearing rule: a `stdlib` crate depending on
    `runtime` is the cycle that keeps the god-crate un-splittable."""
    out = []
    for cid, cdir in sorted(crates.items()):
        L = layer_of(cid)
        if L is None:
            continue
        allowed = LAYERS[L]["may_depend_on"] | {L}
        for dep in sorted(_dep_dirs(cdir)):
            dL = layer_of(dep)
            if dL is None:
                continue
            if dL not in allowed:
                sev = "high" if (L in ("stdlib", "third_party") and dL == "runtime") else "medium"
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


def check_duplicate_authority(root: Path, crates: dict[str, Path]) -> list[Violation]:
    """A stdlib crate exists AND a large non-bridge builtins module for the same
    domain also exists -> the implementation has two homes (facade crate + impl
    left behind). Single-home is the contract."""
    out = []
    for cid, cdir in sorted(crates.items()):
        if layer_of(cid) != "stdlib":
            continue
        domain = cid[len(STDLIB_PREFIX):]
        crate_lines = sum(_rs_line_count(p) for p in (cdir / "src").rglob("*.rs")) if (cdir / "src").is_dir() else 0
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


def check_workspace_membership(root: Path, crates: dict[str, Path]) -> list[Violation]:
    """Every layer crate should be a `[workspace].members` entry, or it escapes
    workspace-wide gates (cargo build --workspace, clippy-all). asyncio/math/etc.
    are path-deps only today."""
    members = {crate_id(d) for d in workspace_members(root)}
    out = []
    for cid in sorted(crates):
        if layer_of(cid) in ("core", "stdlib", "third_party", "runtime") and cid not in members:
            out.append(
                Violation(
                    kind="not_workspace_member",
                    severity="medium",
                    crate=cid,
                    detail="layer crate is a path-dep but NOT in [workspace].members -- "
                    "escapes workspace-wide gates (clippy-all, build --workspace)",
                    metric=1,
                )
            )
    return out


def run_all(root: Path) -> list[Violation]:
    crates = discover_crates(root)
    vs = []
    vs += check_dependency_direction(crates)
    vs += check_duplicate_authority(root, crates)
    vs += check_misplaced_modules(root)
    vs += check_workspace_membership(root, crates)
    vs.sort(key=lambda v: (-{"high": 2, "medium": 1}.get(v.severity, 0), -v.metric, v.crate))
    return vs


def ratchet_metrics(vs: list[Violation]) -> dict[str, float]:
    """Scalars that may only decrease; --check fails CI on any increase."""
    return {
        "layer_dependency_violations": float(sum(1 for v in vs if v.kind == "layer_dependency")),
        "critical_layer_violations": float(
            sum(1 for v in vs if v.kind == "layer_dependency" and v.severity == "high")
        ),
        "duplicate_authority_domains": float(sum(1 for v in vs if v.kind == "duplicate_authority")),
        "duplicate_authority_recoverable_lines": float(
            sum(int(v.metric) for v in vs if v.kind == "duplicate_authority")
        ),
        "misplaced_module_lines": float(sum(int(v.metric) for v in vs if v.kind == "misplaced_module")),
        "non_member_layer_crates": float(sum(1 for v in vs if v.kind == "not_workspace_member")),
    }


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--json", action="store_true")
    p.add_argument("--check", action="store_true")
    p.add_argument("--update-baseline", action="store_true")
    p.add_argument("--root", default=str(ROOT))
    args = p.parse_args(argv)
    root = Path(args.root)

    vs = run_all(root)
    metrics = ratchet_metrics(vs)

    if args.json:
        print(json.dumps({"violations": [asdict(v) for v in vs], "metrics": metrics}, indent=2))
        return 0

    if args.update_baseline:
        (root / BASELINE_REL).write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"baseline re-pinned: {BASELINE_REL}")
        return 0

    if args.check:
        base_path = root / BASELINE_REL
        baseline = json.loads(base_path.read_text(encoding="utf-8")) if base_path.exists() else {}
        regressed = [(k, baseline.get(k, 0), metrics.get(k, 0)) for k in metrics if metrics.get(k, 0) > baseline.get(k, 0)]
        if regressed:
            print("CANONICALIZATION CONTRACT REGRESSED -- new layer/organization debt:")
            for k, was, now in regressed:
                print(f"  {k}: {was} -> {now}")
            print("\nFix the violation or, if intentional, re-pin with --update-baseline.")
            return 1
        improved = [k for k in metrics if metrics.get(k, 0) < baseline.get(k, 0)]
        print(f"canonicalization contract OK ({len(vs)} open violations; improved: {improved or 'none'})")
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
