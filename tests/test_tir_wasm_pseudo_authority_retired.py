from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TIR_DIR = ROOT / "runtime" / "molt-tir" / "src" / "tir"
RETIRED_MODULES = ("wasm_split", "wasm_streaming", "wasm_component")


def test_tir_no_longer_exports_estimate_only_wasm_modules() -> None:
    mod_rs = (TIR_DIR / "mod.rs").read_text(encoding="utf-8")

    for module in RETIRED_MODULES:
        assert f"pub mod {module};" not in mod_rs
        assert f"mod {module};" not in mod_rs


def test_estimate_only_tir_wasm_modules_stay_retired() -> None:
    existing = [
        f"{module}.rs"
        for module in RETIRED_MODULES
        if (TIR_DIR / f"{module}.rs").exists()
    ]

    assert existing == [], (
        "TIR must not expose WASM split/component/streaming product authority "
        "from name-prefix or op-count estimates. Product work belongs on real "
        f"emitted-WASM artifact facts, not retired stubs: {existing}"
    )
