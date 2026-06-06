#!/usr/bin/env python3
"""Fail-closed, down-only ECOSYSTEM-COMPATIBILITY ratchet.

Arc 1 of docs/design/foundation/24_ecosystem_compat_gap_audit.md. Modeled
exactly on tools/check_satellite_parity.py (CONTRACT, not a sync/build script;
committed baseline; one-way ratchet; fail-closed on regression; deterministic
sorted output).

The problem this guard exists to kill
-------------------------------------
Per binding policy amendment A, library compatibility must be DERIVABLE from a
classification of the *dynamism features a library requires* against what molt
supports — never asserted as tribal knowledge. Two machine-readable manifests
encode doc 24's audit:

  * tools/ecosystem/dynamism_features.json  — the SINGLE SOURCE OF TRUTH for
    every dynamism feature's status (supported / typed-shim / bridge / partial /
    unsupported), each with file:line evidence carried over from doc 24.
  * tools/ecosystem/package_triage.json     — the 25 audited packages, each with
    the features it REQUIRES on its import + commonly-used path.

A package's verdict is the MIN over its required features' verdict classes (the
worst-status required feature wins). This guard RE-DERIVES every verdict from the
two manifests and FAILS if a stored verdict was hand-edited to disagree, if a
feature status lacks evidence, if a package references an unknown feature, if a
doctrine field (excluded_feature / tracking) is missing, or if the verdict
distribution REGRESSES versus the committed baseline.

The verdict lattice (best -> worst), and the feature-status -> verdict-class map:

    supported   -> compatible
    typed-shim  -> compatible-via-typed-shim
    bridge      -> compatible-via-bridge
    partial     -> partial
    unsupported -> incompatible-by-design

`compatible` is the top: a package requiring NO non-supported feature (empty
required set) is trivially compatible.

What this guard enforces (fail-closed everywhere)
-------------------------------------------------
For the feature manifest, the guard FAILS when any feature:
  * has an unknown status, or
  * has an empty `evidence`, or
  * is `unsupported` but has an empty `excluded_feature` (Lane E: an
    incompatible verdict MUST name the exact excluded feature), or
  * is `unsupported` or `partial` but has an empty `tracking` (anti-parking-lot
    doctrine: every not-yet-supported feature must name the arc/task/baton that
    converts it; a verdict can never be silently parked).

For the triage manifest, the guard FAILS when any package:
  * references a feature id absent from the feature manifest (unknown feature
    = worst class, fail-closed), or
  * lists the same id in both required_features and optional_features, or
  * has a STORED `verdict` that disagrees with the DERIVED verdict (no
    hand-edited verdicts), or
  * has a STORED `hardest_feature` that disagrees with the derived one, or
  * has `compile_probe_status` outside the allowed set.

Against the committed baseline (tools/ecosystem/ecosystem_compat_baseline.json),
the guard FAILS when, for any package:
  * its derived verdict REGRESSED versus baseline (moved DOWN the lattice), or
  * its evidence SHA changed while the verdict was unchanged (the provenance
    that justified the verdict changed -> re-verify), or
  * it has no baseline entry (a new package needs an explicit verdict), or
and globally when:
  * `compatible_floor` decreased (the count of `compatible` packages may only
    rise), or
  * `incompatible_ceiling` increased, or `partial_ceiling` increased (those
    counts may only fall — same one-way asymmetry as the satellite ceiling).

Graduating a feature in dynamism_features.json (e.g. D16 module __getattr__
lands -> status `unsupported` becomes `supported`) automatically RE-DERIVES every
dependent package verdict, raising the compatible floor and shrinking the
incompatible set. You then run --update-baseline, which the guard permits ONLY in
the improving direction (it refuses to lower the floor or raise a ceiling).

Usage
-----
  python3 tools/check_ecosystem_compat.py                 # check vs baseline
  python3 tools/check_ecosystem_compat.py --verbose       # + per-package table
  python3 tools/check_ecosystem_compat.py --show PACKAGE  # one package's derivation
  python3 tools/check_ecosystem_compat.py --matrix        # print the generated matrix
  python3 tools/check_ecosystem_compat.py --update-baseline
  python3 tools/check_ecosystem_compat.py --update-matrix # regenerate the .generated.md
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ECO_DIR = Path(__file__).resolve().parent / "ecosystem"
FEATURES_PATH = ECO_DIR / "dynamism_features.json"
TRIAGE_PATH = ECO_DIR / "package_triage.json"
BASELINE_PATH = ECO_DIR / "ecosystem_compat_baseline.json"
MATRIX_PATH = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compat"
    / "surfaces"
    / "ecosystem"
    / "ecosystem_compat_matrix.generated.md"
)

# --- the verdict lattice ---------------------------------------------------
# best -> worst; index == "badness rank". The min over required features is the
# verdict with the HIGHEST badness rank (the worst-status required feature).
VERDICT_ORDER = [
    "compatible",
    "compatible-via-typed-shim",
    "compatible-via-bridge",
    "partial",
    "incompatible-by-design",
]
VERDICT_RANK = {v: i for i, v in enumerate(VERDICT_ORDER)}

# feature status -> verdict class it contributes. Kept in lockstep with the
# `_status_to_verdict_class` block in dynamism_features.json; the guard asserts
# they agree so the two can never drift.
STATUS_TO_VERDICT = {
    "supported": "compatible",
    "typed-shim": "compatible-via-typed-shim",
    "bridge": "compatible-via-bridge",
    "partial": "partial",
    "unsupported": "incompatible-by-design",
}
VALID_STATUSES = set(STATUS_TO_VERDICT)

# Statuses that are NOT a finished, natively-supported state. Per the
# anti-parking-lot doctrine each such feature MUST name the arc/task/baton in
# `tracking` so no verdict is ever silently parked.
TRACKING_REQUIRED_STATUSES = {"partial", "unsupported"}

# Allowed compile-probe states. None is verified yet (fail-closed): the audit
# was paper-only. A later wave flips these to pass/fail with committed evidence.
VALID_PROBE_STATUS = {"pending", "pass", "fail", "skipped"}


def feature_sort_key(fid: str) -> tuple[int, int, str]:
    """Deterministic, human-intuitive feature ordering: D2 < D10 (numeric on the
    `D<n>` suffix), with a stable fallback for any non-`D<n>` id. Used for
    tie-breaking the `hardest_feature` and for all id iteration so a lexicographic
    surprise (\"D10\" < \"D2\") can never leak into a verdict or a cache value.
    """
    if len(fid) > 1 and fid[0] == "D" and fid[1:].isdigit():
        return (0, int(fid[1:]), "")
    return (1, 0, fid)


class GuardError(Exception):
    """A manifest-level defect (not a baseline regression). Always fatal."""


def _load_json(path: Path) -> dict:
    if not path.exists():
        raise GuardError(f"manifest missing: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:  # pragma: no cover - exercised via fixtures
        raise GuardError(f"manifest is not valid JSON ({path}): {exc}") from exc


def load_features() -> dict[str, dict]:
    data = _load_json(FEATURES_PATH)
    # Defensive: the embedded map in the manifest must agree with this module's
    # STATUS_TO_VERDICT, or the "single source of truth" claim is a lie.
    embedded = data.get("_status_to_verdict_class")
    if embedded is not None and embedded != STATUS_TO_VERDICT:
        raise GuardError(
            "dynamism_features.json `_status_to_verdict_class` disagrees with the "
            f"guard's STATUS_TO_VERDICT.\n  manifest: {embedded}\n  guard:    "
            f"{STATUS_TO_VERDICT}\nKeep them in lockstep."
        )
    features = data.get("features")
    if not isinstance(features, dict) or not features:
        raise GuardError("dynamism_features.json has no `features` object")
    return features


def load_triage() -> dict[str, dict]:
    data = _load_json(TRIAGE_PATH)
    packages = data.get("packages")
    if not isinstance(packages, dict) or not packages:
        raise GuardError("package_triage.json has no `packages` object")
    return packages


def validate_features(features: dict[str, dict]) -> list[str]:
    """Return a sorted list of feature-manifest defects (fail-closed checks)."""
    problems: list[str] = []
    for fid in sorted(features):
        feat = features[fid]
        status = feat.get("status")
        if status not in VALID_STATUSES:
            problems.append(
                f"[feature {fid}] invalid status {status!r}; allowed: "
                f"{sorted(VALID_STATUSES)}"
            )
            # Without a valid status the remaining checks are meaningless.
            continue
        if not str(feat.get("evidence", "")).strip():
            problems.append(
                f"[feature {fid}] status {status!r} but empty `evidence`. "
                "Every feature status must be backed by file:line evidence from "
                "doc 24."
            )
        if (
            status == "unsupported"
            and not str(feat.get("excluded_feature", "")).strip()
        ):
            problems.append(
                f"[feature {fid}] is `unsupported` but has no `excluded_feature`. "
                "An incompatible-by-design verdict MUST name the exact excluded "
                "feature (doc 24 Lane E / binding requirement)."
            )
        if (
            status in TRACKING_REQUIRED_STATUSES
            and not str(feat.get("tracking", "")).strip()
        ):
            problems.append(
                f"[feature {fid}] is `{status}` but has empty `tracking`. "
                "Anti-parking-lot doctrine: every not-yet-supported feature must "
                "name the arc/task/baton that converts it. A verdict can never be "
                "silently parked."
            )
    return problems


def feature_verdict(fid: str, features: dict[str, dict]) -> str:
    """Verdict class a feature contributes. FAIL-CLOSED: an unknown feature id
    or an invalid status maps to the WORST class (incompatible-by-design) rather
    than raising — so derivation never crashes on a malformed manifest, and a
    malformed entry can only make a verdict stricter, never silently pass.
    `validate_features` separately reports the malformed entry as a fatal defect.
    """
    feat = features.get(fid)
    if not isinstance(feat, dict):
        return "incompatible-by-design"
    return STATUS_TO_VERDICT.get(feat.get("status"), "incompatible-by-design")


def derive_verdict(
    required: list[str], features: dict[str, dict]
) -> tuple[str, str | None]:
    """Return (verdict, hardest_feature) for a required-feature set.

    verdict = min over the required features' verdict classes (== the worst /
    highest-badness-rank class). hardest_feature = the feature realizing that
    verdict; among ties the lowest feature id (deterministic). An empty required
    set is `compatible` (top of the lattice) with hardest_feature None.
    """
    if not required:
        return "compatible", None
    worst_rank = -1
    worst_verdict = "compatible"
    hardest: str | None = None
    # Iterate in deterministic, numeric feature order so tie-breaking the
    # hardest_feature is intuitive (D2 before D10) and never lexicographic.
    for fid in sorted(required, key=feature_sort_key):
        v = feature_verdict(fid, features)
        r = VERDICT_RANK[v]
        if r > worst_rank:
            worst_rank = r
            worst_verdict = v
            hardest = fid
    return worst_verdict, hardest


def evidence_sha(pkg: dict, features: dict[str, dict]) -> str:
    """Deterministic SHA over the provenance that PRODUCED the verdict.

    Per doc 24 Lane E.4 the per-package evidence hash must change whenever the
    justification behind the verdict changes. Until the functional suite is
    wired (compile_probe_status == 'pending' for all today), the provenance is:
    the required-feature ids in canonical order, each paired with that feature's
    current status + evidence string, plus the package's probe status. Any edit
    to a feature's status or evidence that bears on this package, or to the
    required set, flips the hash -> forces re-verification + a baseline update.
    """
    required = sorted(pkg.get("required_features", []))
    parts: list[str] = []
    for fid in required:
        feat = features.get(fid, {})
        parts.append(f"{fid}|{feat.get('status', '?')}|{feat.get('evidence', '')}")
    payload = "\n".join(
        [
            "required:" + ",".join(required),
            "probe:" + str(pkg.get("compile_probe_status", "")),
            *parts,
        ]
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def validate_triage(
    triage: dict[str, dict], features: dict[str, dict]
) -> tuple[list[str], dict[str, dict]]:
    """Validate the triage manifest and return (problems, derived).

    `derived[pkg]` = {verdict, hardest_feature, sha256_of_evidence} computed
    purely from the manifests (the authoritative values the baseline records).
    """
    problems: list[str] = []
    derived: dict[str, dict] = {}
    known = set(features)
    for name in sorted(triage):
        pkg = triage[name]
        required = pkg.get("required_features", [])
        optional = pkg.get("optional_features", [])
        if not isinstance(required, list) or not isinstance(optional, list):
            problems.append(
                f"[pkg {name}] required_features/optional_features must be lists"
            )
            continue
        # Fail-closed: an unknown feature id is the worst class — refuse it.
        unknown = sorted((set(required) | set(optional)) - known)
        if unknown:
            problems.append(
                f"[pkg {name}] references unknown feature id(s) {unknown}. "
                "A package may only reference features defined in "
                "dynamism_features.json (unknown = fail-closed)."
            )
            continue
        overlap = sorted(set(required) & set(optional))
        if overlap:
            problems.append(
                f"[pkg {name}] feature id(s) {overlap} appear in BOTH "
                "required_features and optional_features."
            )
            continue
        probe = pkg.get("compile_probe_status")
        if probe not in VALID_PROBE_STATUS:
            problems.append(
                f"[pkg {name}] compile_probe_status {probe!r} invalid; allowed: "
                f"{sorted(VALID_PROBE_STATUS)}"
            )
        verdict, hardest = derive_verdict(required, features)
        derived[name] = {
            "verdict": verdict,
            "hardest_feature": hardest,
            "sha256_of_evidence": evidence_sha(pkg, features),
        }
        # No hand-edited verdicts: the stored cache MUST equal the derivation.
        stored_verdict = pkg.get("verdict")
        if stored_verdict is not None and stored_verdict != verdict:
            problems.append(
                f"[pkg {name}] STORED verdict {stored_verdict!r} != DERIVED "
                f"verdict {verdict!r}. Verdicts are DERIVED, never hand-asserted. "
                "Fix the required_features (or the feature statuses), not the "
                "stored verdict."
            )
        stored_hardest = pkg.get("hardest_feature")
        if stored_hardest is not None and stored_hardest != hardest:
            problems.append(
                f"[pkg {name}] STORED hardest_feature {stored_hardest!r} != "
                f"DERIVED {hardest!r}."
            )
    return problems, derived


def verdict_distribution(derived: dict[str, dict]) -> dict[str, int]:
    counts = {v: 0 for v in VERDICT_ORDER}
    for d in derived.values():
        counts[d["verdict"]] += 1
    return counts


def load_baseline() -> dict:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def _baseline_payload(derived: dict[str, dict]) -> dict:
    dist = verdict_distribution(derived)
    return {
        "_comment": (
            "Fail-closed, DOWN-ONLY ecosystem-compatibility baseline. Generated "
            "by tools/check_ecosystem_compat.py --update-baseline. Every package "
            "verdict here is DERIVED from tools/ecosystem/dynamism_features.json + "
            "package_triage.json — never hand-edited. compatible_floor may only "
            "INCREASE; incompatible_ceiling and partial_ceiling may only DECREASE "
            "(the guard refuses the wrong direction). Graduate a feature's status "
            "(with evidence) to improve verdicts, then regenerate. See the script "
            "docstring and docs/design/foundation/24_ecosystem_compat_gap_audit.md."
        ),
        "compatible_floor": dist["compatible"],
        "incompatible_ceiling": dist["incompatible-by-design"],
        "partial_ceiling": dist["partial"],
        "distribution": dist,
        "packages": {
            name: {
                "verdict": derived[name]["verdict"],
                "hardest_feature": derived[name]["hardest_feature"],
                "sha256_of_evidence": derived[name]["sha256_of_evidence"],
            }
            for name in sorted(derived)
        },
    }


def cmd_update_baseline() -> int:
    features = load_features()
    triage = load_triage()
    fproblems = validate_features(features)
    tproblems, derived = validate_triage(triage, features)
    problems = fproblems + tproblems
    if problems:
        print(
            "REFUSING to update baseline: the manifests have defects that must be "
            "fixed first:\n",
            file=sys.stderr,
        )
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    new = _baseline_payload(derived)
    prev = load_baseline()
    # One-way ratchet, mirroring the satellite ceiling: refuse any regressing
    # baseline rewrite. The good metric (compatible_floor) may only rise; the bad
    # metrics (ceilings) may only fall.
    if prev:
        if new["compatible_floor"] < prev.get("compatible_floor", 0):
            print(
                f"REFUSING to lower compatible_floor "
                f"{prev.get('compatible_floor')} -> {new['compatible_floor']}. "
                "A regression made fewer packages compatible. Fix the regression "
                "instead of widening the baseline.",
                file=sys.stderr,
            )
            return 1
        for key in ("incompatible_ceiling", "partial_ceiling"):
            prev_v = prev.get(key)
            if prev_v is not None and new[key] > prev_v:
                print(
                    f"REFUSING to raise {key} {prev_v} -> {new[key]}. A "
                    "regression moved a package to a worse class. Fix it instead "
                    "of widening the baseline.",
                    file=sys.stderr,
                )
                return 1
    BASELINE_PATH.write_text(
        json.dumps(new, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )
    dist = new["distribution"]
    print(
        "baseline updated: "
        + ", ".join(f"{k}={v}" for k, v in dist.items())
        + f" (floor={new['compatible_floor']}, "
        f"incompatible_ceiling={new['incompatible_ceiling']}, "
        f"partial_ceiling={new['partial_ceiling']})"
    )
    return 0


def _check_against_baseline(derived: dict[str, dict], baseline: dict) -> list[str]:
    failures: list[str] = []
    base_pkgs = baseline.get("packages", {})
    for name in sorted(derived):
        cur = derived[name]
        base = base_pkgs.get(name)
        if base is None:
            failures.append(
                f"[pkg {name}] has no baseline entry (derived verdict "
                f"{cur['verdict']!r}). A new package needs an explicit verdict: "
                "run --update-baseline after confirming it is intentional."
            )
            continue
        cur_rank = VERDICT_RANK[cur["verdict"]]
        base_rank = VERDICT_RANK[base["verdict"]]
        if cur_rank > base_rank:
            failures.append(
                f"[pkg {name}] VERDICT REGRESSED: {base['verdict']} -> "
                f"{cur['verdict']} (moved DOWN the lattice). A feature it requires "
                "lost support, or its required set grew. Fix the regression, or "
                "(if intentional) --update-baseline — which is allowed only in the "
                "improving direction."
            )
        elif cur_rank == base_rank and cur["sha256_of_evidence"] != base.get(
            "sha256_of_evidence"
        ):
            failures.append(
                f"[pkg {name}] EVIDENCE CHANGED (verdict unchanged at "
                f"{cur['verdict']!r}): the provenance that justified the verdict "
                "changed (a required feature's status/evidence, or the required "
                f"set). Re-verify and --update-baseline. Run --show {name}."
            )
    # Global down-only ratchet on the distribution.
    dist = verdict_distribution(derived)
    floor = baseline.get("compatible_floor")
    if floor is not None and dist["compatible"] < floor:
        failures.append(
            f"compatible_floor regressed: {dist['compatible']} compatible "
            f"packages < committed floor {floor}."
        )
    inc_ceiling = baseline.get("incompatible_ceiling")
    if inc_ceiling is not None and dist["incompatible-by-design"] > inc_ceiling:
        failures.append(
            f"incompatible_ceiling exceeded: {dist['incompatible-by-design']} "
            f"incompatible-by-design packages > committed ceiling {inc_ceiling}."
        )
    part_ceiling = baseline.get("partial_ceiling")
    if part_ceiling is not None and dist["partial"] > part_ceiling:
        failures.append(
            f"partial_ceiling exceeded: {dist['partial']} partial packages > "
            f"committed ceiling {part_ceiling}."
        )
    return failures


def cmd_check(verbose: bool) -> int:
    try:
        features = load_features()
        triage = load_triage()
    except GuardError as exc:
        print(f"\nECOSYSTEM COMPAT GUARD FAILED:\n  - {exc}\n", file=sys.stderr)
        return 1
    problems = validate_features(features)
    tproblems, derived = validate_triage(triage, features)
    problems += tproblems
    baseline = load_baseline()
    if not baseline:
        problems.append(
            "no committed baseline at tools/ecosystem/ecosystem_compat_baseline."
            "json. Run --update-baseline once to establish the ratchet."
        )
    else:
        problems += _check_against_baseline(derived, baseline)

    if verbose:
        dist = verdict_distribution(derived)
        print(f"{'package':<20} {'derived verdict':<26} {'hardest':<8}")
        for name in sorted(derived):
            d = derived[name]
            print(f"{name:<20} {d['verdict']:<26} {(d['hardest_feature'] or '-'):<8}")
        print()
        print("verdict distribution:")
        for v in VERDICT_ORDER:
            print(f"  {v:<26} {dist[v]}")
        print(f"  {'TOTAL':<26} {sum(dist.values())}")

    if problems:
        print("\nECOSYSTEM COMPAT GUARD FAILED:\n", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        print(
            "\nLibrary compatibility verdicts are DERIVED from the dynamism-"
            "feature taxonomy (tools/ecosystem/dynamism_features.json) and the "
            "package triage (package_triage.json), and ratcheted DOWN-ONLY "
            "against tools/ecosystem/ecosystem_compat_baseline.json. Fix the "
            "manifests / the regression; do not hand-edit verdicts. See doc 24.\n",
            file=sys.stderr,
        )
        return 1
    dist = verdict_distribution(derived)
    print(
        f"ecosystem compat OK: {len(derived)} packages within baseline. "
        + ", ".join(f"{k}={v}" for k, v in dist.items())
    )
    return 0


def cmd_show(name: str) -> int:
    features = load_features()
    triage = load_triage()
    if name not in triage:
        print(
            f"unknown package '{name}'. Known: {', '.join(sorted(triage))}",
            file=sys.stderr,
        )
        return 2
    pkg = triage[name]
    required = pkg.get("required_features", [])
    optional = pkg.get("optional_features", [])
    verdict, hardest = derive_verdict(required, features)
    print(f"# package {name}")
    print(f"#   source: {pkg.get('source')}")
    print(f"#   compile_probe_status: {pkg.get('compile_probe_status')}")
    print(f"#   derived verdict: {verdict}")
    print(f"#   hardest_feature: {hardest}")
    print(f"#   evidence sha256: {evidence_sha(pkg, features)}")
    print("#   required features (min over these):")
    for fid in sorted(required):
        feat = features[fid]
        print(
            f"#     {fid:<5} status={feat['status']:<12} "
            f"verdict={feature_verdict(fid, features):<26} {feat['name']}"
        )
    if optional:
        print("#   optional features (NOT in the min — provenance only):")
        for fid in sorted(optional):
            feat = features[fid]
            print(f"#     {fid:<5} status={feat['status']:<12} {feat['name']}")
    basis = pkg.get("source_basis")
    if basis:
        print("#   source_basis:")
        print(f"#     {basis}")
    return 0


MATRIX_HEADER = (
    "<!-- GENERATED by tools/check_ecosystem_compat.py --update-matrix from the\n"
    "     canonical manifests (tools/ecosystem/dynamism_features.json +\n"
    "     package_triage.json). DO NOT EDIT BY HAND — edits are overwritten and\n"
    "     the guard fails if this file is stale. Regenerate after changing a\n"
    "     feature status or a package's required features. -->\n"
)


def render_matrix(derived: dict[str, dict], features: dict[str, dict]) -> str:
    triage = load_triage()
    dist = verdict_distribution(derived)
    lines: list[str] = [MATRIX_HEADER]
    lines.append("# Ecosystem Compatibility Matrix (derived)\n")
    lines.append(
        "> Verdicts are DERIVED as the min over each package's required dynamism "
        "features (doc 24 Lane B/C). `compatible-via-bridge` involves the CPython-"
        "ABI bridge whose policy is an OPEN USER DECISION (doc 24 OQ1). All "
        "`compile_probe` are `pending` — no package is molt-build-verified yet "
        "(fail-closed).\n"
    )
    lines.append("## Verdict distribution\n")
    lines.append("| Verdict class | Count |")
    lines.append("|---|---|")
    for v in VERDICT_ORDER:
        lines.append(f"| {v} | {dist[v]} |")
    lines.append(f"| **TOTAL** | **{sum(dist.values())}** |")
    lines.append("")
    lines.append("## Packages\n")
    lines.append(
        "| Package | Verdict | Hardest feature | Required | Optional | Probe |"
    )
    lines.append("|---|---|---|---|---|---|")
    for name in sorted(derived):
        d = derived[name]
        pkg = triage[name]
        req = ", ".join(sorted(pkg.get("required_features", []))) or "(none)"
        opt = ", ".join(sorted(pkg.get("optional_features", []))) or "-"
        hf = d["hardest_feature"] or "-"
        note = ""
        if d["verdict"] == "compatible-via-bridge":
            note = " (policy decision pending — doc 24 OQ1)"
        lines.append(
            f"| {name} | {d['verdict']}{note} | {hf} | {req} | {opt} | "
            f"{pkg.get('compile_probe_status')} |"
        )
    lines.append("")
    lines.append("## Feature taxonomy (status reference)\n")
    lines.append("| Feature | Status | Verdict class | Tracking |")
    lines.append("|---|---|---|---|")
    for fid in sorted(features, key=feature_sort_key):
        feat = features[fid]
        tracking = feat.get("tracking", "") or "-"
        lines.append(
            f"| {fid} {feat.get('name', '?')} | {feat.get('status', '?')} | "
            f"{feature_verdict(fid, features)} | {tracking} |"
        )
    lines.append("")
    return "\n".join(lines)


def cmd_update_matrix() -> int:
    features = load_features()
    triage = load_triage()
    fproblems = validate_features(features)
    tproblems, derived = validate_triage(triage, features)
    if fproblems + tproblems:
        print(
            "REFUSING to regenerate matrix: fix manifest defects first "
            "(run the guard).",
            file=sys.stderr,
        )
        return 1
    MATRIX_PATH.parent.mkdir(parents=True, exist_ok=True)
    MATRIX_PATH.write_text(render_matrix(derived, features), encoding="utf-8")
    print(f"matrix regenerated: {MATRIX_PATH.relative_to(ROOT)}")
    return 0


def cmd_matrix() -> int:
    features = load_features()
    triage = load_triage()
    _f = validate_features(features)
    _t, derived = validate_triage(triage, features)
    print(render_matrix(derived, features))
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="regenerate the committed baseline (improving direction only)",
    )
    ap.add_argument(
        "--update-matrix",
        action="store_true",
        help="regenerate the generated ecosystem compat matrix doc",
    )
    ap.add_argument(
        "--verbose", action="store_true", help="print the per-package table"
    )
    ap.add_argument("--show", metavar="PACKAGE", help="print one package's derivation")
    ap.add_argument(
        "--matrix", action="store_true", help="print the matrix to stdout (no write)"
    )
    args = ap.parse_args(argv)
    try:
        if args.show:
            return cmd_show(args.show)
        if args.matrix:
            return cmd_matrix()
        if args.update_matrix:
            return cmd_update_matrix()
        if args.update_baseline:
            return cmd_update_baseline()
        return cmd_check(args.verbose)
    except GuardError as exc:
        print(f"\nECOSYSTEM COMPAT GUARD FAILED:\n  - {exc}\n", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
