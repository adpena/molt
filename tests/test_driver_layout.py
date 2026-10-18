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


def test_shared_artifact_wire_paths_and_order_are_host_independent(
    tmp_path: Path,
) -> None:
    import hashlib

    from drivers._shared.artifacts import artifact_record, directory_records

    # Distinct names also on case-insensitive filesystems; native Windows Path
    # ordering would put lowercase a before uppercase Z.
    names = [
        "weights/a-low.bin",
        "weights/Z-high.bin",
        "nested space/config.json",
        "weights.part.bin",
    ]
    payload = b"independent bytes\r\n"
    for name in names:
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
    one = artifact_record(kind="fixture", path=tmp_path / names[0], root=tmp_path)
    assert one["relative_path"] == "weights/a-low.bin"
    rows = directory_records(kind="fixture", root=tmp_path)
    assert [row["relative_path"] for row in rows] == [
        "nested space/config.json",
        "weights.part.bin",
        "weights/Z-high.bin",
        "weights/a-low.bin",
    ]
    assert all(row["size_bytes"] == len(payload) for row in rows)
    assert all(row["sha256"] == hashlib.sha256(payload).hexdigest() for row in rows)
    assert all("\\" not in row["relative_path"] for row in rows)
