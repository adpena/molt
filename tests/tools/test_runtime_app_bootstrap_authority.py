"""Keep app bootstrap final-image owned, separate from satellite bridge fixtures."""

from pathlib import Path
import re
import tomllib


ROOT = Path(__file__).resolve().parents[2]
RUNTIME = ROOT / "runtime"


def test_native_bootstrap_definition_has_one_typed_authority() -> None:
    definition = re.compile(
        r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+molt_isolate_bootstrap\s*'
        r"\([^)]*\)\s*->\s*u64\s*\{"
    )
    owners = [
        path.relative_to(RUNTIME).as_posix()
        for crate in (
            "molt-runtime-core",
            "molt-runtime",
            "molt-cext-discovery",
            "molt-ffi",
            "molt-wasm-host",
        )
        for path in sorted((RUNTIME / crate).rglob("*.rs"))
        if definition.search(path.read_text(encoding="utf-8"))
    ]
    assert owners == ["molt-runtime-core/src/app_bootstrap.rs"]
    build = (RUNTIME / "molt-runtime/build.rs").read_text(encoding="utf-8")
    assert "molt_isolate_bootstrap" not in build
    assert "molt_isolate_import" not in build
    assert "molt_test_isolate_stubs" not in build


def test_runtime_linking_test_images_explicitly_declare_one_provider() -> None:
    for path in sorted((RUNTIME / "molt-runtime/tests").glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        if "molt_runtime::" in source:
            assert source.count("declare_app_bootstrap!(") == 1, path
    for path in sorted((RUNTIME / "molt-runtime/fuzz/fuzz_targets").glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        assert source.count("declare_app_bootstrap!(") == 1, path
    for name in ("molt-cext-discovery", "molt-ffi"):
        source = (RUNTIME / name / "src/lib.rs").read_text(encoding="utf-8")
        assert source.count("declare_app_bootstrap!(") == 1, name


def test_wasm_host_does_not_link_or_bootstrap_the_native_runtime() -> None:
    host_root = RUNTIME / "molt-wasm-host"
    manifest = tomllib.loads((host_root / "Cargo.toml").read_text(encoding="utf-8"))
    dependency_tables = [manifest.get("dependencies", {})]
    dependency_tables.extend(
        target.get("dependencies", {}) for target in manifest.get("target", {}).values()
    )
    packages = {
        value.get("package", name) if isinstance(value, dict) else name
        for table in dependency_tables
        for name, value in table.items()
    }
    assert "molt-runtime" not in packages
    assert "molt-runtime-core" not in packages
    assert "molt-runtime-resource" in packages
    for path in sorted((host_root / "src").rglob("*.rs")):
        source = path.read_text(encoding="utf-8")
        assert "molt_runtime::" not in source, path
        assert "declare_app_bootstrap!(" not in source, path
        assert "fn molt_isolate_import" not in source, path


def test_dependency_does_not_own_bootstrap_through_fuzzing_or_features() -> None:
    source = (RUNTIME / "molt-runtime/src/lib.rs").read_text(encoding="utf-8")
    assert re.search(r"#\[cfg\(test\)\]\s+declare_app_bootstrap!", source)
    assert source.count("declare_app_bootstrap!(") == 1
    authority = (RUNTIME / "molt-runtime-core/src/app_bootstrap.rs").read_text(
        encoding="utf-8"
    )
    assert '#[cfg(not(target_arch = "wasm32"))]' in authority
    assert "const PROVIDER:" in authority
    assert "std::process::abort()" in authority
    fixtures = (RUNTIME / "molt-runtime-core/src/bridge_test_stubs.rs").read_text(
        encoding="utf-8"
    )
    assert "molt_isolate_bootstrap" not in fixtures
