from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


def _load_tool():
    root = Path(__file__).resolve().parents[2]
    path = root / "tools" / "check_runtime_symbol_owners.py"
    spec = importlib.util.spec_from_file_location(
        "check_runtime_symbol_owners_under_test", path
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


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
    module = _load_tool()
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
    module = _load_tool()
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
    module = _load_tool()
    runtime = tmp_path / "runtime"
    _write_fn(runtime / "molt-runtime-http" / "src" / "lib.rs", "molt_http_only")
    _write_fn(
        runtime / "molt-runtime-serial" / "src" / "test_host.rs",
        "molt_http_only",
    )

    owners = module.collect_symbol_owners(runtime)

    assert module.find_cross_crate_collisions(owners) == {}
