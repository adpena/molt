from __future__ import annotations

import json
import os
import platform
import shutil
import sys
from pathlib import Path

import pytest

from tests.rust.process_guard import run_rust_test_process

ROOT = Path(__file__).resolve().parents[1]


def test_runtime_build_python_consumers_share_one_authority() -> None:
    for crate in ("molt-runtime", "molt-cpython-abi", "molt-runtime-platform"):
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
def generators(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Path]:
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
    } else {
        let root = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let runtime = root.join("runtime");
        let abi = root.join("abi");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&abi).unwrap();
        unicode_tables::emit_runtime_unicode_tables(&runtime, &python);
        unicode_tables::emit_cpython_abi_unicode_tables(&abi, &python);
    }
}
""",
        encoding="utf-8",
    )
    sources = {
        "unicode": driver,
        "errno": ROOT / "runtime/molt-runtime-platform/build.rs",
    }
    binaries = {}
    for name, source in sources.items():
        binary = directory / (name + (".exe" if os.name == "nt" else ""))
        result = run_rust_test_process(
            [rustc, "--edition=2024", str(source), "-o", str(binary)],
            cwd=str(ROOT),
            env=os.environ,
            timeout=60,
        )
        assert result.returncode == 0, result.stderr
        binaries[name] = binary
    return binaries


@pytest.mark.slow
def test_real_build_generators_ignore_ambient_imports(
    tmp_path: Path, generators: dict[str, Path]
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
        environment["CARGO_CFG_TARGET_ARCH"] = platform.machine()
        for name in ("PYTHONPATH", "PYTHONHOME", "PYTHONUSERBASE", "PYTHON"):
            environment.pop(name, None)
        if polluted:
            environment.update(
                PYTHONPATH=str(poison),
                PYTHONHOME=str(poison / "not-a-python-home"),
                PYTHONUSERBASE=str(poison),
                PYTHON="shadowed-nonexistent-interpreter",
            )
        for name, binary in generators.items():
            result = run_rust_test_process(
                [str(binary), "generate"], cwd=str(poison), env=environment, timeout=60
            )
            assert result.returncode == 0, (name, result.stderr)
        outputs.append(
            {path.relative_to(out): path.read_bytes() for path in out.rglob("*.rs")}
        )
    assert outputs[0] == outputs[1]
    assert len(outputs[0]) == 12  # Six runtime, five ABI, one errno module.
    assert b"EINVAL" in outputs[0][Path("errno_constants.rs")]
    assert not tuple(poison.rglob("*.pyc"))
    for relative, content in outputs[0].items():
        if relative.parts[0] == "abi":
            assert content == outputs[0][Path("runtime") / relative.name]


@pytest.mark.slow
def test_real_build_python_selection_and_failure_diagnostics(
    tmp_path: Path, generators: dict[str, Path]
) -> None:
    environment = {**os.environ, "MOLT_BUILD_PYTHON": " ", "PYTHON": sys.executable}
    result = run_rust_test_process(
        [str(generators["unicode"]), "probe"],
        cwd=str(tmp_path),
        env=environment,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "flags 1 1 1" in result.stdout
    result = run_rust_test_process(
        [str(generators["unicode"]), "failure"],
        cwd=str(tmp_path),
        env=environment,
        timeout=30,
    )
    assert result.returncode != 0
    assert "failure probe failed" in result.stderr
    assert "build-script-sentinel" in result.stderr
