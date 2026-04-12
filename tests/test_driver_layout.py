from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_driver_layout_exposes_generalized_target_namespaces() -> None:
    expected = [
        ROOT / "drivers" / "_shared" / "README.md",
        ROOT / "drivers" / "browser" / "README.md",
        ROOT / "drivers" / "browser" / "wasm_cpu" / "README.md",
        ROOT / "drivers" / "browser" / "webgpu" / "README.md",
        ROOT / "drivers" / "wasm" / "README.md",
        ROOT / "drivers" / "wasm" / "wasi_server" / "README.md",
        ROOT / "drivers" / "cloudflare" / "README.md",
        ROOT / "drivers" / "cloudflare" / "thin_adapter" / "README.md",
        ROOT / "drivers" / "native" / "README.md",
        ROOT / "drivers" / "native" / "packaging" / "README.md",
        ROOT / "drivers" / "falcon" / "README.md",
        ROOT / "drivers" / "falcon" / "browser_webgpu" / "README.md",
    ]
    for path in expected:
        assert path.exists(), path


def test_driver_layout_readme_mentions_shared_and_target_classes() -> None:
    readme = (ROOT / "drivers" / "README.md").read_text(encoding="utf-8")
    assert "_shared/" in readme
    assert "browser/" in readme
    assert "wasm/" in readme
    assert "cloudflare/" in readme
    assert "native/" in readme
