from __future__ import annotations

from pathlib import Path

from tools import check_satellite_parity


def test_normalize_ignores_cfg_test_items(tmp_path: Path) -> None:
    module = check_satellite_parity
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
