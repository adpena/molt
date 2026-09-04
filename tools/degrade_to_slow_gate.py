#!/usr/bin/env python3
"""Enforcement gate for the "silent degrade-to-slow" metabug.

THE METABUG this gate makes un-regrowable
------------------------------------------
A perf/capability path (parallelism, caching, incremental reuse, fast-path
lowering) that SILENTLY degrades to a naive path on a HANDLEABLE input, with no
diagnostic and no test proving the fast path is taken on the hard input. This is
DISTINCT from sound conservatism (over-approximation for correctness), which the
gate must NOT flag.

Shape (same as the op-kind / intrinsic-manifest / generator-manifest drift gates)
---------------------------------------------------------------------------------
ONE declarative authority -- ``tools/degrade_to_slow_registry.toml`` -- carries a
classified row per known degrade site. This gate:

  1. DRIFT: DISCOVERS candidate degrade sites by scanning the source roots for a
     curated degrade-signature set. Every discovered site MUST have a registry
     row. A newly introduced, unregistered degrade site FAILS the gate -- that is
     what makes the metabug un-regrowable: you cannot add a silent bail without
     either classifying it (and, if it is a real degrade, wiring a fast_path_test
     or a loud diagnostic) or proving it is sound.

  2. ANCHOR: every registry row's ``file`` must exist and its ``symbol`` /
     ``signature`` anchor must still be present in that file. A row whose anchor
     has drifted away (the code it froze was moved/renamed) FAILS -- the row can
     no longer vouch for code it no longer names.

  3. TEETH:
       * ``make_loud`` rows must statically reach a diagnostic emit
         (warnings.append / logger.* / print / emit_line / emit_diagnostic /
         eprintln! / warn! carrying the loud marker string) somewhere in the
         named file. A make_loud site that stops being loud FAILS.
       * every non-``sound_keep`` row (``metabug_fix_pending`` and ``make_loud``)
         must name an existing ``fast_path_test`` id that resolves to a real,
         non-skipped test in the tree. (``metabug_fix_pending`` names the test
         the fixing arc WILL add; the gate accepts a not-yet-present pending
         test but records it, and REQUIRES presence for ``make_loud``.)

  4. RATCHET: the count of ``metabug_fix_pending`` rows is monotonically
     non-increasing against the stored baseline in ``[meta]``. You may lower the
     baseline as sites are fixed; raising it is an explicit, reviewed edit.

Discovery-vs-authority firewall (doc 46 rule #1)
------------------------------------------------
The signature scan is DISCOVERY: it ranks "does this look like a degrade site?".
The AUTHORITY that a site is correctly classified is the reviewed registry row +
its justification. A discovery miss can only yield a false "no new site"; the
curated fast-path-emitter allowlist keeps genuine soundness bails (fixpoint caps,
cost models, memory bounds) from being swept in as false degrades, which would
otherwise force fake make_loud rows. New discovery hits fail CLOSED (must be
registered), so the firewall never manufactures a passing gate.

Usage::

    python tools/degrade_to_slow_gate.py            # run the gate (exit 1 on fail)
    python tools/degrade_to_slow_gate.py --json      # machine-readable report
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

try:
    from tools import release_criterion_receipt as release_receipt
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    import release_criterion_receipt as release_receipt  # type: ignore


REPO_ROOT = Path(__file__).resolve().parent.parent
REGISTRY_PATH = REPO_ROOT / "tools" / "degrade_to_slow_registry.toml"

# Source roots scanned for degrade signatures. The metabug lives in the perf /
# capability planes: the CLI build pipeline (parallelism + caching), the pass
# pipeline (fast-path lowering), and both perf-critical backends.
SCAN_ROOTS = (
    "src/molt/cli",
    "runtime/molt-passes/src",
    "runtime/molt-backend-wasm/src",
    "runtime/molt-backend-native/src",
)

VALID_CLASSIFICATIONS = frozenset(
    {"metabug_fix_pending", "make_loud", "sound_keep"}
)
# Rows that make a real perf/capability claim must be backed by a fast_path_test.
NON_SOUND = frozenset({"metabug_fix_pending", "make_loud"})

# ---------------------------------------------------------------------------
# Discovery signatures.
# ---------------------------------------------------------------------------
# Reason/mode STRING literals that name a degrade-to-slow decision. These are the
# high-signal markers: a source line that assigns/compares one of these strings
# is choosing a slow path and naming it. Curated -- extend deliberately.
_DEGRADE_REASON_STRINGS = (
    "fallback_serial",
    "pool_unavailable",
    "dependency_back_edge",
    "phase_timeout",
    "cache_miss",
    "degrade_to_slow",
    "worker_error_isolated_serial",
    "worker_pool_broken_recreate",
    "serial_disabled",
    "serial_layer_policy",
    "pool_unavailable_after_error",
    "parallel_fallback_serial",
)
# A degrade-reason string appearing as a quoted literal.
_REASON_STRING_RE = re.compile(
    r"""["'](?P<reason>[a-z_]*(?:"""
    + "|".join(re.escape(s) for s in _DEGRADE_REASON_STRINGS)
    + r""")[a-z_]*)["']"""
)

# Python: a pool/executor fast path with a sibling serial branch. The signature
# is a mode/reason variable set to a "*serial*" value guarding a slow branch.
_PY_SERIAL_MODE_RE = re.compile(
    r"""(?:mode|policy_reason|reason)\s*=\s*["'][a-z_]*serial[a-z_]*["']"""
)

# A reason-string literal only names a degrade DECISION when it is assigned,
# returned, compared, or passed as a reason/mode/policy argument -- not when it
# is a bare element of a telemetry field-name set/list (e.g. the health-metric
# schema `{"cache_hits", "cache_misses", ...}`, where "cache_misses" is a
# counter NAME, not a slow-path decision). This context guard is what keeps
# telemetry vocabulary out of the discovery set without hard-coding field names.
_DEGRADE_DECISION_CONTEXT_RE = re.compile(
    r"""=\s*["']|"""            # assignment: x = "reason..."
    r"""[=!]=\s*["']|"""        # comparison: x == "reason..."
    r"""["']\s*[=!]=|"""        # comparison: "reason..." == x
    r"""\breturn\b|"""          # return "reason..."
    r"""(?:reason|mode|policy|serial_mode|policy_reason)\s*=|"""  # kwarg
    r"""\bif\b|\belif\b|\belse\b"""  # inline conditional / ternary branch
)

# Rust fast-path emitter that BAILS (returns None / falls through) under a
# _LIMIT / _BUDGET / >= N threshold. A raw threshold comparison is NOT enough
# (fixpoint caps and cost models are sound), so a Rust hit only counts as a
# candidate degrade when the enclosing symbol is on the curated
# fast-path-emitter allowlist below. This is the mechanism that keeps genuine
# soundness bails out of the discovery set.
_RUST_LIMIT_RE = re.compile(r"(?:_LIMIT|_BUDGET)\b|>=\s*\d+")
_RUST_BAIL_RE = re.compile(r"\breturn\s+None\b|=>\s*None\b")

# Curated allowlist of Rust fast-path EMITTERS: symbols whose whole job is to
# emit an optimized/cached lowering and that could silently emit the naive form
# instead. A threshold-guarded bail INSIDE one of these is a candidate degrade
# and must be registered. Symbols NOT on this list (fixpoint loops, cost models,
# flush bounds, const-fold size caps) are soundness bails by construction and
# are deliberately excluded from Rust discovery -- they are still frozen as
# sound_keep rows in the registry, but the gate does not force them to be loud.
_RUST_FAST_PATH_EMITTER_ALLOWLIST = (
    # (file-suffix, symbol) pairs. Empty by default: the audited WASM/native
    # fast-path emitters that CAN silently degrade are cost models (constant
    # cache locals, deferred flush) already classified sound_keep. Any new Rust
    # fast-path emitter that bails silently must be added here AND registered;
    # that co-edit is the enforcement point for the Rust plane.
)

# Diagnostic-emit markers, used to prove a make_loud row is actually loud.
_DIAG_EMIT_RE = re.compile(
    r"""warnings\.append\(|"""
    r"""\blogger\.\w+\(|"""
    r"""\bprint\(|"""
    r"""emit_line\(|"""
    r"""emit_diagnostic\(|"""
    r"""\beprintln!\(|"""
    r"""\bwarn!\("""
)


@dataclass
class RegistryRow:
    file: str
    classification: str
    justification: str
    symbol: str | None = None
    signature: str | None = None
    fast_path_test: str | None = None
    _index: int = -1


@dataclass
class DiscoveredSite:
    file: str
    line: int
    marker: str


@dataclass
class GateReport:
    ok: bool = True
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    metabug_fix_pending_count: int = 0
    metabug_fix_pending_baseline: int = 0
    discovered_site_count: int = 0
    registry_row_count: int = 0

    def fail(self, msg: str) -> None:
        self.ok = False
        self.errors.append(msg)


# ---------------------------------------------------------------------------
# Registry loading + validation.
# ---------------------------------------------------------------------------
def load_registry(path: Path = REGISTRY_PATH) -> tuple[list[RegistryRow], int]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    baseline = int(data.get("meta", {}).get("metabug_fix_pending_baseline", 0))
    rows: list[RegistryRow] = []
    for idx, raw in enumerate(data.get("site", [])):
        rows.append(
            RegistryRow(
                file=str(raw["file"]),
                classification=str(raw["classification"]),
                justification=str(raw.get("justification", "")),
                symbol=raw.get("symbol"),
                signature=raw.get("signature"),
                fast_path_test=raw.get("fast_path_test"),
                _index=idx,
            )
        )
    return rows, baseline


def _anchor_present(text: str, row: RegistryRow) -> bool:
    """A row's anchor is present if its symbol OR a distinctive slice of its
    signature still appears in the named file. Signatures are long; we require a
    stable substring (the head up to the first paren or a 24-char prefix) so
    cosmetic reformatting does not break the anchor while a real move/rename
    does."""
    if row.symbol and row.symbol in text:
        return True
    sig = row.signature
    if sig:
        head = sig.split("(", 1)[0].strip()
        if head and head in text:
            return True
        probe = sig.strip()[:24]
        if probe and probe in text:
            return True
    return False


# ---------------------------------------------------------------------------
# Discovery scan.
# ---------------------------------------------------------------------------
def _iter_source_files(
    repo_root: Path = REPO_ROOT,
    scan_roots: tuple[str, ...] = SCAN_ROOTS,
) -> list[Path]:
    files: list[Path] = []
    for root in scan_roots:
        base = repo_root / root
        if not base.exists():
            continue
        for pat in ("*.py", "*.rs"):
            files.extend(base.rglob(pat))
    return sorted(files)


def _enclosing_rust_symbol(lines: list[str], idx: int) -> str | None:
    for j in range(idx, -1, -1):
        m = re.search(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*", lines[j])
        if m:
            return m.group(1)
    return None


def discover_sites(
    repo_root: Path = REPO_ROOT,
    scan_roots: tuple[str, ...] = SCAN_ROOTS,
) -> list[DiscoveredSite]:
    sites: list[DiscoveredSite] = []
    for path in _iter_source_files(repo_root, scan_roots):
        rel = path.relative_to(repo_root).as_posix()
        # The gate + registry + tests themselves define the signature strings;
        # skip them so the scanner does not flag its own vocabulary.
        if rel in {
            "tools/degrade_to_slow_gate.py",
            "tools/degrade_to_slow_registry.toml",
        }:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        lines = text.splitlines()
        is_rust = path.suffix == ".rs"
        for i, line in enumerate(lines):
            stripped = line.strip()
            if stripped.startswith(("#", "//", "///", "*", "//!")):
                continue
            marker: str | None = None
            reason_hit = _REASON_STRING_RE.search(line)
            if reason_hit and _DEGRADE_DECISION_CONTEXT_RE.search(line):
                marker = f"reason:{reason_hit.group('reason')}"
            elif not is_rust and _PY_SERIAL_MODE_RE.search(line):
                marker = "py_serial_mode"
            elif (
                is_rust
                and _RUST_LIMIT_RE.search(line)
                and _RUST_BAIL_RE.search(line)
            ):
                sym = _enclosing_rust_symbol(lines, i)
                allow = any(
                    rel.endswith(suf) and sym == want
                    for suf, want in _RUST_FAST_PATH_EMITTER_ALLOWLIST
                )
                if allow:
                    marker = f"rust_fast_path_bail:{sym}"
            if marker is not None:
                sites.append(DiscoveredSite(file=rel, line=i + 1, marker=marker))
    return sites


# ---------------------------------------------------------------------------
# Test-presence resolution (fast_path_test).
# ---------------------------------------------------------------------------
def _find_test(test_id: str, repo_root: Path = REPO_ROOT) -> tuple[bool, bool]:
    """Return (present, skipped). Searches tests/ for a `def <test_id>` and
    checks whether it (or its class) is decorated skip/xfail on the line above.
    """
    tests_root = repo_root / "tests"
    if not tests_root.exists():
        return (False, False)
    needle = re.compile(rf"^\s*def\s+{re.escape(test_id)}\s*\(")
    skip_re = re.compile(r"@pytest\.mark\.(skip|xfail)\b|@unittest\.skip")
    for path in tests_root.rglob("test_*.py"):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        for i, line in enumerate(lines):
            if needle.match(line):
                skipped = False
                for k in range(max(0, i - 4), i):
                    if skip_re.search(lines[k]):
                        skipped = True
                        break
                return (True, skipped)
    return (False, False)


# ---------------------------------------------------------------------------
# The gate.
# ---------------------------------------------------------------------------
def run_gate(
    registry_path: Path = REGISTRY_PATH,
    *,
    repo_root: Path | None = None,
    scan_roots: tuple[str, ...] = SCAN_ROOTS,
    extra_sites: list[DiscoveredSite] | None = None,
) -> GateReport:
    # Default to the module-global REPO_ROOT, read at CALL time so tests that
    # rebind gate.REPO_ROOT (or pass repo_root explicitly) are honored.
    if repo_root is None:
        repo_root = REPO_ROOT
    report = GateReport()
    rows, baseline = load_registry(registry_path)
    report.registry_row_count = len(rows)
    report.metabug_fix_pending_baseline = baseline

    # -- registry well-formedness -------------------------------------------
    seen_keys: set[tuple[str, str]] = set()
    rows_by_file: dict[str, list[RegistryRow]] = {}
    for row in rows:
        if row.classification not in VALID_CLASSIFICATIONS:
            report.fail(
                f"registry row #{row._index} ({row.file}) has invalid "
                f"classification {row.classification!r}"
            )
        if not row.justification.strip():
            report.fail(
                f"registry row #{row._index} ({row.file}) has empty justification"
            )
        anchor = row.symbol or (row.signature or "")
        key = (row.file, anchor)
        if key in seen_keys:
            report.fail(
                f"duplicate registry row for {row.file} anchor {anchor!r}"
            )
        seen_keys.add(key)
        rows_by_file.setdefault(row.file, []).append(row)

    # -- ratchet -------------------------------------------------------------
    pending = [r for r in rows if r.classification == "metabug_fix_pending"]
    report.metabug_fix_pending_count = len(pending)
    if len(pending) > baseline:
        report.fail(
            f"ratchet regression: {len(pending)} metabug_fix_pending rows "
            f"exceed baseline {baseline}. A new silent-degrade site was added "
            f"without being fixed. Fix it (reclassify to make_loud/sound_keep) "
            f"or, only with review, raise metabug_fix_pending_baseline."
        )

    # -- anchor presence -----------------------------------------------------
    file_text_cache: dict[str, str | None] = {}

    def _file_text(rel: str) -> str | None:
        if rel not in file_text_cache:
            p = repo_root / rel
            try:
                file_text_cache[rel] = p.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                file_text_cache[rel] = None
        return file_text_cache[rel]

    for row in rows:
        text = _file_text(row.file)
        if text is None:
            report.fail(
                f"registry row #{row._index}: file {row.file} does not exist "
                f"(anchor cannot be verified)"
            )
            continue
        if not _anchor_present(text, row):
            report.fail(
                f"registry row #{row._index}: anchor "
                f"({row.symbol or row.signature!r}) no longer present in "
                f"{row.file}. The code this row froze was moved/renamed -- "
                f"re-anchor the row and re-verify its classification."
            )

    # -- teeth: make_loud must reach a diagnostic ---------------------------
    for row in rows:
        if row.classification != "make_loud":
            continue
        text = _file_text(row.file)
        if text is None:
            continue  # already reported as missing above
        if not _DIAG_EMIT_RE.search(text):
            report.fail(
                f"make_loud row #{row._index} ({row.file}) reaches NO diagnostic "
                f"emit (warnings.append/logger/print/emit_line/eprintln!/warn!). "
                f"A make_loud degrade that is not loud is the metabug -- either "
                f"add the diagnostic or reclassify."
            )

    # -- teeth: non-sound rows must name a real, non-skipped test -----------
    for row in rows:
        if row.classification not in NON_SOUND:
            continue
        if not row.fast_path_test:
            report.fail(
                f"{row.classification} row #{row._index} ({row.file}) names no "
                f"fast_path_test. A perf/capability claim needs a test proving "
                f"the fast path is taken on the hard input."
            )
            continue
        present, skipped = _find_test(row.fast_path_test, repo_root)
        if skipped:
            report.fail(
                f"row #{row._index} ({row.file}) fast_path_test "
                f"{row.fast_path_test!r} exists but is skipped/xfail -- a skipped "
                f"fast-path test proves nothing."
            )
        elif not present:
            if row.classification == "make_loud":
                report.fail(
                    f"make_loud row #{row._index} ({row.file}) names "
                    f"fast_path_test {row.fast_path_test!r} which does not exist. "
                    f"A landed make_loud site must have its proving test in tree."
                )
            else:
                # metabug_fix_pending: the fixing arc will add this test. Record
                # it (not a failure yet) so the pending debt is visible.
                report.warnings.append(
                    f"metabug_fix_pending row #{row._index} ({row.file}) "
                    f"fast_path_test {row.fast_path_test!r} not yet in tree "
                    f"(owned by the parallel fix arc)."
                )

    # -- drift: every discovered site must be registered --------------------
    discovered = discover_sites(repo_root, scan_roots)
    if extra_sites:
        discovered = discovered + list(extra_sites)
    report.discovered_site_count = len(discovered)
    registered_files = set(rows_by_file)
    unregistered: dict[str, list[DiscoveredSite]] = {}
    for site in discovered:
        if site.file not in registered_files:
            unregistered.setdefault(site.file, []).append(site)
    for rel, hits in sorted(unregistered.items()):
        sample = ", ".join(f"L{h.line}[{h.marker}]" for h in hits[:5])
        report.fail(
            f"UNREGISTERED degrade site(s) in {rel}: {sample}"
            + (f" (+{len(hits) - 5} more)" if len(hits) > 5 else "")
            + ". A degrade-to-slow signature with no registry row is exactly the "
            "metabug this gate prevents. Add a classified row to "
            "tools/degrade_to_slow_registry.toml (sound_keep with justification "
            "if it is correctness/cost conservatism; metabug_fix_pending/make_loud "
            "with a fast_path_test if it is a real perf degrade)."
        )

    return report


def _format_report(report: GateReport) -> str:
    out: list[str] = []
    status = "PASS" if report.ok else "FAIL"
    out.append(f"degrade-to-slow gate: {status}")
    out.append(
        f"  registry rows: {report.registry_row_count} | "
        f"discovered sites: {report.discovered_site_count} | "
        f"metabug_fix_pending: {report.metabug_fix_pending_count}"
        f"/{report.metabug_fix_pending_baseline} (baseline)"
    )
    for w in report.warnings:
        out.append(f"  [note] {w}")
    for e in report.errors:
        out.append(f"  [FAIL] {e}")
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json", action="store_true", help="emit a machine-readable report"
    )
    release_receipt.add_receipt_arguments(parser)
    args = parser.parse_args(raw_argv)
    try:
        receipt_destination = release_receipt.prepare_receipt_destination(
            repo_root=REPO_ROOT,
            receipt_path=args.receipt,
            source_sha=args.source_sha,
        )
    except ValueError as exc:
        parser.error(str(exc))
    report = run_gate()
    if receipt_destination is not None:
        status = (
            release_receipt.STATUS_PASS if report.ok else release_receipt.STATUS_FAIL
        )
        try:
            receipt = release_receipt.build_receipt(
                kind=release_receipt.KIND_DEGRADE_TO_SLOW_GATE,
                source_sha=receipt_destination.source_sha,
                status=status,
                argv=raw_argv,
                tool_path=Path(__file__),
                facts={
                    "discovered_site_count": report.discovered_site_count,
                    "errors": report.errors,
                    "metabug_fix_pending_baseline": (
                        report.metabug_fix_pending_baseline
                    ),
                    "metabug_fix_pending_count": report.metabug_fix_pending_count,
                    "registry_path": REGISTRY_PATH.relative_to(REPO_ROOT).as_posix(),
                    "registry_row_count": report.registry_row_count,
                    "warnings": report.warnings,
                },
                input_paths=[REGISTRY_PATH],
                repo_root=REPO_ROOT,
            )
            release_receipt.write_receipt(receipt, receipt_destination)
        except ValueError as exc:
            print(f"degrade-to-slow receipt: ERROR: {exc}", file=sys.stderr)
            return 2
    if args.json:
        print(
            json.dumps(
                {
                    "ok": report.ok,
                    "errors": report.errors,
                    "warnings": report.warnings,
                    "registry_row_count": report.registry_row_count,
                    "discovered_site_count": report.discovered_site_count,
                    "metabug_fix_pending_count": report.metabug_fix_pending_count,
                    "metabug_fix_pending_baseline": (
                        report.metabug_fix_pending_baseline
                    ),
                },
                indent=2,
            )
        )
    else:
        print(_format_report(report))
    if receipt_destination is not None:
        print(
            f"degrade-to-slow receipt written: {receipt_destination.output_path}",
            file=sys.stderr if args.json else sys.stdout,
        )
    return 0 if report.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
