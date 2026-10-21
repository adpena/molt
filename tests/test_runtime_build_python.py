from __future__ import annotations

import json
import os
import re
import shutil
import sys
import unicodedata
from pathlib import Path

import pytest

from molt.process_environment import NameRule, ProcessEnvironment
from tests.rust.process_guard import run_rust_test_process

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_python_loader_policy_uses_the_callers_name_rule(rule):
    from molt.python_runtime_admission import _loader_environment

    environment = ProcessEnvironment(
        {
            "PATH": "canonical path",
            "Path": "alias path",
            "LD_LIBRARY_PATH": "Canonical Loader",
            "ld_library_path": "Alias Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
            "PYTHONPATH": "isolated startup ignores this",
            "pythonexecutable": "alias executable",
        },
        rule=rule,
    )
    selected = _loader_environment(environment)
    assert selected.rule is rule
    assert dict(selected) == (
        {
            "PATH": "canonical path",
            "LD_LIBRARY_PATH": "Canonical Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
        }
        if rule is NameRule.EXACT
        else {
            "PATH": "alias path",
            "LD_LIBRARY_PATH": "Alias Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
            "PYTHONEXECUTABLE": "alias executable",
        }
    )


@pytest.mark.parametrize(
    ("channel", "selector"),
    [
        ("MOLT_BUILD_PYTHON", "explicit"),
        ("MOLT_BUILD_PYTHON", "path"),
        ("PYTHON", "explicit"),
        ("PYTHON", "path"),
        ("default", "path"),
    ],
)
@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_generator_selection_is_pinned_before_cargo_changes_cwd(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    channel: str,
    selector: str,
    rule: NameRule,
) -> None:
    from molt.cli.runtime_build_python import pin_runtime_build_python_environment
    from molt.python_runtime_admission import resolve_build_python

    name = "python.exe" if os.name == "nt" else "python3"
    operation = tmp_path / "operation"
    crate = tmp_path / "crate"
    for directory, content in ((operation, b"selected"), (crate, b"poison")):
        executable = directory / "bin" / name
        executable.parent.mkdir(parents=True)
        executable.write_bytes(content)
        executable.chmod(0o755)
    environment = {"PATH": "bin", "PATHEXT": ".EXE", "MOLT_BUILD_PYTHON": " "}
    if channel != "default":
        environment[channel] = f"bin/{name}" if selector == "explicit" else name
    if channel == "MOLT_BUILD_PYTHON":
        environment["PYTHON"] = "shadowed-unavailable-python"
    if rule is NameRule.WINDOWS:
        environment["Path"] = environment.pop("PATH")
        environment["Molt_Build_Python"] = environment.pop("MOLT_BUILD_PYTHON")
    environment = ProcessEnvironment(environment, rule=rule)
    original = dict(environment)
    monkeypatch.chdir(crate)
    pinned = pin_runtime_build_python_environment(environment, cwd=operation)
    key = "MOLT_BUILD_PYTHON" if channel == "default" else channel
    assert pinned[key] == str(operation / "bin" / name)
    assert resolve_build_python(pinned) == operation / "bin" / name
    assert resolve_build_python(pinned).read_bytes() == b"selected"
    assert isinstance(pinned, ProcessEnvironment) and pinned.rule is rule
    assert dict(environment) == original
    assert Path.cwd() == crate
    if channel == "PYTHON":
        assert pinned["MOLT_BUILD_PYTHON"] == " "
    if channel == "MOLT_BUILD_PYTHON":
        assert pinned["PYTHON"] == "shadowed-unavailable-python"


def test_unicode_width_projection_matches_running_cpython() -> None:
    source = (
        ROOT / "runtime/molt-runtime/src/object/unicode_width_generated.rs"
    ).read_text(encoding="utf-8")
    major, minor = sys.version_info[:2]
    table_name = f"WIDTH_{major}_{minor}"
    table = source.split(f"const {table_name}: WidthRanges = &[", 1)[1].split("];", 1)[
        0
    ]
    assert (
        f'({major}, {minor}) => Some(("{unicodedata.unidata_version}", {table_name}))'
        in source
    )
    expected = 0
    for lo, hi, width in re.findall(r'\((\d+), (\d+), "(N|Na|A|H|W|F)"\)', table):
        start, end = int(lo), int(hi)
        assert start == expected
        assert all(
            unicodedata.east_asian_width(chr(code)) == width
            for code in range(start, end + 1)
        )
        expected = end + 1
    assert expected == 0x110000


def test_runtime_build_python_consumers_share_one_authority() -> None:
    for crate in ("molt-runtime", "molt-cpython-abi"):
        source = (ROOT / "runtime" / crate / "build.rs").read_text(encoding="utf-8")
        assert '#[path = "../build_support/build_python.rs"]' in source
        assert "build_python::resolve()" in source
        assert "fn resolve_build_python" not in source
        assert "rerun-if-env-changed=PYTHONPATH" not in source
    unicode = (ROOT / "runtime/build_support/unicode_tables.rs").read_text(
        encoding="utf-8"
    )
    assert "crate::build_python::run_script(" in unicode
    assert "Command::new" not in unicode


@pytest.fixture(scope="module")
def unicode_generator(tmp_path_factory: pytest.TempPathFactory) -> Path:
    rustc = shutil.which("rustc")
    if rustc is None:
        pytest.skip("rustc is required for build-script consumer execution")
    directory = tmp_path_factory.mktemp("runtime-build-python")
    driver = directory / "unicode_driver.rs"
    shared = ROOT / "runtime/build_support"
    driver.write_text(
        f"#[path = {json.dumps((shared / 'build_python.rs').as_posix())}]\n"
        "mod build_python;\n"
        f"#[path = {json.dumps((shared / 'unicode_tables.rs').as_posix())}]\n"
        "mod unicode_tables;\n"
        r"""
fn main() {
    let python = build_python::resolve();
    let mode = std::env::args().nth(1).expect("mode");
    if mode == "probe" {
        print!("{}", build_python::run_script(&python,
            "import sys; print('flags', sys.flags.isolated, sys.flags.no_site, sys.flags.dont_write_bytecode)",
            "isolation probe"));
    } else if mode == "failure" {
        build_python::run_script(&python,
            "raise RuntimeError('build-script-sentinel')", "failure probe");
    } else if mode == "generate" {
        let root = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let runtime = root.join("runtime");
        let abi = root.join("abi");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&abi).unwrap();
        unicode_tables::emit_runtime_unicode_tables(&runtime, &python);
        unicode_tables::emit_cpython_abi_unicode_tables(&abi, &python);
    } else {
        panic!("unexpected fixture mode: {mode}");
    }
}
""",
        encoding="utf-8",
    )
    binary = directory / ("unicode.exe" if os.name == "nt" else "unicode")
    result = run_rust_test_process(
        [rustc, "--edition=2024", str(driver), "-o", str(binary)],
        cwd=str(ROOT),
        env=os.environ,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    return binary


@pytest.mark.slow
def test_real_build_generators_ignore_ambient_imports(
    tmp_path: Path, unicode_generator: Path
) -> None:
    poison = tmp_path / "ambient"
    poison.mkdir()
    for name in ("unicodedata", "sitecustomize", "usercustomize"):
        (poison / f"{name}.py").write_text(
            "raise RuntimeError('ambient Python module executed')\n", encoding="utf-8"
        )
    outputs = []
    for polluted in (False, True):
        out = tmp_path / ("polluted" if polluted else "baseline")
        out.mkdir()
        environment = {
            **os.environ,
            "MOLT_BUILD_PYTHON": sys.executable,
            "OUT_DIR": str(out),
        }
        for name in ("PYTHONPATH", "PYTHONHOME", "PYTHONUSERBASE", "PYTHON"):
            environment.pop(name, None)
        if polluted:
            environment.update(
                PYTHONPATH=str(poison),
                PYTHONHOME=str(poison / "not-a-python-home"),
                PYTHONUSERBASE=str(poison),
                PYTHON="shadowed-nonexistent-interpreter",
            )
        result = run_rust_test_process(
            [str(unicode_generator), "generate"],
            cwd=str(poison),
            env=environment,
            timeout=60,
        )
        assert result.returncode == 0, result.stderr
        outputs.append(
            {path.relative_to(out): path.read_bytes() for path in out.rglob("*.rs")}
        )
    assert outputs[0] == outputs[1]
    # Both consumers emit the same complete projection of the one selected
    # CPython Unicode authority; only the runtime needs titlecase mappings.
    properties = {
        "digit",
        "decimal",
        "numeric",
        "space",
        "printable",
        "alpha",
        "lower",
        "upper",
        "title",
        "identifier_start",
        "identifier_continue",
    }
    expected = {
        Path(consumer) / f"unicode_{name}_ranges.rs"
        for consumer in ("runtime", "abi")
        for name in properties
    }
    expected.add(Path("runtime/unicode_titlecase_map.rs"))
    assert set(outputs[0]) == expected
    assert not tuple(poison.rglob("*.pyc"))
    for relative, content in outputs[0].items():
        if relative.parts[0] == "abi":
            assert content == outputs[0][Path("runtime") / relative.name]


@pytest.mark.slow
def test_real_build_python_selection_and_failure_diagnostics(
    tmp_path: Path, unicode_generator: Path
) -> None:
    environment = {**os.environ, "MOLT_BUILD_PYTHON": " ", "PYTHON": sys.executable}
    result = run_rust_test_process(
        [str(unicode_generator), "probe"],
        cwd=str(tmp_path),
        env=environment,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "flags 1 1 1" in result.stdout
    result = run_rust_test_process(
        [str(unicode_generator), "failure"],
        cwd=str(tmp_path),
        env=environment,
        timeout=30,
    )
    assert result.returncode != 0
    assert "failure probe failed" in result.stderr
    assert "build-script-sentinel" in result.stderr


def test_pinned_generator_runs_shared_rust_helper_from_another_crate_cwd(
    tmp_path: Path, unicode_generator: Path
) -> None:
    from molt.cli.runtime_build_python import pin_runtime_build_python_environment

    operation, crate = tmp_path / "operation", tmp_path / "crate"
    operation.mkdir()
    crate.mkdir()
    if os.name == "nt":
        from tests.python_environment_test_support import create_test_venv

        # Preserve the standard copied Windows launcher/configuration layout.
        name = create_test_venv(operation).relative_to(operation)
        poison = crate / name
        poison.parent.mkdir(parents=True)
        shutil.copyfile(unicode_generator, poison)
    else:
        name = Path("selected-python")
        interpreter = Path(getattr(sys, "_base_executable", sys.executable)).resolve(
            strict=True
        )
        try:
            (operation / name).symlink_to(interpreter)
            (crate / name).symlink_to(unicode_generator)
        except OSError:
            pytest.skip("host cannot create executable symlinks")
    environment = {**os.environ, "MOLT_BUILD_PYTHON": f"./{name.as_posix()}"}
    pinned = pin_runtime_build_python_environment(environment, cwd=operation)
    assert pinned["MOLT_BUILD_PYTHON"] == str(operation / name)
    result = run_rust_test_process(
        [str(unicode_generator), "probe"],
        cwd=str(crate),
        env=pinned,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "flags 1 1 1" in result.stdout
    # The same original relative selector invokes the poison crate entrypoint.
    poisoned = run_rust_test_process(
        [str(unicode_generator), "probe"],
        cwd=str(crate),
        env=environment,
        timeout=30,
    )
    assert poisoned.returncode != 0
    assert "flags 1 1 1" not in poisoned.stdout
    assert "unexpected fixture mode: -B" in poisoned.stderr


def test_unicode_width_facts_replay_keyed_by_the_admitted_runtime(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import gen_unicode_width, scan_records

    versions = gen_unicode_width.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    queried: list[str] = []

    def query(version, selected, admission):
        queried.append(version)
        return {
            "python": version,
            "unicode": "15.0.0",
            "ranges": [[0, 0x10FFFF, "N"]],
        }

    def runtimes(digest: str):
        return {
            version: (
                {
                    "runtime": {"runtime_closure_sha256": digest + version},
                    "launch_argv": ["python" + version, "-I", "-S", "-B"],
                },
                None,
            )
            for version in versions
        }

    def records():
        return scan_records.ScanRecords(
            "generator_gen_unicode_width",
            owner=Path(gen_unicode_width.__file__),
            cache_root=tmp_path,
        )

    monkeypatch.setattr(gen_unicode_width, "_query", query)
    computed = gen_unicode_width.render(python_runtimes=runtimes("a"))
    assert queried == list(versions)
    cold = records()
    assert gen_unicode_width.render(python_runtimes=runtimes("a"), records=cold) == (
        computed
    )
    assert cold.publish() is True
    queried.clear()
    warm = records()
    assert gen_unicode_width.render(python_runtimes=runtimes("a"), records=warm) == (
        computed
    )
    assert queried == [] and warm.misses == 0
    # Another runtime closure is another key: its facts are queried again.
    changed = records()
    gen_unicode_width.render(python_runtimes=runtimes("b"), records=changed)
    assert queried == list(versions)


def test_unicode_width_projection_preserves_full_target_identity(monkeypatch):
    from tools import gen_unicode_width

    # Synthetic admitted query results exercise projection only. They are not
    # claims about Python 3.0 or 4.0 Unicode data or compiler support.
    monkeypatch.setattr(
        gen_unicode_width, "SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS", ("3.0", "4.0")
    )
    observed = []

    def query(version, selected, admission):
        observed.append(version)
        return {
            "python": version,
            "unicode": "three" if version == "3.0" else "four",
            "ranges": [[0, 0x10FFFF, "N" if version == "3.0" else "W"]],
        }

    monkeypatch.setattr(gen_unicode_width, "_query", query)
    runtimes = {
        version: (
            {
                "runtime": {"runtime_closure_sha256": version},
                "launch_argv": ["python" + version, "-I", "-S", "-B"],
            },
            None,
        )
        for version in ("3.0", "4.0")
    }
    rendered = gen_unicode_width.render(python_runtimes=runtimes)
    assert observed == ["3.0", "4.0"]
    assert '(3, 0) => Some(("three", WIDTH_3_0))' in rendered
    assert '(4, 0) => Some(("four", WIDTH_4_0))' in rendered
    assert "match target" in rendered and "_ => None" in rendered
    with pytest.raises(ValueError, match="every admitted target runtime"):
        gen_unicode_width.render(python_runtimes={"3.0": runtimes["3.0"]})
