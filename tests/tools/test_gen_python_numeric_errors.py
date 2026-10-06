"""Closed numeric policy schema and reproducible consumer projections."""

from __future__ import annotations
from pathlib import Path
import sys
import json
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import gen_python_numeric_errors as policy  # noqa: E402
from tests.process_guard_common import run_guarded_test_process  # noqa: E402


def test_policy_coordinates_and_class_contract():
    # Independent CPython3.12.13/3.13.11/3.14.3 oracle observations; these are
    # conformance assertions, not generated aliases of the policy authority.
    observed_legacy = {
        "IntegerTrueDivision": ("division by zero", "division by zero"),
        "FloatTrueDivision": ("float division by zero", "float division by zero"),
        "ComplexTrueDivision": ("complex division by zero", "complex division by zero"),
        "IntegerFloorDivision": ("integer division or modulo by zero",) * 2,
        "FloatFloorDivision": ("float floor division by zero",) * 2,
        "IntegerModulo": ("integer modulo by zero",) * 2,
        "FloatModulo": ("float modulo", "float modulo by zero"),
        "IntegerDivmod": ("integer division or modulo by zero",) * 2,
        "FloatDivmod": ("float divmod()",) * 2,
        "NegativePower": ("0.0 cannot be raised to a negative power",) * 2,
        "ComplexNegativePower": ("0.0 to a negative or complex power",) * 2,
        "ModularPowerZero": ("pow() 3rd argument cannot be 0",) * 2,
    }
    observed_314 = {
        "NegativePower": "zero to a negative power",
        "ComplexNegativePower": "zero to a negative or complex power",
        "ModularPowerZero": "pow() 3rd argument cannot be 0",
    }
    rows = policy.load_policy()
    assert {row.name for row in rows} == set(observed_legacy)
    for row in rows:
        assert row.messages == (
            *observed_legacy[row.name],
            observed_314.get(row.name, "division by zero"),
        )
        assert row.error_class == (
            "ValueError" if row.name == "ModularPowerZero" else "ZeroDivisionError"
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "missing_coordinate",
        "unknown_coordinate",
        "duplicate",
        "missing_context",
        "unknown_kind",
        "unknown_class",
        "unknown_field",
    ],
)
def test_schema_rejects_incomplete_or_unadmitted_policy(tmp_path, mutation):
    source = policy.SOURCE.read_text(encoding="utf-8")
    if mutation == "missing_coordinate":
        source = source.replace('python_3_13 = "division by zero"', "", 1)
    elif mutation == "unknown_coordinate":
        source = source.replace('"3.14"', '"3.15"', 1)
    elif mutation == "duplicate":
        source += source[
            source.index("[[context]]") : source.index(
                "[[context]]", source.index("[[context]]") + 1
            )
        ]
    elif mutation == "missing_context":
        source = source[: source.rindex("[[context]]")]
    elif mutation == "unknown_kind":
        source = source.replace('operand_kind = "int"', 'operand_kind = "decimal"', 1)
    elif mutation == "unknown_class":
        source = source.replace(
            'error_class = "ZeroDivisionError"', 'error_class = "ValueError"', 1
        )
    else:
        source += '\nunknown_field = "unexpected"\n'
    path = tmp_path / "invalid.toml"
    path.write_text(source, encoding="utf-8")
    with pytest.raises(policy.SchemaError):
        policy.load_policy(path)


def test_generated_consumers_are_semantically_identical_and_idempotent():
    expected = policy.render(policy.load_policy())
    assert policy.render(policy.load_policy()) == expected
    for path in policy.OUTPUTS:
        assert path.read_text(encoding="utf-8") == policy.render(
            policy.load_policy(),
            integer_helper="molt-backend-luau" not in path.parts,
        ), path
    assert "_ => None" in expected
    assert "checked_div" in expected and "checked_rem" in expected


def test_compiled_policy_and_integer_sign_family(tmp_path):
    import shutil

    compiler = shutil.which("rustc")
    if compiler is None:
        pytest.skip("rustc unavailable: compiled policy coverage is unverified")
    signed = [
        (a, b)
        for a in (-(2**63), -7, -1, 0, 1, 7, 2**63 - 1)
        for b in (-(2**63), -3, -1, 1, 3, 2**63 - 1)
    ]
    checks = []
    for a, b in signed:
        q, r = divmod(a, b)
        checks.append(
            f"assert_eq!(python_integer_divmod({a}i128, {b}i128), Some(({q}i128, {r}i128)));"
        )
    for row in policy.load_policy():
        for minor, message in zip((12, 13, 14), row.messages):
            checks.append(
                f"assert_eq!(NumericErrorContext::{row.name}.message(3, {minor}), Some({json.dumps(message)}));"
            )
        for major, minor in ((2, 12), (3, 11), (3, 15), (4, 12)):
            checks.append(
                f"assert_eq!(NumericErrorContext::{row.name}.message({major}, {minor}), None);"
            )
    checks += [
        "assert_eq!(python_integer_divmod(1, 0), None);",
        "assert_eq!(python_integer_divmod(i128::MIN, -1), None);",
    ]
    source = tmp_path / "numeric_policy.rs"
    source.write_text(
        policy.render(policy.load_policy())
        + "\nfn main() {\n"
        + "\n".join(checks)
        + "\n}\n",
        encoding="utf-8",
    )
    binary = tmp_path / (
        "numeric_policy.exe" if sys.platform == "win32" else "numeric_policy"
    )
    run_guarded_test_process(
        [compiler, "--edition=2024", str(source), "-o", str(binary)],
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    )
    run_guarded_test_process(
        [str(binary)], check=True, capture_output=True, text=True, timeout=10
    )


def test_standalone_rust_emitted_arithmetic_signed_and_bool_cases(tmp_path):
    """Compile the real emitted arithmetic fragments without compiling Cargo."""
    import ast
    import shutil

    compiler = shutil.which("rustc")
    if compiler is None:
        pytest.skip("rustc unavailable: emitted arithmetic is unverified")
    emitter = (ROOT / "runtime/molt-backend-rust/src/rust/prelude.rs").read_text(
        encoding="utf-8"
    )
    fragments = []
    for helper in ("floor_div", "mod", "div", "pow"):
        start = emitter.index(f'        if used("molt_{helper}(")')
        end = emitter.index("\n        }", start)
        fragment = "".join(
            ast.literal_eval(line.strip().removesuffix(","))
            for line in emitter[start:end].splitlines()
            if line.lstrip().startswith('"')
        )
        fragments.append(fragment)
    start = emitter.index("fn molt_numeric_error(context:")
    end = emitter.index('"#', start)
    numeric_helpers = emitter[start:end]
    scaffold = r"""
#[derive(Clone)] enum MoltValue { Int(i64), Float(f64), Bool(bool) }
fn molt_int(v: &MoltValue) -> i64 { match v { MoltValue::Int(x)=>*x, MoltValue::Bool(x)=>i64::from(*x), MoltValue::Float(x)=>*x as i64 } }
fn molt_float(v: &MoltValue) -> f64 { match v { MoltValue::Float(x)=>*x, _=>molt_int(v) as f64 } }
fn molt_int_pow(a:i64,b:i64)->i64 { a.pow(b as u32) }
#[derive(Clone)] struct Target { major:i64, minor:i64 }
fn molt_sys_version_state()->&'static std::sync::Mutex<Target> { static STATE:std::sync::Mutex<Target>=std::sync::Mutex::new(Target{major:3,minor:12}); &STATE }
"""
    checks = []
    for a in (-(2**63), -7, -1, 0, 1, 7, 2**63 - 1):
        for b in (-3, -1, 1, 3):
            q, r = divmod(a, b)
            if -(2**63) <= q < 2**63:
                checks.append(
                    f"assert!(matches!(molt_floor_div(MoltValue::Int({a}), MoltValue::Int({b})), MoltValue::Int({q})));"
                )
            checks.append(
                f"assert!(matches!(molt_mod(MoltValue::Int({a}), MoltValue::Int({b})), MoltValue::Int({r})));"
            )
    checks += [
        "assert!(matches!(molt_floor_div(MoltValue::Int(-7), MoltValue::Bool(true)), MoltValue::Int(-7)));",
        "assert!(matches!(molt_mod(MoltValue::Bool(true), MoltValue::Int(-3)), MoltValue::Int(-2)));",
    ]
    for minor in (12, 13, 14):
        checks.append(f"molt_sys_version_state().lock().unwrap().minor = {minor};")
        for operation, context in (
            ("div", "FloatTrueDivision"),
            ("floor_div", "FloatFloorDivision"),
            ("mod", "FloatModulo"),
        ):
            row = next(row for row in policy.load_policy() if row.name == context)
            expected = json.dumps("ZeroDivisionError: " + row.messages[minor - 12])
            checks.append(
                f"let error = std::panic::catch_unwind(|| molt_{operation}(MoltValue::Float(7.0), MoltValue::Float(-0.0))).err().unwrap(); assert_eq!(error.downcast_ref::<String>().unwrap(), {expected});"
            )
        row = next(row for row in policy.load_policy() if row.name == "NegativePower")
        expected = json.dumps("ZeroDivisionError: " + row.messages[minor - 12])
        checks.append(
            f"let error = std::panic::catch_unwind(|| molt_pow(MoltValue::Int(0), MoltValue::Int(-1))).err().unwrap(); assert_eq!(error.downcast_ref::<String>().unwrap(), {expected});"
        )
    source = tmp_path / "emitted_numeric.rs"
    source.write_text(
        policy.render(policy.load_policy())
        + scaffold
        + numeric_helpers
        + "\n".join(fragments)
        + "\nfn main() {\n"
        + "\n".join(checks)
        + "\n}\n",
        encoding="utf-8",
    )
    binary = tmp_path / (
        "emitted_numeric.exe" if sys.platform == "win32" else "emitted_numeric"
    )
    result = run_guarded_test_process(
        [compiler, "--edition=2024", str(source), "-o", str(binary)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    result = run_guarded_test_process(
        [str(binary)], capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, result.stderr


def test_actual_timedelta_integer_normalizer_sign_range_and_overflow(tmp_path):
    import shutil

    compiler = shutil.which("rustc")
    if compiler is None:
        pytest.skip("rustc unavailable: exact timedelta normalization unverified")
    source = (ROOT / "runtime/molt-runtime-serial/src/datetime.rs").read_text(
        encoding="utf-8"
    )
    start = source.index("fn normalize_timedelta_us_exact(")
    # td_total_us float helper occurs between these; only the closed exact function.
    exact = source[start : source.index("\nfn td_total_us(", start)]
    cases = (
        -2,
        -1,
        0,
        1,
        2,
        -999_999_999 * 86_400_000_000,
        1_000_000_000 * 86_400_000_000 - 1,
    )
    checks = []
    for value in cases:
        day, residue = divmod(value, 86_400_000_000)
        second, microsecond = divmod(residue, 1_000_000)
        checks.append(
            f"assert_eq!(normalize_timedelta_us_exact({value}i128), Some(({day}, {second}, {microsecond})));"
        )
    for value in (-999_999_999 * 86_400_000_000 - 1, 1_000_000_000 * 86_400_000_000):
        checks.append(f"assert_eq!(normalize_timedelta_us_exact({value}i128), None);")
    checks += [
        "assert_eq!(normalize_timedelta_us_exact(i128::MIN), None);",
        "assert_eq!(normalize_timedelta_us_exact(i128::MAX), None);",
    ]
    path = tmp_path / "td_normalizer.rs"
    path.write_text(
        exact + "\nfn main() {\n" + "\n".join(checks) + "\n}\n", encoding="utf-8"
    )
    binary = tmp_path / (
        "td_normalizer.exe" if sys.platform == "win32" else "td_normalizer"
    )
    result = run_guarded_test_process(
        [compiler, "--edition=2024", str(path), "-o", str(binary)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    result = run_guarded_test_process(
        [str(binary)], capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, result.stderr


def test_compiled_float_divmod_against_host_cpython(tmp_path):
    """Exact finite/signed-zero/infinity bits; NaNs have no stable payload oracle."""
    import itertools
    import math
    import shutil
    import struct

    compiler = shutil.which("rustc")
    if compiler is None:
        pytest.skip("rustc unavailable: float divmod execution unverified")
    if sys.version_info[:2] not in ((3, 12), (3, 13), (3, 14)):
        pytest.skip("host CPython is outside declared numeric oracle coordinates")

    def bits(value):
        return struct.unpack(">Q", struct.pack(">d", value))[0]

    def classify(value):
        return "nan" if math.isnan(value) else struct.pack(">d", value).hex()

    values = (
        0.0,
        -0.0,
        1.0,
        -1.0,
        7.0,
        -7.0,
        0.1,
        -0.1,
        1e-300,
        -1e-300,
        1e300,
        -1e300,
        float("inf"),
        float("-inf"),
        float("nan"),
    )
    floats = list(itertools.product(values, repeat=2))
    expected = []
    for a, b in floats:
        try:
            q, r = divmod(a, b)
            expected.append("F:" + classify(q) + ":" + classify(r))
        except ZeroDivisionError:
            expected.append("F:ZERO")
    scaffold = 'fn classify(v:f64)->String { if v.is_nan() { "nan".into() } else { format!("{:016x}",v.to_bits()) } }\nfn main() {\n'
    scaffold += (
        "let floats:&[(u64,u64)] = &["
        + ",".join(f"({bits(a)}u64,{bits(b)}u64)" for a, b in floats)
        + "];\n"
    )
    scaffold += 'for &(a,b) in floats { match python_float_divmod(f64::from_bits(a),f64::from_bits(b)) { Some((q,r))=>println!("F:{}:{}",classify(q),classify(r)),None=>println!("F:ZERO") } }\n'
    scaffold += "}\n"
    source = tmp_path / "numeric_float_divmod.rs"
    source.write_text(policy.render(policy.load_policy()) + scaffold, encoding="utf-8")
    binary = tmp_path / (
        "numeric_float_divmod.exe"
        if sys.platform == "win32"
        else "numeric_float_divmod"
    )
    result = run_guarded_test_process(
        [compiler, "--edition=2024", str(source), "-o", str(binary)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    result = run_guarded_test_process(
        [str(binary)], capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == expected


def test_numeric_policy_covers_the_selected_target_python_catalog():
    sys.path.insert(0, str(ROOT / "src"))
    from molt.target_python import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS

    assert policy.TARGETS == SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS, (
        "a newly admitted target minor requires observed numeric policy coordinates"
    )


def test_admission_expansion_requires_observed_numeric_coordinates(monkeypatch):
    monkeypatch.setattr(policy, "TARGETS", (*policy.TARGETS, "3.15"))
    with pytest.raises(policy.SchemaError, match="needs observed numeric coordinates"):
        policy.load_policy()
