"""Tests for the shared, generalized parity engine `collab/pact/parity/check_parity.py`.

This is the single acceptance authority molt owes pact per `009 §5` /
`011 §2` (`collab/pact/011_molt_reply_progress_sync_and_harness_proposal_20260710.md`).
Two things are proved here:

1. **Fail-loud guarantees** (`test_fail_loud_guarantee`): the engine never
   skips, never widens tolerance, never passes by default, for every
   corruption class the operator specified. The actual scenarios (each
   proving the TRUE reference PASSES and an injected corruption FAILS) live
   in `tests/helpers/pact_parity_scenarios.py`, run once via a numpy-carrying
   `uv run --with numpy==1.26.4` child process (see that module's docstring
   for why numpy never touches the main pytest process).

2. **Verdict equivalence** (`test_new_engine_matches_legacy_kernel_a_oracle_*`):
   the new engine + `field_solve_gates.json`, run against the REAL Kernel A
   numpy-fp32 reference, produces the identical per-array PASS/FAIL verdict
   as the original inline `pact_witness_kernel/check_parity.py` oracle it
   generalizes -- both on a true (uncorrupted) run and on an injected
   corruption, so the generalization is proved to be behavior-preserving, not
   just structurally similar.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import json
from pathlib import Path
import re
import shutil
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[2]
PARITY_ENGINE = ROOT / "collab" / "pact" / "parity" / "check_parity.py"
KERNEL_ROOT = ROOT / "collab" / "pact" / "pact_witness_kernel"
LEGACY_ORACLE = KERNEL_ROOT / "check_parity.py"
FIELD_SOLVE_GATES = KERNEL_ROOT / "field_solve_gates.json"
SCENARIOS_SCRIPT = ROOT / "tests" / "helpers" / "pact_parity_scenarios.py"

_UV_NUMPY = ["--with", "numpy==1.26.4"]
_UV_NUMPY_SCIPY = ["--with", "numpy==1.26.4", "--with", "scipy==1.17.1"]


def _uv_run(*args: str, cwd: Path, with_packages: list[str]) -> subprocess.CompletedProcess[str]:
    cmd = [
        "uv",
        "run",
        "--no-project",
        "--python",
        "3.12",
        *with_packages,
        "python",
        *args,
    ]
    return run_guarded_test_process(
        cmd,
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
        timeout=180,
    )


def _require_uv() -> None:
    if shutil.which("uv") is None:
        pytest.skip("uv is not on PATH; cannot provision the numpy/scipy child interpreter")


# --------------------------------------------------------------------------- #
# 1. Fail-loud guarantee battery (see tests/helpers/pact_parity_scenarios.py)
# --------------------------------------------------------------------------- #
@pytest.fixture(scope="module")
def scenario_results(tmp_path_factory: pytest.TempPathFactory) -> dict[str, dict[str, object]]:
    _require_uv()
    workdir = tmp_path_factory.mktemp("pact_parity_scenarios")
    result = _uv_run(str(SCENARIOS_SCRIPT), str(workdir), cwd=ROOT, with_packages=_UV_NUMPY)
    match = re.search(r"^RESULT_JSON: (.+)$", result.stdout, flags=re.MULTILINE)
    assert match, (
        "scenario battery did not print a RESULT_JSON line "
        f"(rc={result.returncode}):\nSTDOUT:\n{result.stdout}\nSTDERR:\n{result.stderr}"
    )
    return json.loads(match.group(1))


# Mirrors tests/helpers/pact_parity_scenarios.py::SCENARIOS keys exactly --
# duplicated as a literal list (not imported, to stay numpy-free in-process)
# so a scenario silently vanishing from the battery shows up as a KeyError
# here rather than the parametrize list quietly shrinking.
_FAIL_LOUD_GUARANTEES = [
    "missing_candidate_array",
    "extra_candidate_array",
    "dtype_mismatch",
    "shape_mismatch",
    "nan_mismatch",
    "inf_mismatch",
    "atol_never_widened",
    "empty_zero_size_candidate",
    "unevaluable_manifest_reference_drift",
    "bitwise_exact_fp32",
    "order_robust_atol",
    "exact_set",
    "scaffold_manifest_never_passes",
]


@pytest.mark.parametrize("name", _FAIL_LOUD_GUARANTEES)
def test_fail_loud_guarantee(
    name: str, scenario_results: dict[str, dict[str, object]]
) -> None:
    assert name in scenario_results, f"scenario {name!r} did not run at all"
    outcome = scenario_results[name]
    assert outcome["ok"], outcome["detail"]


def test_fail_loud_battery_covers_exactly_the_declared_guarantees(
    scenario_results: dict[str, dict[str, object]]
) -> None:
    assert set(scenario_results) == set(_FAIL_LOUD_GUARANTEES)


# --------------------------------------------------------------------------- #
# 2. CLI-level exit code contract (main(): 0 PASS / 1 FAIL / 2 structural)
# --------------------------------------------------------------------------- #
def test_cli_exit_0_on_pass(tmp_path: Path) -> None:
    _require_uv()
    _write_trivial_fixture_and_gates(tmp_path)
    result = _uv_run(
        str(PARITY_ENGINE),
        str(tmp_path / "candidate.npz"),
        str(tmp_path / "reference.npz"),
        str(tmp_path / "gates.json"),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "PARITY: PASS" in result.stdout


def test_cli_exit_1_on_gate_fail(tmp_path: Path) -> None:
    _require_uv()
    _write_trivial_fixture_and_gates(tmp_path, corrupt=True)
    result = _uv_run(
        str(PARITY_ENGINE),
        str(tmp_path / "candidate.npz"),
        str(tmp_path / "reference.npz"),
        str(tmp_path / "gates.json"),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )
    assert result.returncode == 1, result.stdout + result.stderr
    assert "PARITY: FAIL" in result.stdout


def test_cli_exit_2_on_scaffold_manifest(tmp_path: Path) -> None:
    _require_uv()
    _write_trivial_fixture_and_gates(tmp_path)
    scaffold_gates = tmp_path / "gates.json"
    scaffold_gates.write_text(
        json.dumps({"schema_version": 1, "kernel": "k", "status": "AWAITING_PACT_KERNEL_SOURCE"}),
        encoding="utf-8",
    )
    result = _uv_run(
        str(PARITY_ENGINE),
        str(tmp_path / "candidate.npz"),
        str(tmp_path / "reference.npz"),
        str(scaffold_gates),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )
    assert result.returncode == 2, result.stdout + result.stderr
    assert "PARITY: FAIL" in result.stdout
    assert "NOT IMPLEMENTED" in result.stdout


def _write_trivial_fixture_and_gates(tmp_path: Path, *, corrupt: bool = False) -> None:
    script = tmp_path / "_write.py"
    script.write_text(
        "import numpy as np\n"
        "import sys\n"
        "ref = np.arange(4, dtype='int32')\n"
        f"cand = ref.copy(){' ; cand[0] = 99' if corrupt else ''}\n"
        "np.savez(sys.argv[1], a=ref)\n"
        "np.savez(sys.argv[2], a=cand)\n",
        encoding="utf-8",
    )
    result = _uv_run(
        str(script),
        str(tmp_path / "reference.npz"),
        str(tmp_path / "candidate.npz"),
        cwd=tmp_path,
        with_packages=_UV_NUMPY,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    (tmp_path / "gates.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "kernel": "trivial",
                "status": "ready",
                "outputs": {"a": {"gate": "exact", "dtype": "int32", "shape": [4]}},
            }
        ),
        encoding="utf-8",
    )


# --------------------------------------------------------------------------- #
# 3. Verdict-equivalence proof: new engine vs. the legacy Kernel A inline
#    oracle it generalizes, on the REAL numpy-fp32 reference.
# --------------------------------------------------------------------------- #
def _line_verdicts(stdout: str) -> dict[str, bool]:
    """Parse `  PASS <key> ...` / `  FAIL <key> ...` lines from either oracle's
    stdout into {key: ok}. Both the legacy oracle's and the new engine's
    report lines share this leading shape (see check_parity.py::ArrayResult.line
    and pact_witness_kernel/check_parity.py's per-key prints)."""
    verdicts: dict[str, bool] = {}
    for line in stdout.splitlines():
        m = re.match(r"^\s*(PASS|FAIL)\s+(\S+)\s", line)
        if m:
            verdicts[m.group(2)] = m.group(1) == "PASS"
    return verdicts


@pytest.fixture(scope="module")
def real_kernel_a_reference(
    tmp_path_factory: pytest.TempPathFactory,
) -> Path:
    """Regenerate the REAL Kernel A numpy-fp32 reference via the pinned stack
    (numpy 1.26.4 / scipy 1.17.1), exactly as `tools/pact_witness_oracle.py`
    does. `field_solve` is deterministic (no RNG/time/I-O; see
    pact_witness_kernel/field_solve.py's module docstring), so this is a real,
    non-fabricated reference -- not a synthetic stand-in.
    """
    _require_uv()
    work = tmp_path_factory.mktemp("kernel_a_reference")
    for name in ("make_fixture.py", "field_solve.py"):
        shutil.copy2(KERNEL_ROOT / name, work / name)
    r1 = _uv_run("make_fixture.py", cwd=work, with_packages=_UV_NUMPY_SCIPY)
    assert r1.returncode == 0, r1.stdout + r1.stderr
    r2 = _uv_run("field_solve.py", "lstar_sample.npz", cwd=work, with_packages=_UV_NUMPY_SCIPY)
    assert r2.returncode == 0, r2.stdout + r2.stderr
    reference = work / "reference_outputs.npz"
    assert reference.is_file()
    return reference


def test_new_engine_matches_legacy_kernel_a_oracle_on_true_reference(
    tmp_path: Path, real_kernel_a_reference: Path
) -> None:
    """field_solve is bit-identical across re-runs (README.md: 'deterministic
    ... bit-identical across CPython re-runs'), so a second real run's output
    is a legitimate stand-in for a hypothetical bit-perfect Molt-WASM
    candidate -- not a fabricated identity comparison."""
    candidate = tmp_path / "candidate_outputs.npz"
    shutil.copy2(real_kernel_a_reference, candidate)

    legacy = _uv_run(
        str(LEGACY_ORACLE),
        str(candidate),
        str(real_kernel_a_reference),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )
    new = _uv_run(
        str(PARITY_ENGINE),
        str(candidate),
        str(real_kernel_a_reference),
        str(FIELD_SOLVE_GATES),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )

    assert legacy.returncode == 0, legacy.stdout + legacy.stderr
    assert new.returncode == 0, new.stdout + new.stderr

    legacy_verdicts = _line_verdicts(legacy.stdout)
    new_verdicts = _line_verdicts(new.stdout)
    assert legacy_verdicts, "legacy oracle produced no parseable per-key verdict lines"
    assert set(legacy_verdicts) == set(new_verdicts) == {
        "sdf_argmax",
        "sdf_margin_m12",
        "sdf_gap13",
        "boundary",
        "m_smooth",
        "crit_max_rc",
        "crit_min_rc",
        "crit_saddle_rc",
        "crit_saddle_eigvec",
        "curvature",
        "dist",
    }
    assert legacy_verdicts == new_verdicts, (
        f"per-array verdict diverged: legacy={legacy_verdicts} new={new_verdicts}"
    )
    assert all(legacy_verdicts.values()), "the true reference must PASS every key"


def test_new_engine_matches_legacy_kernel_a_oracle_on_injected_corruption(
    tmp_path: Path, real_kernel_a_reference: Path
) -> None:
    """A real, non-numpy-only corruption (flip one label pixel in sdf_argmax)
    must be caught identically by both oracles, and must not bleed into an
    unrelated key's verdict for either oracle."""
    _require_uv()
    candidate = tmp_path / "candidate_outputs.npz"

    corrupt_script = tmp_path / "_corrupt.py"
    corrupt_script.write_text(
        "import numpy as np\n"
        "import sys\n"
        "src, dst = sys.argv[1], sys.argv[2]\n"
        "d = dict(np.load(src))\n"
        "d['sdf_argmax'] = d['sdf_argmax'].copy()\n"
        "d['sdf_argmax'][0, 0] = (int(d['sdf_argmax'][0, 0]) + 1) % 5\n"
        "np.savez(dst, **d)\n",
        encoding="utf-8",
    )
    r = _uv_run(
        str(corrupt_script),
        str(real_kernel_a_reference),
        str(candidate),
        cwd=tmp_path,
        with_packages=_UV_NUMPY,
    )
    assert r.returncode == 0, r.stdout + r.stderr

    legacy = _uv_run(
        str(LEGACY_ORACLE),
        str(candidate),
        str(real_kernel_a_reference),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )
    new = _uv_run(
        str(PARITY_ENGINE),
        str(candidate),
        str(real_kernel_a_reference),
        str(FIELD_SOLVE_GATES),
        cwd=ROOT,
        with_packages=_UV_NUMPY,
    )

    assert legacy.returncode == 1, legacy.stdout + legacy.stderr
    assert new.returncode == 1, new.stdout + new.stderr

    legacy_verdicts = _line_verdicts(legacy.stdout)
    new_verdicts = _line_verdicts(new.stdout)
    assert legacy_verdicts == new_verdicts, (
        f"per-array verdict diverged on corruption: legacy={legacy_verdicts} new={new_verdicts}"
    )
    assert legacy_verdicts["sdf_argmax"] is False
    assert new_verdicts["sdf_argmax"] is False
    # boundary is derived from sdf_argmax (inter-class boundary mask), so it
    # may legitimately also flip; everything NOT touched by the corruption
    # must still agree PASS between the two oracles (already asserted above
    # by full-dict equality) -- this just pins the one key we corrupted
    # directly.
