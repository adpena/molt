"""Tests for tools/build_graph_audit.py -- the recompile blast-radius ratchet.

The unit tests construct SYNTHETIC declared/measured graphs in-process (no cargo)
so the blast-radius math, the layer back-edge detector, and the --check ratchet
are proven deterministically and fast. The integration test asserts the real
workspace is clean (zero back-edges) and the committed baseline is consistent
with `cargo metadata`; it is skipped if cargo is unavailable.

The headline falsification (doc 56 §5 "real gate, not theater"): a synthetic
re-coupling edge that re-couples a low layer to a high one must (a) be flagged as
a layer back-edge and (b) widen the offending crate's blast radius, and
`check_against_baseline` must report it as a regression. `test_recoupling_*`
prove exactly that.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import build_graph_audit as bga  # noqa: E402


# ---------------------------------------------------------------------------
# Synthetic-graph fixtures
# ---------------------------------------------------------------------------


def _declared(layers: dict[str, int], allowed=()) -> bga.DeclaredGraph:
    crates = {
        name: bga.DeclaredCrate(name=name, layer=layer, role="")
        for name, layer in layers.items()
    }
    return bga.DeclaredGraph(
        crates=crates,
        allowed_same_layer_edges=frozenset(allowed),
    )


def _measured(crates, edges) -> bga.MeasuredGraph:
    return bga.MeasuredGraph(
        crates=frozenset(crates),
        edges=tuple(bga.Edge(src=s, dst=d, kind=k) for s, d, k in edges),
    )


# A tiny but representative DAG mirroring the real layering shape:
#   ir(L0) <- passes(L1) <- tir(L2) <- {native,wasm}(L3) <- backend(L4)
#   codegen-abi(L0) consumed by native+wasm.
_CLEAN_LAYERS = {
    "ir": 0,
    "codegen-abi": 0,
    "passes": 1,
    "tir": 2,
    "native": 3,
    "wasm": 3,
    "backend": 4,
}
_CLEAN_EDGES = [
    ("passes", "ir", "normal"),
    ("tir", "passes", "normal"),
    ("tir", "ir", "normal"),
    ("native", "tir", "normal"),
    ("native", "codegen-abi", "normal"),
    ("wasm", "tir", "normal"),
    ("wasm", "codegen-abi", "normal"),
    ("backend", "native", "normal"),
    ("backend", "wasm", "normal"),
    ("backend", "ir", "normal"),
]


def _clean_report() -> bga.GraphReport:
    return bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, _CLEAN_EDGES))


# ---------------------------------------------------------------------------
# Blast-radius math
# ---------------------------------------------------------------------------


def test_blast_radius_is_reverse_transitive_closure():
    report = _clean_report()
    # ir is depended on (transitively) by passes, tir, native, wasm, backend.
    assert report.blast_radius["ir"] == 5
    assert set(report.downstream["ir"]) == {
        "passes",
        "tir",
        "native",
        "wasm",
        "backend",
    }
    # codegen-abi feeds native+wasm+backend (backend via native/wasm).
    assert report.blast_radius["codegen-abi"] == 3
    assert set(report.downstream["codegen-abi"]) == {"native", "wasm", "backend"}
    # a single backend is a leaf consumed only by the driver.
    assert report.blast_radius["native"] == 1
    assert report.downstream["native"] == ["backend"]
    # the driver is the top of the DAG: nothing depends on it.
    assert report.blast_radius["backend"] == 0


def test_clean_graph_has_no_back_edges():
    report = _clean_report()
    assert report.back_edges == []
    metrics = bga.ratchet_metrics(report)
    assert metrics["crate_layer_backedges"] == 0
    assert metrics["max_crate_blast_radius"] == 5
    assert metrics["undeclared_crates"] == 0


def test_dev_edges_excluded_from_blast_radius_but_checked_for_layering():
    # A dev edge from the driver back down to ir does NOT widen ir's lib cone
    # (dev deps rebuild only the dependent's test target).
    edges = _CLEAN_EDGES + [("backend", "ir", "dev")]
    report = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    assert report.blast_radius["ir"] == 5  # unchanged
    assert report.back_edges == []  # backend(L4)->ir(L0) is a legal downward dev edge

    # But a dev edge that points UPWARD (ir depends on backend for tests) is a
    # back-edge even though it does not enter the lib cone -- a test-only edge
    # must never seed a production cycle (21f risk register).
    up = _CLEAN_EDGES + [("ir", "backend", "dev")]
    report_up = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, up))
    assert report_up.blast_radius["backend"] == 0  # lib cone unchanged
    assert [be.src for be in report_up.back_edges] == ["ir"]
    assert report_up.back_edges[0].dst == "backend"
    assert report_up.back_edges[0].kind == "dev"


# ---------------------------------------------------------------------------
# Layer back-edge detection
# ---------------------------------------------------------------------------


def test_upward_edge_is_a_back_edge():
    # The canonical re-coupling: ir(L0) depends on backend(L4).
    edges = _CLEAN_EDGES + [("ir", "backend", "normal")]
    report = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    assert len(report.back_edges) == 1
    be = report.back_edges[0]
    assert (be.src, be.dst) == ("ir", "backend")
    assert be.src_layer == 0 and be.dst_layer == 4
    assert "STRICTLY LOWER" in be.describe()


def test_same_layer_edge_is_a_back_edge_unless_whitelisted():
    # Two siblings at L3 must be independent.
    edges = _CLEAN_EDGES + [("native", "wasm", "normal")]
    report = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    assert [(be.src, be.dst) for be in report.back_edges] == [("native", "wasm")]

    # ...but the documented exception is permitted.
    declared = _declared(_CLEAN_LAYERS, allowed=[("native", "wasm")])
    report_ok = bga.analyze(declared, _measured(_CLEAN_LAYERS, edges))
    assert report_ok.back_edges == []


def test_downward_edge_is_legal():
    # backend(L4)->ir(L0) is already in the clean graph and is legal.
    report = _clean_report()
    srcs = {be.src for be in report.back_edges}
    assert "backend" not in srcs


# ---------------------------------------------------------------------------
# The --check ratchet (the falsification)
# ---------------------------------------------------------------------------


def test_check_passes_against_matching_baseline():
    report = _clean_report()
    baseline = bga.baseline_payload(report)
    outcome = bga.check_against_baseline(report, baseline)
    assert outcome.ok
    assert outcome.regressions == []


def test_recoupling_trips_the_ratchet_on_every_axis():
    """The headline real-gate proof: baseline is the clean graph; a re-coupling
    edge fails --check with the back-edge named AND the widened radius."""
    clean = _clean_report()
    baseline = bga.baseline_payload(clean)

    # Now re-couple ir -> backend and check against the clean baseline.
    edges = _CLEAN_EDGES + [("ir", "backend", "normal")]
    recoupled = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    outcome = bga.check_against_baseline(recoupled, baseline)

    assert not outcome.ok
    blob = "\n".join(outcome.regressions)
    # (a) the offending edge is named
    assert "LAYER BACK-EDGE: ir (L0) -> backend (L4)" in blob
    # (b) the scalar ratchet regressed
    assert "crate_layer_backedges: 0 -> 1" in blob
    # (c) the per-crate blast radius widened (backend gains a downstream cone)
    assert "BLAST RADIUS WIDENED: backend" in blob


def test_recoupling_widens_radius_even_without_beating_global_max():
    """A sibling coupling that does NOT beat the global max blast radius is still
    caught by the per-crate radius baseline -- the key anti-silent-regression
    property (global max alone would miss it)."""
    clean = _clean_report()
    baseline = bga.baseline_payload(clean)
    assert baseline["metrics"]["max_crate_blast_radius"] == 5

    # native(L3) -> wasm(L3): wasm's cone grows from {backend} to {backend,native}
    # (radius 1 -> 2), which is < the global max of 5, so max_crate_blast_radius
    # does NOT regress -- only the per-crate check catches it.
    edges = _CLEAN_EDGES + [("native", "wasm", "normal")]
    recoupled = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    metrics = bga.ratchet_metrics(recoupled)
    assert metrics["max_crate_blast_radius"] == 5  # unchanged
    assert recoupled.blast_radius["wasm"] == 2  # widened from 1

    outcome = bga.check_against_baseline(recoupled, baseline)
    assert not outcome.ok
    blob = "\n".join(outcome.regressions)
    assert "BLAST RADIUS WIDENED: wasm: 1 -> 2" in blob
    assert "LAYER BACK-EDGE: native (L3) -> wasm (L3)" in blob


def test_check_reports_improvement_when_radius_shrinks():
    # Baseline has the clean graph; removing native->codegen-abi shrinks abi's cone.
    clean = _clean_report()
    baseline = bga.baseline_payload(clean)
    edges = [e for e in _CLEAN_EDGES if e != ("native", "codegen-abi", "normal")]
    shrunk = bga.analyze(_declared(_CLEAN_LAYERS), _measured(_CLEAN_LAYERS, edges))
    outcome = bga.check_against_baseline(shrunk, baseline)
    assert outcome.ok  # shrinking is always allowed
    assert any("codegen-abi" in imp for imp in outcome.improvements)


def test_undeclared_crate_is_a_regression():
    # A workspace crate missing from the declared graph cannot be layer-checked.
    layers = dict(_CLEAN_LAYERS)
    del layers["wasm"]  # forget to declare wasm
    report = bga.analyze(_declared(layers), _measured(_CLEAN_LAYERS, _CLEAN_EDGES))
    assert report.undeclared_crates == ["wasm"]
    assert bga.ratchet_metrics(report)["undeclared_crates"] == 1
    # undeclared crate's edges are NOT flagged as back-edges (reported separately).
    assert all(be.src != "wasm" and be.dst != "wasm" for be in report.back_edges)

    baseline = bga.baseline_payload(_clean_report())
    outcome = bga.check_against_baseline(report, baseline)
    assert not outcome.ok
    assert any("UNDECLARED CRATE: wasm" in r for r in outcome.regressions)


# ---------------------------------------------------------------------------
# Fail-loud behaviors
# ---------------------------------------------------------------------------


def test_unexpected_dependency_kind_fails_loud():
    with pytest.raises(bga.BuildGraphError, match="unexpected dependency kind"):
        bga._normalize_kind("frobnicate")


def test_null_kind_is_normal():
    assert bga._normalize_kind(None) == "normal"
    assert bga._normalize_kind("build") == "build"
    assert bga._normalize_kind("dev") == "dev"


def test_bad_layer_in_toml_fails_loud(tmp_path):
    toml = tmp_path / "runtime" / "crate_graph.toml"
    toml.parent.mkdir(parents=True)
    toml.write_text(
        'schema_version = 1\n[[crate]]\nname = "x"\nlayer = -1\n', encoding="utf-8"
    )
    with pytest.raises(bga.BuildGraphError, match="invalid layer"):
        bga.load_declared_graph(tmp_path)


def test_duplicate_crate_in_toml_fails_loud(tmp_path):
    toml = tmp_path / "runtime" / "crate_graph.toml"
    toml.parent.mkdir(parents=True)
    toml.write_text(
        "schema_version = 1\n"
        '[[crate]]\nname = "x"\nlayer = 0\n'
        '[[crate]]\nname = "x"\nlayer = 1\n',
        encoding="utf-8",
    )
    with pytest.raises(bga.BuildGraphError, match="duplicate crate"):
        bga.load_declared_graph(tmp_path)


def test_missing_toml_fails_loud(tmp_path):
    with pytest.raises(bga.BuildGraphError, match="declared crate graph not found"):
        bga.load_declared_graph(tmp_path)


def test_parse_measured_graph_drops_external_and_self_edges():
    metadata = {
        "workspace_members": ["a 0.1.0 (path+file:///a)", "b 0.1.0 (path+file:///b)"],
        "packages": [
            {
                "id": "a 0.1.0 (path+file:///a)",
                "name": "a",
                "dependencies": [
                    {"name": "b", "kind": None},
                    {"name": "serde", "kind": None},  # external -> dropped
                    {"name": "a", "kind": "dev"},  # self -> dropped
                ],
            },
            {
                "id": "b 0.1.0 (path+file:///b)",
                "name": "b",
                "dependencies": [],
            },
        ],
    }
    measured = bga.parse_measured_graph(metadata)
    assert measured.crates == frozenset({"a", "b"})
    assert measured.edges == (bga.Edge(src="a", dst="b", kind="normal"),)


# ---------------------------------------------------------------------------
# Integration: the real workspace + committed baseline
# ---------------------------------------------------------------------------


@pytest.mark.skipif(shutil.which("cargo") is None, reason="cargo not available")
def test_real_workspace_has_no_back_edges_and_baseline_matches():
    report = bga.build_report(REPO_ROOT)
    # The real tree must be clean: the 21b/21f decomposition is acyclic-layered.
    assert report.back_edges == [], [be.describe() for be in report.back_edges]
    assert report.undeclared_crates == [], report.undeclared_crates
    assert report.stale_declarations == [], report.stale_declarations

    # The committed baseline must match the live graph (so --check is green and
    # the baseline is not stale).
    baseline_path = REPO_ROOT / bga.BASELINE_PATH_REL
    assert baseline_path.is_file(), "run --update-baseline to pin the baseline"
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    outcome = bga.check_against_baseline(report, baseline)
    assert outcome.ok, outcome.regressions


@pytest.mark.skipif(shutil.which("cargo") is None, reason="cargo not available")
def test_cli_check_exits_zero_on_clean_tree():
    proc = subprocess.run(
        [sys.executable, str(TOOLS_ROOT / "build_graph_audit.py"), "--check"],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr
    assert "build-graph ratchet OK" in proc.stdout


# ---------------------------------------------------------------------------
# ci_gate wiring (the discoverability/wired contract)
# ---------------------------------------------------------------------------


def _load_ci_gate():
    if str(TOOLS_ROOT) not in sys.path:
        sys.path.insert(0, str(TOOLS_ROOT))
    src = REPO_ROOT / "src"
    if str(src) not in sys.path:
        sys.path.insert(0, str(src))
    import ci_gate

    return ci_gate


def test_ci_gate_tier1_includes_build_graph_ratchet():
    module = _load_ci_gate()
    checks = {check.name: check for check in module._build_checks()}
    check = checks["build-graph-ratchet"]

    assert check.tier == 1
    assert check.required is True
    assert check.needs_pytest is False
    # A metadata-only check needs cargo PRESENT but must NOT take a compile slot
    # (needs_rust would starve it behind real builds).
    assert check.needs_rust is False
    assert check.needs_cargo is True
    assert str(module.TOOLS / "build_graph_audit.py") in check.cmd
    assert "--check" in check.cmd


def test_ci_gate_tier1_includes_build_graph_audit_contract():
    module = _load_ci_gate()
    checks = {check.name: check for check in module._build_checks()}
    check = checks["build-graph-audit-contract"]

    assert check.tier == 1
    assert check.required is True
    assert check.needs_pytest is True
    assert str(module.TESTS / "tools" / "test_build_graph_audit.py") in check.cmd


def test_run_check_does_not_acquire_compile_slot_for_needs_cargo(monkeypatch):
    """A needs_cargo check (cargo metadata) must NOT enter compile_slot, so it is
    never starved waiting for compile capacity behind real builds."""
    module = _load_ci_gate()
    slot_calls: list[dict[str, object]] = []

    def fake_compile_slot(**kwargs):
        slot_calls.append(kwargs)
        raise AssertionError("needs_cargo check must not acquire a compile slot")

    def fake_guarded_completed_process(command, **kwargs):
        return module.harness_memory_guard.GuardedCompletedProcess(
            command,
            0,
            "metadata ok\n",
            "",
            elapsed_s=0.1,
        )

    monkeypatch.setattr(module.compile_governor, "compile_slot", fake_compile_slot)
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = module._run_check(
        module.Check(
            name="cargo-metadata-check",
            tier=1,
            cmd=["cargo", "metadata"],
            needs_cargo=True,
        )
    )

    assert result.status == "pass"
    assert slot_calls == []  # never entered compile_slot
