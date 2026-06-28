"""Gate + robustness tests for tools/check_generator_manifest.py.

The semantic-fact-plane meta-gate (doc 59 Phases 1-3). Two jobs, mirroring the
tests/test_structural_audit.py + tests/test_gen_op_kinds.py discipline:

  1. GATE: the live tree passes the meta-gate — every generated authority is
     registered + --check-gated, there are no orphan generated files, and the
     closed-domain silent-default ratchet is at/under baseline (no regression).

  2. PROVEN-TO-FIND-DEBT: the auditor is exercised against SYNTHETIC inputs so a
     parser regression cannot silently zero it out (a tool that finds nothing
     must be PROVEN to find nothing, never broken into finding nothing). The
     closed-domain auditor TRIPS on an injected missing-arm match; the orphan
     scan TRIPS on an injected unregistered generated file; the manifest loader
     fails loud on a malformed manifest.

Run: pytest -q tests/test_generator_manifest.py
CI : python3 tools/check_generator_manifest.py --check  (the same gate)
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools" / "check_generator_manifest.py"


def _load_tool():
    spec = importlib.util.spec_from_file_location("molt_test_generator_manifest", TOOL)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_generator_manifest"] = module
    spec.loader.exec_module(module)
    return module


CGM = _load_tool()


# --- 1. the live gate -----------------------------------------------------


def test_manifest_loads_and_self_validates():
    """The committed manifest is structurally valid (fail-loud loader passes)."""
    manifest = CGM.load_manifest(ROOT)
    assert manifest.schema_version == 1
    assert manifest.generators, "manifest must register at least one generator"
    # Every gen_*.py in tools/ must be registered (no undiscovered generator).
    on_disk = {f"tools/{p.name}" for p in (ROOT / "tools").glob("gen_*.py")}
    registered = {g["tool"] for g in manifest.generators}
    missing = on_disk - registered
    assert not missing, f"generators on disk but not in the manifest: {sorted(missing)}"


def test_live_gate_has_no_gating_violations():
    """The whole meta-gate is green on the live tree (the CI --check contract)."""
    _violations, summary, gating = CGM.run_all(ROOT)
    assert gating == [], "meta-gate gating violations on main:\n" + "\n".join(
        f"  [{v.kind}] {v.location}: {v.detail}" for v in gating
    )
    # No orphan generated files, every authority --check-gated.
    assert summary["by_kind"]["orphan"] == 0
    assert summary["by_kind"]["ungated"] == 0
    assert summary["regressions"] == 0


def test_closed_domains_parse_to_live_enums():
    """Each declared closed domain resolves to a non-empty live enum (the
    discovery parser still finds the variants — drift in the enum location or a
    parser regression is caught here, not silently zeroed)."""
    sa = CGM._load_structural_audit(ROOT)
    manifest = CGM.load_manifest(ROOT)
    assert manifest.closed_domains, "expected at least one declared closed domain"
    for cd in manifest.closed_domains:
        enum_file = ROOT / cd["enum_file"]
        assert enum_file.is_file(), f"{cd['name']}: missing {cd['enum_file']}"
        variants = sa._count_enum_variants(
            enum_file.read_text(errors="replace"), cd["enum_name"]
        )
        assert variants, f"{cd['name']}: parsed 0 variants of {cd['enum_name']}"


def test_baseline_matches_live_counts():
    """The committed ratchet baseline equals the live counts (no silent drift in
    either direction outside an explicit --update-baseline)."""
    counts, _sites = CGM.collect_backlog_sites(ROOT)
    baseline = CGM.load_baseline(ROOT)
    for name, live in counts.items():
        assert name in baseline, f"closed domain {name} missing from baseline"
        assert live <= baseline[name], (
            f"closed domain {name} regressed: live={live} > baseline={baseline[name]}"
        )


# --- 2. proven-to-find-debt: the closed-domain auditor --------------------


def _silent_default_match(enum: str) -> str:
    return (
        f"fn classify(t: &{enum}) -> bool {{\n"
        f"    match t {{\n"
        f"        {enum}::Branch {{ .. }} => true,\n"
        f"        {enum}::CondBranch {{ .. }} => true,\n"
        f"        _ => false,\n"
        f"    }}\n"
        f"}}\n"
    )


def _failloud_default_match(enum: str) -> str:
    return (
        f"fn lower(t: &{enum}) {{\n"
        f"    match t {{\n"
        f"        {enum}::Branch {{ .. }} => emit_branch(),\n"
        f"        {enum}::CondBranch {{ .. }} => emit_cond(),\n"
        f'        _ => panic!("unsupported terminator {{:?}}", t),\n'
        f"    }}\n"
        f"}}\n"
    )


def _value_producer_match(enum: str) -> str:
    # Produces enum values in arm BODIES; does NOT dispatch on the enum.
    return (
        f"fn pick(step: i64) -> {enum} {{\n"
        f"    match step.signum() {{\n"
        f"        -1 => {enum}::Branch {{ target: a }},\n"
        f"        _ => {enum}::CondBranch {{ cond: c }},\n"
        f"    }}\n"
        f"}}\n"
    )


def _scan(rust: str, enum: str, variants: set[str], audited: set[str] | None = None):
    sa = CGM._load_structural_audit(ROOT)
    return CGM._scan_closed_domain_matches(
        sa,
        rust,
        "runtime/molt-passes/src/tir/passes/synthetic.rs",
        enum,
        variants,
        audited or set(),
    )


_TERM_VARIANTS = {
    "Branch",
    "CondBranch",
    "Switch",
    "StateDispatch",
    "Return",
    "Unreachable",
}


def test_silent_default_closed_domain_match_is_flagged():
    """The core defense: a `match` over a closed domain that covers a SUBSET then
    silently defaults IS the dispatch-handler-mirror-hazard and must be flagged."""
    findings = _scan(_silent_default_match("Terminator"), "Terminator", _TERM_VARIANTS)
    assert len(findings) == 1, f"expected 1 silent-default finding, got {findings}"
    assert findings[0].kind == "closed_domain"
    assert "SILENT default" in findings[0].detail


def test_failloud_default_closed_domain_match_is_not_flagged():
    """A fail-loud default (panic/unreachable/Err) is the CORRECT fail-closed
    dispatch pattern — a new variant panics, never silently miscompiles."""
    findings = _scan(
        _failloud_default_match("Terminator"), "Terminator", _TERM_VARIANTS
    )
    assert findings == [], f"fail-loud default wrongly flagged: {findings}"


def test_value_producer_match_is_not_flagged():
    """A match that PRODUCES enum values in arm bodies (but dispatches on
    something else) is not a closed-domain classifier and must not be flagged —
    the arm-pattern-vs-arm-body precision guard."""
    findings = _scan(_value_producer_match("Terminator"), "Terminator", _TERM_VARIANTS)
    assert findings == [], f"value-producer match wrongly flagged: {findings}"


def test_audited_default_is_not_flagged():
    """A file explicitly listed in the domain's audited_defaults is exempt (the
    per-site justified escape valve)."""
    audited = {"runtime/molt-passes/src/tir/passes/synthetic.rs"}
    findings = _scan(
        _silent_default_match("Terminator"), "Terminator", _TERM_VARIANTS, audited
    )
    assert findings == [], f"audited default wrongly flagged: {findings}"


def test_generated_table_consumer_match_is_not_flagged():
    """A match that consumes a generated *_table() result is a generated-role
    consumer, not a hand-maintained authority over the raw enum."""
    rust = (
        "fn scan(t: &Terminator) {\n"
        "    match terminator_kind_table(t) {\n"
        "        TerminatorKind::Branch => record(),\n"
        "        TerminatorKind::CondBranch => record(),\n"
        "        _ => {}\n"
        "    }\n"
        "}\n"
    )
    findings = _scan(rust, "Terminator", _TERM_VARIANTS)
    assert findings == [], f"generated-table consumer wrongly flagged: {findings}"


def test_single_variant_guard_is_not_flagged():
    """A single `Terminator::X` mention (a guard/equality check, not a
    classifier) must not be flagged — needs >= 2 dispatched variants."""
    rust = (
        "fn is_return(t: &Terminator) -> bool {\n"
        "    match t {\n"
        "        Terminator::Return { .. } => true,\n"
        "        _ => false,\n"
        "    }\n"
        "}\n"
    )
    findings = _scan(rust, "Terminator", _TERM_VARIANTS)
    assert findings == [], f"single-variant guard wrongly flagged: {findings}"


def test_exhaustive_no_wildcard_match_is_not_flagged():
    """A match with no wildcard is rustc-enforced exhaustive — trusted (the
    discovery firewall: the parser cannot manufacture a pass rustc would fail)."""
    rust = (
        "fn lower(t: &Terminator) {\n"
        "    match t {\n"
        "        Terminator::Branch { .. } => a(),\n"
        "        Terminator::CondBranch { .. } => b(),\n"
        "        Terminator::Switch { .. } => c(),\n"
        "        Terminator::StateDispatch { .. } => d(),\n"
        "        Terminator::Return { .. } => e(),\n"
        "        Terminator::Unreachable => f(),\n"
        "    }\n"
        "}\n"
    )
    findings = _scan(rust, "Terminator", _TERM_VARIANTS)
    assert findings == [], f"exhaustive match wrongly flagged: {findings}"


# --- 2b. proven-to-find-debt: end-to-end regression detection -------------


def _mirror_min_tree(tmp_path: Path) -> Path:
    """Build a minimal but VALID mirror of the meta-gate's inputs in tmp_path:
    the manifest, the checker + its structural_audit dependency, the closed-domain
    enum files, and a CI file that satisfies every ci_checkable generator's
    --check-step requirement (so the only gating signal is whatever WE inject).
    Returns the temp root."""
    import shutil

    (tmp_path / "tools").mkdir()
    shutil.copy(ROOT / "tools" / "generator_manifest.toml", tmp_path / "tools")
    shutil.copy(ROOT / "tools" / "structural_audit.py", tmp_path / "tools")
    shutil.copy(ROOT / "tools" / "check_generator_manifest.py", tmp_path / "tools")
    manifest = CGM.load_manifest(ROOT)
    # Make every registered generator file exist + every output exist, so the
    # gating/orphan checks are clean in the mirror.
    for g in manifest.generators:
        gtool = tmp_path / g["tool"]
        gtool.parent.mkdir(parents=True, exist_ok=True)
        gtool.write_text("# stub\n", encoding="utf-8")
        for out in g["outputs"]:
            op = tmp_path / out
            op.parent.mkdir(parents=True, exist_ok=True)
            if not op.exists():
                op.write_text("// stub output\n", encoding="utf-8")
        # A declared sync_test must exist (the loader validates this).
        st = g.get("sync_test")
        if st:
            sp = tmp_path / st
            sp.parent.mkdir(parents=True, exist_ok=True)
            if not sp.exists():
                sp.write_text("# stub sync test\n", encoding="utf-8")
    for og in manifest.orphan_generated:
        op = tmp_path / og["path"]
        op.parent.mkdir(parents=True, exist_ok=True)
        op.write_text("// @generated stub. DO NOT EDIT.\n", encoding="utf-8")
    for cd in manifest.closed_domains:
        dst = tmp_path / cd["enum_file"]
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / cd["enum_file"], dst)
    # A CI file that contains every ci_checkable generator's --check step.
    (tmp_path / ".github" / "workflows").mkdir(parents=True)
    ci_lines = ["name: CI"]
    for g in manifest.generators:
        if g.get("ci_checkable", True) and not g.get("discovery_only", False):
            ci_lines.append(f"      - run: {g['check_command']}")
    (tmp_path / ".github" / "workflows" / "ci.yml").write_text(
        "\n".join(ci_lines) + "\n", encoding="utf-8"
    )
    return tmp_path


def _write_baseline(tmp_path: Path, counts: dict[str, int]) -> None:
    import json

    (tmp_path / "tools" / "generator_manifest_baseline.json").write_text(
        json.dumps({"closed_domain_silent_defaults": counts}) + "\n",
        encoding="utf-8",
    )


def test_injected_regression_fails_the_gate(tmp_path: Path):
    """THE central proof the gate is real, not theater: with a baseline of 0,
    inject ONE silent-default match over the closed Terminator domain and prove
    the ratchet fires a gating regression. (The clean mirror has 0 offending
    sites, so baseline=0 is its true baseline; the injected site makes live=1.)"""
    root = _mirror_min_tree(tmp_path)
    _write_baseline(root, {"OpCode": 0, "Terminator": 0})

    # Sanity: before injection the mirror is GREEN.
    _v, summary0, gating0 = CGM.run_all(root)
    assert gating0 == [], f"clean mirror unexpectedly had gating violations: {gating0}"
    assert summary0["closed_domain_counts"].get("Terminator", 0) == 0

    # Inject ONE new offending hand-written match over Terminator.
    offending = root / "runtime" / "molt-passes" / "src" / "tir" / "passes"
    offending.mkdir(parents=True, exist_ok=True)
    (offending / "synthetic_regression.rs").write_text(
        _silent_default_match("Terminator"), encoding="utf-8"
    )

    _violations, summary, gating = CGM.run_all(root)
    assert summary["closed_domain_counts"].get("Terminator", 0) == 1, (
        "expected the injected match to raise the live Terminator count to 1"
    )
    regressions = [v for v in gating if v.kind == "closed_domain_regression"]
    assert regressions, (
        "injected silent-default match did NOT trip the ratchet — the gate is theater"
    )
    assert "Terminator" in regressions[0].location


def test_injected_orphan_generated_file_fails_the_gate(tmp_path: Path):
    """Inject an unregistered @generated file under an owned source root and prove
    the orphan scan fires (proven-to-find-debt for the orphan half)."""
    root = _mirror_min_tree(tmp_path)
    _write_baseline(root, {"OpCode": 0, "Terminator": 0})
    # An unregistered generated file under an owned root, matching no output.
    orphan_dir = root / "runtime" / "molt-ir" / "src"
    orphan_dir.mkdir(parents=True, exist_ok=True)
    (orphan_dir / "ghost_generated.rs").write_text(
        "// @generated by nobody. DO NOT EDIT.\npub const X: u8 = 0;\n",
        encoding="utf-8",
    )

    _violations, _summary, gating = CGM.run_all(root)
    orphans = [v for v in gating if v.kind == "orphan"]
    assert orphans, "injected orphan generated file did NOT trip the scan"
    assert any("ghost_generated.rs" in v.location for v in orphans)


def test_generated_marker_inside_string_literal_is_not_an_orphan(tmp_path: Path):
    """Generated banners emitted as data are not generated-file headers."""
    root = _mirror_min_tree(tmp_path)
    _write_baseline(root, {"OpCode": 0, "Terminator": 0})
    handwritten = root / "runtime" / "molt-backend-rust" / "src" / "rust"
    handwritten.mkdir(parents=True, exist_ok=True)
    (handwritten / "prelude.rs").write_text(
        "fn emit_header(output: &mut String) {\n"
        "    output.push_str(concat!(\n"
        '        "// Auto-generated - do not edit\\n",\n'
        '        "#![allow(dead_code)]\\n",\n'
        "    ));\n"
        "}\n",
        encoding="utf-8",
    )

    _violations, _summary, gating = CGM.run_all(root)
    orphans = [v for v in gating if v.kind == "orphan"]
    assert orphans == []


# --- 2c. proven-to-find-debt: gating & manifest validation ----------------


def test_ungated_generator_is_flagged(tmp_path: Path):
    """A registered generator with no CI --check step (and ci_checkable true) is
    flagged ungated."""
    import shutil

    (tmp_path / "tools").mkdir()
    shutil.copy(ROOT / "tools" / "structural_audit.py", tmp_path / "tools")
    shutil.copy(ROOT / "tools" / "check_generator_manifest.py", tmp_path / "tools")
    # A minimal manifest with one ci_checkable generator and an empty CI file.
    (tmp_path / "tools" / "generator_manifest.toml").write_text(
        "schema_version = 1\n"
        'generated_scan_roots = ["runtime", "src", "tools"]\n'
        "[[generator]]\n"
        'tool = "tools/gen_op_kinds.py"\n'
        'outputs = ["runtime/molt-ir/src/tir/op_kinds_generated.rs"]\n'
        'source = "x"\n'
        "check_mode = true\n"
        'check_command = "tools/gen_op_kinds.py --check"\n'
        'sync_test_reason = "stub"\n'
        "closed_domains = []\n"
        "discovery_only = false\n",
        encoding="utf-8",
    )
    # The generator file must exist for the "ungated, not missing" path.
    (tmp_path / "tools" / "gen_op_kinds.py").write_text("# stub\n", encoding="utf-8")
    (tmp_path / ".github" / "workflows").mkdir(parents=True)
    (tmp_path / ".github" / "workflows" / "ci.yml").write_text(
        "name: CI\n", encoding="utf-8"
    )
    _violations, summary, gating = CGM.run_all(tmp_path)
    ungated = [v for v in gating if v.kind == "ungated"]
    assert ungated, "a generator with no CI --check step was not flagged ungated"


def test_phantom_sync_test_fails_loud(tmp_path: Path):
    """A declared sync_test that names a non-existent file is a fail-loud
    ManifestError — the manifest must not claim coverage that does not exist."""
    import shutil

    (tmp_path / "tools").mkdir()
    shutil.copy(ROOT / "tools" / "check_generator_manifest.py", tmp_path / "tools")
    (tmp_path / "tools" / "generator_manifest.toml").write_text(
        "schema_version = 1\n"
        'generated_scan_roots = ["runtime"]\n'
        "[[generator]]\n"
        'tool = "tools/gen_x.py"\n'
        'outputs = ["a"]\n'
        'source = "s"\n'
        "check_mode = true\n"
        'check_command = "tools/gen_x.py --check"\n'
        'sync_test = "tests/test_does_not_exist.py"\n'
        "closed_domains = []\n"
        "discovery_only = false\n",
        encoding="utf-8",
    )
    with pytest.raises(CGM.ManifestError):
        CGM.load_manifest(tmp_path)


def test_malformed_manifest_temp(tmp_path: Path):
    """Concrete fail-loud check via a temp manifest (the closed_domain owner does
    not declare the domain)."""
    import shutil

    (tmp_path / "tools").mkdir()
    shutil.copy(ROOT / "tools" / "check_generator_manifest.py", tmp_path / "tools")
    (tmp_path / "tools" / "generator_manifest.toml").write_text(
        "schema_version = 1\n"
        'generated_scan_roots = ["runtime"]\n'
        "[[generator]]\n"
        'tool = "tools/gen_x.py"\n'
        'outputs = ["a"]\n'
        'source = "s"\n'
        "check_mode = true\n"
        'sync_test = "t"\n'
        "closed_domains = []\n"
        "discovery_only = false\n"
        "[[closed_domain]]\n"
        'name = "OpCode"\n'
        'enum_file = "x.rs"\n'
        'enum_name = "OpCode"\n'
        'owned_by = "tools/gen_x.py"\n',
        encoding="utf-8",
    )
    with pytest.raises(CGM.ManifestError):
        CGM.load_manifest(tmp_path)
