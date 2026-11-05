from __future__ import annotations

from pathlib import Path

import pytest

from tools import check_runtime_symbol_owners


def _write_fn(path: Path, symbol: str, *, cfg: str | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = []
    if cfg is not None:
        lines.append(f"#[cfg({cfg})]")
    lines.extend(
        [
            "#[unsafe(no_mangle)]",
            f'pub extern "C" fn {symbol}() -> u64 {{ 1 }}',
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


def test_detects_cross_satellite_symbol_collision(tmp_path: Path) -> None:
    module = check_runtime_symbol_owners
    runtime = tmp_path / "runtime"
    _write_fn(runtime / "molt-runtime-http" / "src" / "functions.rs", "molt_shared")
    _write_fn(runtime / "molt-runtime-serial" / "src" / "csv.rs", "molt_shared")

    owners = module.collect_symbol_owners(runtime)
    collisions = module.find_cross_crate_collisions(owners)

    assert set(collisions) == {"molt_shared"}
    assert {owner.crate for owner in collisions["molt_shared"]} == {
        "molt-runtime-http",
        "molt-runtime-serial",
    }


def test_allows_cfg_alternatives_inside_one_satellite(tmp_path: Path) -> None:
    module = check_runtime_symbol_owners
    runtime = tmp_path / "runtime"
    _write_fn(
        runtime / "molt-runtime-http" / "src" / "net.rs",
        "molt_cfg_owned",
        cfg="windows",
    )
    with (runtime / "molt-runtime-http" / "src" / "net.rs").open(
        "a", encoding="utf-8"
    ) as fh:
        fh.write(
            "\n#[cfg(unix)]\n"
            "#[unsafe(no_mangle)]\n"
            'pub extern "C" fn molt_cfg_owned() -> u64 { 2 }\n'
        )

    owners = module.collect_symbol_owners(runtime)

    assert module.find_cross_crate_collisions(owners) == {}


def test_ignores_satellite_test_hosts(tmp_path: Path) -> None:
    module = check_runtime_symbol_owners
    runtime = tmp_path / "runtime"
    _write_fn(runtime / "molt-runtime-http" / "src" / "lib.rs", "molt_http_only")
    _write_fn(
        runtime / "molt-runtime-serial" / "src" / "test_host.rs",
        "molt_http_only",
    )

    owners = module.collect_symbol_owners(runtime)

    assert module.find_cross_crate_collisions(owners) == {}


def _write_source(runtime: Path, crate: str, text: str, name: str = "lib.rs") -> Path:
    path = runtime / crate / "src" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def test_physical_exports_collide_despite_distinct_rust_identifiers(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "runtime"
    physical = "molt_shared__context_v1"
    _write_source(
        runtime,
        "molt-runtime-http",
        f'''
#[unsafe(export_name = "{physical}")]
pub unsafe extern "C" fn first(context: u64) -> u64 {{ context }}
''',
    )
    _write_source(
        runtime,
        "molt-runtime-serial",
        f'''
#[export_name = r#"{physical}"#]
extern "C" fn second(context: u64) -> u64 {{ context }}
''',
    )
    owners = check_runtime_symbol_owners.collect_symbol_owners(runtime)
    assert set(owners) == {physical}
    assert set(check_runtime_symbol_owners.find_cross_crate_collisions(owners)) == {
        physical
    }
    assert {owner.line for owner in owners[physical]} == {2}


def test_raw_and_named_exports_share_physical_namespace(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime"
    _write_fn(runtime / "molt-runtime-http/src/lib.rs", "molt_boundary")
    _write_source(
        runtime,
        "molt-runtime-serial",
        """
#[unsafe(export_name = "molt_boundary")]
pub extern "C" fn different_rust_name() -> u64 { 1 }
""",
    )
    owners = check_runtime_symbol_owners.collect_symbol_owners(runtime)
    assert set(check_runtime_symbol_owners.find_cross_crate_collisions(owners)) == {
        "molt_boundary"
    }


def test_equal_rust_names_with_distinct_physical_exports_are_not_collisions(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "runtime"
    for crate, physical in [
        ("http", "molt_a__context_v1"),
        ("serial", "molt_b__context_v1"),
    ]:
        _write_source(
            runtime,
            f"molt-runtime-{crate}",
            f'''
#[unsafe(export_name = "{physical}")]
pub extern "C" fn common(context: u64) -> u64 {{ context }}
''',
        )
    owners = check_runtime_symbol_owners.collect_symbol_owners(runtime)
    assert set(owners) == {"molt_a__context_v1", "molt_b__context_v1"}
    assert check_runtime_symbol_owners.find_cross_crate_collisions(owners) == {}


def test_excludes_root_test_graphs_and_lexical_decoys(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime"
    _write_fn(runtime / "molt-runtime/src/lib.rs", "molt_owned")
    _write_fn(runtime / "molt-runtime-http/src/lib.rs", "molt_owned")
    _write_source(
        runtime,
        "molt-runtime-serial",
        """
#[cfg(test)]
#[path = "validation.rs"]
mod validation;
#[cfg(test)]
mod inline_validation {
    #[unsafe(export_name = "molt_owned")]
    pub extern "C" fn fixture() -> u64 { 1 }
}
const DECOY: &str = r###"#[unsafe(export_name = "molt_owned")]
pub extern "C" fn fake() -> u64 { 1 }"###;
/* #[no_mangle] pub extern "C" fn molt_owned() -> u64 { 1 } */
pub extern "C" fn molt_owned() -> u64 { 1 }
extern "C" { fn imported(); }
""",
    )
    _write_fn(runtime / "molt-runtime-serial/src/validation.rs", "molt_owned")
    owners = check_runtime_symbol_owners.collect_symbol_owners(runtime)
    assert [owner.crate for owner in owners["molt_owned"]] == ["molt-runtime-http"]
    assert check_runtime_symbol_owners.find_cross_crate_collisions(owners) == {}


def test_export_macro_template_requires_actual_invocation(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime"
    path = _write_source(
        runtime,
        "molt-runtime-http",
        """
macro_rules! publish {
    () => {
        #[unsafe(export_name = "molt_macro")]
        pub extern "C" fn template() -> u64 { 1 }
    };
}
""",
    )
    assert check_runtime_symbol_owners.collect_symbol_owners(runtime) == {}
    with path.open("a", encoding="utf-8") as stream:
        stream.write("publish!();\n")
    with pytest.raises(ValueError, match="unsupported export-producing macro publish!"):
        check_runtime_symbol_owners.collect_symbol_owners(runtime)


def test_nonliteral_real_export_fails_closed(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime"
    _write_source(
        runtime,
        "molt-runtime-http",
        """
#[unsafe(export_name = attached_symbol!("molt_owned"))]
pub extern "C" fn attached(context: u64) -> u64 { context }
""",
    )
    with pytest.raises(ValueError, match="nonliteral export_name"):
        check_runtime_symbol_owners.collect_symbol_owners(runtime)


def test_composite_test_scope_excludes_macro_invocation(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime"
    _write_source(
        runtime,
        "molt-runtime-core",
        """
macro_rules! declare_provider {
    () => {
        #[unsafe(export_name = "molt_application")]
        pub extern "C" fn application() -> u64 { 1 }
    };
}
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests { crate::declare_provider!(); }
""",
    )
    assert check_runtime_symbol_owners.collect_symbol_owners(runtime) == {}
