"""Teeth tests for `tools/parity_microscope.py` (the E1 first-divergence scope).

The numpy/scipy-touching proofs run once in a `uv run --no-project --with
numpy==2.5.1 --with scipy==1.18.0` child (the Kernel A witness stack, per
`field_solve_gates.json` provenance) via
`tests/helpers/parity_microscope_scenarios.py`, mirroring
`tests/tools/test_pact_parity_engine.py` — the base dev group carries no
numpy/scipy, so suite collection stays environment-independent.

The battery proves, with injected-divergence teeth (fails-pre/passes-post):

* drift gate: the staged pipeline is bit-identical to `field_solve()` on the
  real 384x512 fixture (the microscope can never silently fork from the
  kernel it instruments);
* a clean self-compare reports zero divergence;
* an injected single-ulp flip at m_smooth[200,256] is localized to exactly
  that stage AND element, with an exact 1-ulp histogram;
* an injected 1-ulp gaussian-weight drift (the libm divergence model) is
  caught at `w_gauss_s2` while every final output stays bit-identical — the
  measured basis of the E1 feasibility verdict;
* `final` mode (the wasm-candidate surface) maps a whole-field m_smooth ulp
  flip to the correct DAG frontier and the shared engine fails exactly
  `crit_min_rc` (the 630-tie keep-cut of 006 §gates);
* the margins certificate reproduces the fixture's documented tie structure.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import json
import re
import shutil
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCENARIOS_SCRIPT = ROOT / "tests" / "helpers" / "parity_microscope_scenarios.py"
MICROSCOPE = ROOT / "tools" / "parity_microscope.py"

# The Kernel A witness stack (field_solve_gates.json provenance line).
_UV_WITNESS_STACK = ["--with", "numpy==2.5.1", "--with", "scipy==1.18.0"]


def _require_uv() -> None:
    if shutil.which("uv") is None:
        pytest.skip(
            "uv is not on PATH; cannot provision the numpy/scipy child interpreter"
        )


@pytest.fixture(scope="module")
def scenario_results(
    tmp_path_factory: pytest.TempPathFactory,
) -> dict[str, dict[str, object]]:
    _require_uv()
    workdir = tmp_path_factory.mktemp("parity_microscope_scenarios")
    result = run_guarded_test_process(
        [
            "uv",
            "run",
            "--no-project",
            "--python",
            "3.12",
            *_UV_WITNESS_STACK,
            "python",
            str(SCENARIOS_SCRIPT),
            str(workdir),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=600,
    )
    match = re.search(r"^RESULT_JSON: (.+)$", result.stdout, flags=re.MULTILINE)
    assert match, (
        "microscope scenario battery did not print a RESULT_JSON line "
        f"(rc={result.returncode}):\nSTDOUT:\n{result.stdout}\n"
        f"STDERR:\n{result.stderr}"
    )
    return json.loads(match.group(1))


# Mirrors tests/helpers/parity_microscope_scenarios.py::SCENARIOS exactly —
# duplicated as a literal list (not imported, to stay numpy-free in-process)
# so a scenario silently vanishing from the battery is a loud KeyError here.
_TEETH = [
    "staged_matches_kernel",
    "self_compare_clean",
    "single_ulp_localized",
    "weight_ulp_first_stage",
    "final_mode_frontier",
    "margins_reports",
]


@pytest.mark.parametrize("name", _TEETH)
def test_microscope_teeth(
    name: str, scenario_results: dict[str, dict[str, object]]
) -> None:
    assert name in scenario_results, f"scenario {name!r} did not run at all"
    outcome = scenario_results[name]
    assert outcome["ok"], outcome["detail"]


def test_battery_covers_exactly_the_declared_teeth(
    scenario_results: dict[str, dict[str, object]],
) -> None:
    assert set(scenario_results) == set(_TEETH)


def test_microscope_stage_registry_is_numpy_free_to_read() -> None:
    """The stage table is auditable without executing numpy code paths.

    Guard the invariants the E1 feasibility doc cites: the pipeline order
    starts at the fixture, funnels every libm hazard through the gaussian
    weight stages + kappa pow, and ends at dist.
    """
    text = MICROSCOPE.read_text(encoding="utf-8")
    assert '("lstar", "input fixture (uint8 class map)", "exact")' in text
    assert text.index('"w_gauss_s2"') < text.index('"m_smooth"')
    assert text.index('"m_smooth"') < text.index('"crit_min_rc"')
    assert text.count("LIBM-HAZARD") == 3  # w_gauss_s2, w_gauss_s15, kappa pow
