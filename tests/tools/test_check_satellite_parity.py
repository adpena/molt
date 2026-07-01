from __future__ import annotations

import importlib.util
from pathlib import Path


def _load_tool():
    root = Path(__file__).resolve().parents[2]
    path = root / "tools" / "check_satellite_parity.py"
    spec = importlib.util.spec_from_file_location(
        "check_satellite_parity_under_test", path
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_normalize_ignores_cfg_test_items(tmp_path: Path) -> None:
    module = _load_tool()
    source = tmp_path / "runtime.rs"
    source.write_text(
        "\n".join(
            [
                "pub fn shipped() -> u64 {",
                "    1",
                "}",
                "#[cfg(test)]",
                "mod tests {",
                "    #[test]",
                "    fn noisy_access_layer_unit_test() {",
                "        assert_eq!(2 + 2, 4);",
                "    }",
                "}",
                "pub fn also_shipped() -> u64 {",
                "    2",
                "}",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    normalized = module.normalize(source)

    assert "pub fn shipped() -> u64 {" in normalized
    assert "pub fn also_shipped() -> u64 {" in normalized
    assert not any("noisy_access_layer_unit_test" in line for line in normalized)
