"""Closed numeric policy schema and reproducible consumer projections."""

from __future__ import annotations
from pathlib import Path
import sys
import json
import tomllib
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tools import gen_python_numeric_errors as policy  # noqa: E402
from tests.process_guard_common import run_guarded_test_process  # noqa: E402
from molt.process_guard import FRESH_EXECUTABLE_TIMEOUT_S  # noqa: E402


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
        assert dict(row.messages) == {
            "3.12": observed_legacy[row.name][0],
            "3.13": observed_legacy[row.name][1],
            "3.14": observed_314.get(row.name, "division by zero"),
            "3.15": observed_314.get(row.name, "division by zero"),
        }
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
        text = path.read_text(encoding="utf-8")
        assert text == policy.render(
            policy.load_policy(), **policy.render_options(path)
        ), path
        # The runtime raises a typed kind; transpiler backends spell the class.
        typed = "molt-runtime-core" in path.parts
        assert ("crate::BuiltinExc::ZeroDivisionError" in text) == typed, path
        assert ('=> "ZeroDivisionError"' in text) == (not typed), path
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
        for target in policy.TARGETS:
            major, minor = target.split(".")
            message = row.messages[target]
            checks.append(
                f"assert_eq!(NumericErrorContext::{row.name}.message({major}, {minor}), Some({json.dumps(message)}));"
            )
        for major, minor in ((2, 12), (3, 11), (3, 16), (4, 12)):
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
        [str(binary)],
        check=True,
        capture_output=True,
        text=True,
        timeout=FRESH_EXECUTABLE_TIMEOUT_S,
    )


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
        [str(binary)],
        capture_output=True,
        text=True,
        timeout=FRESH_EXECUTABLE_TIMEOUT_S,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == expected


def test_numeric_policy_covers_the_selected_target_python_catalog():
    sys.path.insert(0, str(ROOT / "src"))
    from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS

    assert policy.TARGETS == SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS, (
        "a newly admitted target minor requires observed numeric policy coordinates"
    )


def test_admission_expansion_requires_observed_numeric_coordinates(monkeypatch):
    monkeypatch.setattr(policy, "TARGETS", (*policy.TARGETS, "4.0"))
    with pytest.raises(policy.SchemaError, match="needs observed numeric coordinates"):
        policy.load_policy()


@pytest.fixture
def synthetic_catalog_policy(tmp_path, monkeypatch):
    """Synthetic majors, minors and column order expose positional binding."""
    monkeypatch.setattr(policy, "TARGETS", ("4.20", "3.7"))
    current = tomllib.loads(policy.SOURCE.read_text(encoding="utf-8"))
    from tools import cpython_regrtest

    monkeypatch.setattr(
        cpython_regrtest,
        "load_cpython_sources",
        lambda: {"4.20": object(), "3.7": object()},
    )
    lines = ["schema_version = 2", 'observed_targets = ["4.20", "3.7"]']
    for row in current["context"]:
        lines.append("\n[[context]]")
        for field in ("name", "operation", "operand_kind", "error_class"):
            lines.append(f"{field} = {json.dumps(row[field])}")
        lines.extend(
            (
                'python_3_7 = "seventh coordinate"',
                'python_4_20 = "twentieth coordinate"',
            )
        )
    path = tmp_path / "synthetic.toml"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def test_numeric_columns_follow_exact_selected_catalog(synthetic_catalog_policy):
    rows = policy.load_policy(synthetic_catalog_policy)
    assert rows
    assert all(
        dict(row.messages)
        == {"4.20": "twentieth coordinate", "3.7": "seventh coordinate"}
        for row in rows
    )
    rendered = policy.render(rows)
    twentieth = rendered.index("            (4, 20) => Some(match self {")
    seventh = rendered.index("            (3, 7) => Some(match self {")
    assert twentieth < seventh
    for row in rows:
        assert (
            f'Self::{row.name} => "twentieth coordinate",'
            in rendered[twentieth:seventh]
        )
        assert f'Self::{row.name} => "seventh coordinate",' in rendered[seventh:]
    assert "            (3, 12) =>" not in rendered
    assert "            (3, 13) =>" not in rendered
    assert "            (3, 14) =>" not in rendered
    assert "            _ => None," in rendered


@pytest.mark.parametrize(
    ("before", "after"),
    [
        ('["4.20", "3.7"]', '["4.20", "3.7", "3.7"]'),
        ('["4.20", "3.7"]', '["4.20"]'),
        ('["4.20", "3.7"]', '["4.20", "3.8"]'),
        ('python_3_7 = "seventh coordinate"\n', ""),
        ('python_3_7 = "seventh coordinate"', 'python_3_8 = "foreign coordinate"'),
        (
            'python_3_7 = "seventh coordinate"',
            'python_3_7 = "seventh coordinate"\npython_3_8 = "foreign coordinate"',
        ),
        ('python_3_7 = "seventh coordinate"', 'python_3_7 = ""'),
        ('python_3_7 = "seventh coordinate"', "python_3_7 = 7"),
    ],
)
def test_synthetic_catalog_stays_closed(synthetic_catalog_policy, before, after):
    source = synthetic_catalog_policy.read_text(encoding="utf-8")
    assert before in source
    synthetic_catalog_policy.write_text(
        source.replace(before, after, 1), encoding="utf-8"
    )
    with pytest.raises(policy.SchemaError):
        policy.load_policy(synthetic_catalog_policy)


def test_pending_observations_do_not_admit_or_change_guest_outputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A private admission catalog leaves captured 3.15 pending: capture must
    # not change policy, even though the production catalog now admits it.
    monkeypatch.setattr(policy, "TARGETS", ("3.12", "3.13", "3.14"))
    source = policy.SOURCE.read_text(encoding="utf-8")
    pending = policy.load_policy()
    assert all("3.15" in row.messages for row in pending)
    assert "3.15" not in policy.TARGETS
    assert "            (3, 15) =>" not in policy.render(pending)
    for row in pending:
        with pytest.raises(TypeError):
            row.messages["3.15"] = "mutated"
    source = source.replace(', "3.15"]', "]")
    source = "\n".join(
        line for line in source.splitlines() if not line.startswith("python_3_15 =")
    )
    path = tmp_path / "admitted-only.toml"
    path.write_text(source, encoding="utf-8")
    admitted = policy.load_policy(path)
    for output in policy.OUTPUTS:
        kwargs = policy.render_options(output)
        assert policy.render(pending, **kwargs) == policy.render(admitted, **kwargs)


def test_observation_order_is_not_catalog_order(synthetic_catalog_policy):
    before = policy.render(policy.load_policy(synthetic_catalog_policy))
    source = synthetic_catalog_policy.read_text(encoding="utf-8")
    synthetic_catalog_policy.write_text(
        source.replace('["4.20", "3.7"]', '["3.7", "4.20"]'), encoding="utf-8"
    )
    assert policy.render(policy.load_policy(synthetic_catalog_policy)) == before


def test_capture_independent_operations_and_exact_pin(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from tools import command_execution, cpython_regrtest, gen_stdlib_module_union

    source = cpython_regrtest.CPythonSource(
        "4.20", "a" * 40, "v4.20.7", "https://github.com/python/cpython.git"
    )
    pins = cpython_regrtest.load_cpython_sources()
    monkeypatch.setattr(
        cpython_regrtest, "load_cpython_sources", lambda: {**pins, "4.20": source}
    )
    snapshot_requests = []
    monkeypatch.setattr(
        gen_stdlib_module_union,
        "pinned_snapshots",
        lambda versions: snapshot_requests.append(versions),
    )
    # Independent payload: success records actual observed differences, never
    # fills them from declaration messages or assumes a major-three layout.
    rows = {
        row.wire_key: {
            "error_class": row.error_class,
            "message": "observed future exception",
        }
        for row in policy.load_policy()
    }
    payload = {"implementation": "cpython", "version": "4.20.7", "rows": rows}
    commands = []

    def run(argv, **kwargs):
        commands.append(argv)
        return SimpleNamespace(stdout=json.dumps(payload), stderr="")

    def for_file(source_file, *, stem):
        assert source_file == policy.__file__
        assert stem == "MOLT_GENERATOR"
        return SimpleNamespace(run=run)

    monkeypatch.setattr(
        command_execution.CommandExecutor,
        "for_file",
        for_file,
    )
    executable = tmp_path / "python"
    executable.write_bytes(b"independent executable identity fixture")
    result = policy.capture_observations("4.20", executable)
    assert result["source"]["revision"] == "a" * 40
    assert result["admitted_target"] is False
    assert result["observation"]["rows"] == rows
    assert snapshot_requests == [("4.20",)]
    assert len(commands) == 1
    assert commands[0][:6] == [
        str(executable),
        "-B",
        "-X",
        f"pycache_prefix={Path(sys.prefix) / 'bytecode'}",
        "-I",
        "-c",
    ]
    assert commands[0][6] == policy.OBSERVATION_PROGRAM
    payload["version"] = "4.20.6"
    with pytest.raises(policy.SchemaError, match="exact pinned release"):
        policy.capture_observations("4.20", executable)
    payload["version"] = "4.20.7"
    payload["rows"] = {}
    with pytest.raises(policy.SchemaError, match="incomplete"):
        policy.capture_observations("4.20", executable)


def test_capture_cli_modes_and_input_custody(capsys, tmp_path):
    with pytest.raises(SystemExit) as help_exit:
        policy.main(["--help"])
    assert help_exit.value.code == 0
    help_text = capsys.readouterr().out
    for option in ("--capture-observations", "--python-executable", "--report"):
        assert option in help_text
    alias = policy.SOURCE.with_name(policy.SOURCE.name.swapcase())
    if alias.exists() and alias.samefile(policy.SOURCE):
        with pytest.raises(SystemExit) as alias_refused:
            policy.main(
                [
                    "--capture-observations",
                    "3.15",
                    "--python-executable",
                    str(policy.SOURCE),
                    "--report",
                    str(alias),
                ]
            )
        assert alias_refused.value.code == 2
    for arguments in (
        [
            "--capture-observations",
            "3.15",
            "--python-executable",
            str(policy.SOURCE),
            "--report",
            str(policy.OUTPUTS[0]),
        ],
        ["--check", "--capture-observations", "3.15"],
        ["--check", "--report", str(tmp_path / "report.json")],
        [
            "--capture-observations",
            "3.15",
            "--python-executable",
            str(policy.SOURCE),
            "--report",
            str(policy.SOURCE),
        ],
    ):
        protected = (policy.SOURCE, *policy.OUTPUTS)
        before = {path: path.read_bytes() for path in protected}
        with pytest.raises(SystemExit) as refused:
            policy.main(arguments)
        assert refused.value.code == 2
        assert {path: path.read_bytes() for path in protected} == before
