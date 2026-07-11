"""Teeth battery for `tools/parity_microscope.py` (the first-divergence scope).

Executed as a STANDALONE SCRIPT inside a `uv run --no-project --with
numpy==2.5.1 --with scipy==1.18.0 python ...` subprocess (see
`tests/tools/test_parity_microscope.py`) — never imported by the main pytest
process, mirroring `tests/helpers/pact_parity_scenarios.py` (the base dev
dependency group carries no numpy/scipy). The pins match the Kernel A witness
stack recorded in `field_solve_gates.json` provenance (numpy 2.5.1 /
scipy 1.18.0).

Each scenario proves a with-teeth property of the microscope:

* the staged pipeline is BIT-IDENTICAL to the real `field_solve()` kernel on
  the real fixture (drift gate — the microscope can never silently fork from
  the kernel it instruments);
* a clean self-compare reports zero divergence;
* an INJECTED single-ulp divergence is localized to exactly the perturbed
  stage and element index (fails-pre/passes-post: the injection flips the
  verdict);
* a 1-ulp weight (libm-model) injection is caught at `w_gauss_s2` even though
  every FINAL output stays bit-identical — the empirical basis of the
  E1 feasibility claim that f32 output rounding absorbs multi-ulp weight
  drift;
* `final` mode (the wasm-candidate surface) maps a whole-field m_smooth ulp
  flip onto the correct frontier and the shared gate engine fails exactly
  `crit_min_rc` (the 630-tie keep-cut, per 006 §gates).
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import sys
import traceback
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[2]
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

import numpy as np  # noqa: E402


def _import_by_path(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None, path
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


scope = _import_by_path("parity_microscope", _ROOT / "tools" / "parity_microscope.py")
field_solve_mod = _import_by_path(
    "pact_field_solve_for_scenarios",
    _ROOT / "collab" / "pact" / "pact_witness_kernel" / "field_solve.py",
)
make_fixture = _import_by_path(
    "pact_make_fixture_for_scenarios",
    _ROOT / "collab" / "pact" / "pact_witness_kernel" / "make_fixture.py",
)


def _run_cli(*argv: str) -> tuple[int, str]:
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        try:
            rc = scope.main(list(argv))
        except SystemExit as exc:  # argparse error paths
            rc = int(exc.code or 0)
    return int(rc), buf.getvalue()


def _bit_identical(a: np.ndarray, b: np.ndarray) -> bool:
    return a.shape == b.shape and a.dtype == b.dtype and a.tobytes() == b.tobytes()


class Scenarios:
    """Shared state: one fixture + one baseline staged run for all scenarios."""

    def __init__(self, workdir: Path) -> None:
        self.workdir = workdir
        self.fixture = workdir / "lstar_sample.npz"
        np.savez_compressed(self.fixture, lstar=make_fixture.make_lstar())
        self.base_stages = workdir / "stages-base.npz"
        self.base_final = workdir / "final-base.npz"
        rc, out = _run_cli(
            "run",
            "--fixture",
            str(self.fixture),
            "--out",
            str(self.base_stages),
            "--final-out",
            str(self.base_final),
        )
        assert rc == 0, out

    def scenario_staged_matches_kernel(self) -> None:
        """Drift gate: staged pipeline == field_solve(), bit-for-bit, all 11."""
        lstar = np.load(self.fixture)["lstar"]
        kernel_out = field_solve_mod.field_solve(lstar)
        staged = np.load(self.base_final)
        assert set(staged.files) == set(kernel_out), (
            set(staged.files),
            set(kernel_out),
        )
        for key, want in kernel_out.items():
            got = staged[key]
            assert _bit_identical(got, np.asarray(want)), (
                f"staged pipeline drifted from field_solve() on {key!r}: "
                f"shape {got.shape}/{np.asarray(want).shape} dtype "
                f"{got.dtype}/{np.asarray(want).dtype}"
            )

    def scenario_self_compare_clean(self) -> None:
        rc, out = _run_cli("compare", str(self.base_stages), str(self.base_stages))
        assert rc == 0, out
        assert "ALL STAGES BIT-IDENTICAL" in out, out

    def scenario_single_ulp_localized(self) -> None:
        """Inject +1 ulp at m_smooth[200, 256]; the scope must name stage+index."""
        pert = self.workdir / "stages-1ulp.npz"
        rc, out = _run_cli(
            "run",
            "--fixture",
            str(self.fixture),
            "--out",
            str(pert),
            "--perturb",
            "m_smooth=1@200,256",
        )
        assert rc == 0, out
        rc, out = _run_cli("compare", str(pert), str(self.base_stages))
        assert rc == 1, f"injected divergence must flip the verdict:\n{out}"
        assert "FIRST-DIVERGENT-STAGE: m_smooth" in out, out
        assert "at (200, 256):" in out, out
        assert "1/196608 elements differ" in out, out
        # ulp histogram must attribute exactly one 1-ulp flip.
        assert "=1:1" in out and "max=1" in out, out

    def scenario_weight_ulp_first_stage(self) -> None:
        """1-ulp weight (libm-model) drift: caught at w_gauss_s2; finals identical."""
        stages = self.workdir / "stages-w1.npz"
        final = self.workdir / "final-w1.npz"
        rc, out = _run_cli(
            "run",
            "--fixture",
            str(self.fixture),
            "--out",
            str(stages),
            "--final-out",
            str(final),
            "--perturb",
            "w_gauss_s2=1",
        )
        assert rc == 0, out
        rc, out = _run_cli("compare", str(stages), str(self.base_stages))
        assert rc == 1, out
        assert "FIRST-DIVERGENT-STAGE: w_gauss_s2" in out, out
        # The E1 feasibility keystone: f32 output rounding absorbs 1-ulp
        # weight drift COMPLETELY — every final output stays bit-identical.
        rc, out = _run_cli("final", str(final), str(self.base_final))
        assert rc == 0, out
        assert "ALL FINAL OUTPUTS BIT-IDENTICAL" in out, out

    def scenario_final_mode_frontier(self) -> None:
        """Whole-field 1-ulp m_smooth flip: frontier=m_smooth, crit_min_rc FAILs."""
        stages = self.workdir / "stages-ms1.npz"
        final = self.workdir / "final-ms1.npz"
        rc, out = _run_cli(
            "run",
            "--fixture",
            str(self.fixture),
            "--out",
            str(stages),
            "--final-out",
            str(final),
            "--perturb",
            "m_smooth=1",
        )
        assert rc == 0, out
        rc, out = _run_cli("final", str(final), str(self.base_final))
        assert rc == 1, out
        assert (
            "FRONTIER (earliest ops that introduced divergence): ['m_smooth']" in out
        ), out
        assert "FAIL crit_min_rc" in out, out
        # Everything except the 630-tie keep-cut must still pass its gate.
        for key in (
            "sdf_argmax",
            "boundary",
            "crit_max_rc",
            "crit_saddle_rc",
            "crit_saddle_eigvec",
            "curvature",
            "dist",
        ):
            assert f"PASS {key}" in out, (key, out)

    def scenario_margins_reports(self) -> None:
        rc, out = _run_cli("margins", str(self.base_stages))
        assert rc == 0, out
        assert "keep-120 cut:" in out, out
        # The fixture's documented sharp edge (006 §gates: 630/672 tied).
        assert "tied_at_cut=630" in out, out
        assert "cut lands INSIDE an exact-tie group" in out, out
        assert "[saddle_eigvec] eigh 2x2" in out, out


SCENARIOS = [
    "staged_matches_kernel",
    "self_compare_clean",
    "single_ulp_localized",
    "weight_ulp_first_stage",
    "final_mode_frontier",
    "margins_reports",
]


def main() -> int:
    workdir = Path(sys.argv[1]).resolve()
    workdir.mkdir(parents=True, exist_ok=True)
    shared = Scenarios(workdir)
    results: dict[str, dict[str, object]] = {}
    for name in SCENARIOS:
        try:
            getattr(shared, f"scenario_{name}")()
            results[name] = {"ok": True, "detail": ""}
        except BaseException:  # noqa: BLE001 - report, never abort the battery
            results[name] = {"ok": False, "detail": traceback.format_exc()}
    print(f"RESULT_JSON: {json.dumps(results)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
