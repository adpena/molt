"""Contract tests for board history + the board-vs-history regression gate (doc 64 Phase 4).

Proves the second Performance-Constitution triage axis is GATEABLE: a
previously-green cell that regressed FAILs the gate; a within-CI noise delta does
NOT (the no-false-positive gate); a non-authoritative board never becomes a
baseline (Rule 2). SYNTHETIC boards only — no molt rebuild.
"""

from __future__ import annotations

from tests.tools.perf_scoreboard_fixtures import (
    synthetic_workload,
)

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import perf_board as pb  # noqa: E402
from tools import perf_history as ph  # noqa: E402
from tools import perf_schema as schema  # noqa: E402


def _cell(**overrides: object) -> dict[str, object]:
    cell: dict[str, object] = {
        "workload": synthetic_workload(),
        "benchmark": "tests/benchmarks/bench_fib.py",
        "target": "native",
        "backend": "native",
        "profile": "release-fast",
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "cold_molt_s": 0.10,
        "cold_cpython_s": 0.20,
        "warm_molt_s": 0.10,
        "warm_cpython_s": 0.20,
        "warm_speedup": 2.0,
        "cold_speedup": 2.0,
        "startup_tax_ms": 5.0,
        "verdict": schema.VERDICT_GREEN,
        "binary_size_kib": 512.0,
        "molt_peak_rss_mib": 18.0,
        "compile_time_s": 0.4,
        "stable": True,
        "measured_quiescent": True,
        "repeat_passes": 5,
        "repeat_ci_lo": 1.5,
        "repeat_ci_hi": 2.5,
        "repeat_stability": "STABLE_ABOVE",
        "pypy_ratio": None,
        "codon_ratio": None,
        "codon_equivalent": None,
        "cpython_peak_rss_mib": 15.0,
        "output_parity": schema.output_parity_evidence(
            reference_observations=[("cpython:cold", "result\n", "", 0)],
            molt_observations=[("molt:cold", "result\n", "", 0)],
        ),
        "log_artifact": "bench/scoreboard/logs/fib.log",
        "classification": schema.CLASS_GREEN,
    }
    cell.update(overrides)
    if "workload" not in overrides:
        cell["workload"] = synthetic_workload(cell["benchmark"])
    return cell


def _source_doc(
    cells: list[dict[str, object]],
    *,
    authoritative: bool = True,
    git_rev: str = "a" * 40,
) -> dict:
    nested: dict = {}
    for c in cells:
        (
            nested.setdefault(c["benchmark"], {})
            .setdefault(c["target"], {})
            .setdefault(c["backend"], {})
        )[c["profile"]] = c
    return {
        "schema_version": schema.SCHEMA_VERSION,
        "kind": "cpython_floor_scoreboard",
        "generated_at": f"2026-06-24T00:00:0{git_rev[0]}+00:00",
        "git_rev": git_rev,
        "provenance": {
            "authoritative": authoritative,
            "methodology_schema": "molt-perf-methodology-v1",
            "methodology_version": 1,
            "methodology_sha256": "d" * 64,
            "methodology_locked": True,
        },
        "host": {
            "platform": "linux",
            "arch": "x86_64",
            "pointer_bits": 64,
            "cpython_baseline": "3.14.5",
        },
        "scoreboard": nested,
    }


# --- Board identity ---------------------------------------------------------


def test_identity_class_stable_across_revs_same_tool_suite_host() -> None:
    a = pb.project_all(_source_doc([_cell()], git_rev="a" * 40))["cpython"]
    b = pb.project_all(_source_doc([_cell()], git_rev="b" * 40))["cpython"]
    # Same tool + suite + host => same comparability class (so they CAN be
    # regression-compared) even though git_rev differs.
    assert ph.identity_class(a) == ph.identity_class(b)
    # But the full board_identity differs (content-addressed by rev).
    assert ph.board_identity(a) != ph.board_identity(b)


def test_history_keeps_old_methodology_bytes_out_of_new_baseline(
    tmp_path: Path,
) -> None:
    hist = tmp_path / "history"
    old_doc = _source_doc([_cell()], authoritative=True)
    old_doc["provenance"]["methodology_version"] = 9
    old = pb.project_all(old_doc)["cpython"]
    ph.record_board(old, history_dir=hist)
    assert (
        ph.latest_authoritative_baseline(
            "cpython", ph.identity_class(old), history_dir=hist
        )
        is not None
    )
    retained = {path: path.read_bytes() for path in hist.rglob("*.json")}

    new_doc = _source_doc([_cell()], authoritative=True)
    new_doc["provenance"]["methodology_version"] = 10
    new = pb.project_all(new_doc)["cpython"]
    assert ph.identity_class(old) != ph.identity_class(new)
    assert (
        ph.latest_authoritative_baseline(
            "cpython", ph.identity_class(new), history_dir=hist
        )
        is None
    )
    assert retained and all(
        path.read_bytes() == content for path, content in retained.items()
    )


def test_identity_class_differs_across_host() -> None:
    a = pb.project_all(_source_doc([_cell()]))["cpython"]
    b_doc = _source_doc([_cell()])
    b_doc["host"]["platform"] = "win32"
    b = pb.project_all(b_doc)["cpython"]
    assert ph.identity_class(a) != ph.identity_class(b)


# --- Record + baseline retrieval --------------------------------------------


def test_only_authoritative_boards_become_baselines(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    non_auth = pb.project_all(_source_doc([_cell()], authoritative=False))["cpython"]
    ph.record_board(non_auth, history_dir=hist)
    # A non-authoritative board is recorded but NEVER selected as a baseline.
    baseline = ph.latest_authoritative_baseline(
        "cpython", ph.identity_class(non_auth), history_dir=hist
    )
    assert baseline is None

    auth = pb.project_all(_source_doc([_cell()], authoritative=True, git_rev="c" * 40))[
        "cpython"
    ]
    ph.record_board(auth, history_dir=hist)
    baseline = ph.latest_authoritative_baseline(
        "cpython", ph.identity_class(auth), history_dir=hist
    )
    assert baseline is not None
    assert baseline["git_rev"] == "c" * 40


# --- The regression gate (the headline deliverable) -------------------------


def test_regression_gate_fails_on_previously_green_cpython_red(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    # Baseline: an authoritative all-green board.
    baseline_boards = pb.project_all(
        _source_doc([_cell()], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # Candidate: a different rev, the same cell flipped CPython-RED.
    cand_doc = _source_doc(
        [_cell(warm_speedup=0.75, verdict=schema.VERDICT_FAIL_ENGINE)],
        authoritative=True,
        git_rev="d" * 40,
    )
    cand_boards = pb.project_all(cand_doc)

    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is True
    assert any("previously-green" in e for e in report["errors"])
    assert any("[cpython]" in e for e in report["errors"])


def test_regression_gate_silent_on_within_threshold_noise(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    baseline_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=2.0)], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # 2% slower, still well above the floor: NOT a regression (no false positive).
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=1.96)], authoritative=True, git_rev="d" * 40)
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is False
    assert report["errors"] == []
    # 2% < DRIFT_WARN_FRACTION (5%) => not even a warn.
    assert report["warnings"] == []


def test_regression_gate_warns_on_material_but_passing_drift(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    baseline_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=2.0)], authoritative=True, git_rev="a" * 40)
    )
    for board in baseline_boards.values():
        ph.record_board(board, history_dir=hist)

    # 20% slower but STILL passing the floor: WARN drift, not a gate-blocking error.
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=1.60)], authoritative=True, git_rev="d" * 40)
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    assert report["gate_fails"] is False  # still passing -> does not block
    assert any("drift" in w for w in report["warnings"])


def test_regression_gate_no_baseline_is_not_a_failure(tmp_path: Path) -> None:
    hist = tmp_path / "history"  # empty
    cand_boards = pb.project_all(
        _source_doc([_cell(warm_speedup=0.5, verdict=schema.VERDICT_FAIL_ENGINE)])
    )
    report = ph.regression_gate(cand_boards, history_dir=hist)
    # No baseline to regress against => the regression gate is silent (the
    # ABSOLUTE-floor gate, perf_board, is what catches a brand-new red).
    assert report["gate_fails"] is False


def test_record_is_idempotent_by_identity(tmp_path: Path) -> None:
    hist = tmp_path / "history"
    board = pb.project_all(_source_doc([_cell()], git_rev="a" * 40))["cpython"]
    ph.record_board(board, history_dir=hist)
    ph.record_board(board, history_dir=hist)  # same identity -> replaces, no dup
    index = ph._load_index_at("cpython", hist)
    assert len(index) == 1


def test_full_cli_chain_project_promote_gate(tmp_path: Path, monkeypatch) -> None:
    """Exercise the CLI surface: write a source board, project it, promote the
    authoritative baseline, then gate a regressed candidate -> exit 1."""
    import json

    monkeypatch.setattr(ph, "_revision_is_on_origin_main", lambda _rev: True)

    src = tmp_path / "cpython_source.json"
    src.write_text(
        json.dumps(_source_doc([_cell()], authoritative=True, git_rev="a" * 40)),
        encoding="utf-8",
    )
    board_dir = tmp_path / "boards"
    hist = tmp_path / "history"

    # project + record the baseline (no-gate: all green anyway)
    rc = pb.main([str(src), "--out-dir", str(board_dir), "--no-gate", "--quiet"])
    assert rc == 0
    rc = ph.main(
        [
            str(board_dir / "cpython_aaaaaaaaaaaa.json"),
            "--promote",
            "--history-dir",
            str(hist),
        ]
    )
    assert rc == 0

    # candidate regressed
    cand_src = tmp_path / "cpython_cand.json"
    cand_src.write_text(
        json.dumps(
            _source_doc(
                [_cell(warm_speedup=0.7, verdict=schema.VERDICT_FAIL_ENGINE)],
                authoritative=True,
                git_rev="d" * 40,
            )
        ),
        encoding="utf-8",
    )
    cand_dir = tmp_path / "cand_boards"
    rc = pb.main([str(cand_src), "--out-dir", str(cand_dir), "--no-gate", "--quiet"])
    assert rc == 0
    rc = ph.main(
        [
            str(cand_dir / "cpython_dddddddddddd.json"),
            "--gate",
            "--history-dir",
            str(hist),
        ]
    )
    assert rc == 1  # the regression gate fails CI on the previously-green red


def test_promotion_refuses_boards_that_cannot_be_baselines(
    tmp_path: Path, monkeypatch
) -> None:
    import json

    on_main = {"a" * 40}
    monkeypatch.setattr(ph, "_revision_is_on_origin_main", lambda rev: rev in on_main)
    src = tmp_path / "source.json"
    src.write_text(
        json.dumps(_source_doc([_cell()], authoritative=True, git_rev="a" * 40)),
        encoding="utf-8",
    )
    board_dir = tmp_path / "boards"
    assert pb.main([str(src), "--out-dir", str(board_dir), "--no-gate", "--quiet"]) == 0
    board = json.loads(
        (board_dir / "cpython_aaaaaaaaaaaa.json").read_text(encoding="utf-8")
    )
    assert ph.promotion_problems(board) == []

    off_main = {**board, "git_rev": "e" * 40}
    assert ph.promotion_problems(off_main) == [
        f"board revision {'e' * 12} is not on origin/main"
    ]
    unlocked = {
        **board,
        "provenance": {**board["provenance"], "methodology_locked": False},
    }
    assert "methodology content was not locked to its version" in (
        ph.promotion_problems(unlocked)
    )
    noisy = {**board, "provenance": {**board["provenance"], "authoritative": False}}
    assert "board is not authoritative" in ph.promotion_problems(noisy)

    path = tmp_path / "noisy.json"
    path.write_text(json.dumps(noisy), encoding="utf-8")
    hist = tmp_path / "history"
    assert ph.main([str(path), "--promote", "--history-dir", str(hist)]) == 2
    assert not hist.exists()


def test_unarmed_gate_is_reported_not_silently_passed(tmp_path: Path, capsys) -> None:
    import json

    src = tmp_path / "source.json"
    src.write_text(
        json.dumps(_source_doc([_cell()], authoritative=True, git_rev="a" * 40)),
        encoding="utf-8",
    )
    board_dir = tmp_path / "boards"
    assert pb.main([str(src), "--out-dir", str(board_dir), "--no-gate", "--quiet"]) == 0
    rc = ph.main(
        [
            str(board_dir / "cpython_aaaaaaaaaaaa.json"),
            "--gate",
            "--history-dir",
            str(tmp_path / "empty-history"),
        ]
    )
    assert rc == 0
    assert "UNARMED" in capsys.readouterr().err


def test_history_index_bytes_do_not_depend_on_the_clock(tmp_path: Path) -> None:
    board = pb.project_all(
        _source_doc([_cell()], authoritative=True, git_rev="a" * 40)
    )["cpython"]
    first, second = tmp_path / "first", tmp_path / "second"
    ph.record_board(board, history_dir=first)
    ph.record_board(board, history_dir=second)
    assert (first / "cpython" / "index.json").read_bytes() == (
        second / "cpython" / "index.json"
    ).read_bytes()


def _wasm_board(
    *, path="/opt/node", version="24.1.0", digest="a" * 64, revision="a" * 40
):
    from tests.tools.test_perf_schema import _node_observation

    engine = _node_observation(path=path, version=version, digest=digest)
    cell = _cell(backend="wasm", target="wasm", execution_host=engine)
    source = _source_doc([cell], git_rev=revision)
    source["host"]["execution_host"] = engine
    return pb.project_all(source)["cpython"]


def test_history_class_and_address_bind_node_bytes_and_version():
    a = _wasm_board()
    copied = _wasm_board(path="D:/node/copied.exe")
    copied["host"]["execution_host"]["identity"]["entrypoint"] = "copied.exe"
    copied["host"]["execution_host"]["identity"]["content_filename"] = "copied.exe"
    assert ph.identity_class(a) == ph.identity_class(copied)
    assert ph.board_identity(a) == ph.board_identity(copied)
    for different in (_wasm_board(digest="b" * 64), _wasm_board(version="24.2.0")):
        assert ph.identity_class(a) != ph.identity_class(different)
        assert ph.board_identity(a) != ph.board_identity(different)
        import pytest

        with pytest.raises(ValueError, match="not comparable"):
            ph.regressions_vs_baseline(a, different)


def test_history_does_not_record_or_compare_unbound_legacy_engine(tmp_path):
    import pytest

    board = _wasm_board()
    board["host"].pop("execution_host")
    with pytest.raises(ValueError, match="execution_host"):
        ph.record_board(board, history_dir=tmp_path)
    report = ph.regression_gate({"cpython": board}, history_dir=tmp_path)
    assert report["gate_fails"] and "execution host" in report["errors"][0]


def test_history_index_cannot_launder_changed_engine_bytes(tmp_path):
    import json

    board = _wasm_board()
    entry = ph.record_board(board, history_dir=tmp_path)
    path = tmp_path / entry.path
    forged = _wasm_board(digest="b" * 64)
    path.write_text(json.dumps(forged), encoding="utf-8")
    assert (
        ph.latest_authoritative_baseline(
            "cpython", ph.identity_class(board), history_dir=tmp_path
        )
        is None
    )


def test_history_uses_same_image_at_new_path_but_not_same_version_new_image(tmp_path):
    base = _wasm_board()
    ph.record_board(base, history_dir=tmp_path)
    relocated = _wasm_board(path="/other/node", revision="b" * 40)
    report = ph.regression_gate({"cpython": relocated}, history_dir=tmp_path)
    assert report["boards"]["cpython"]["baseline_git_rev"] == "a" * 40
    changed = _wasm_board(digest="b" * 64, revision="b" * 40)
    assert (
        ph.regression_gate({"cpython": changed}, history_dir=tmp_path)["boards"][
            "cpython"
        ]["baseline"]
        is None
    )


def test_history_identity_cannot_join_changed_or_missing_workloads():
    import copy
    import pytest

    board = pb.project_all(_source_doc([_cell()]))["cpython"]
    changed = copy.deepcopy(board)
    cells = ph._flatten_projected_cells(changed)
    cell = next(iter(cells.values()))
    cell["workload"]["sources"][0]["sha256"] = "b" * 64
    assert ph.identity_class(board) != ph.identity_class(changed)
    with pytest.raises(ValueError, match="workloads are not comparable"):
        ph.regressions_vs_baseline(changed, board)
    cell["workload"].clear()
    with pytest.raises(ValueError, match="workload"):
        ph.identity_class(changed)
