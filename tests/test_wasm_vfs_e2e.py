"""End-to-end tests for VFS bundle → build → run pipeline."""

from __future__ import annotations
import json
import subprocess
import sys
import tarfile
from pathlib import Path
import pytest

PROJECT_ROOT = Path(__file__).resolve().parents[1]


def _create_bundle(src_dir: Path, output: Path) -> None:
    """Create a bundle.tar from a source directory."""
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "wasm_bundle", PROJECT_ROOT / "tools" / "wasm_bundle.py"
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    mod.create_bundle(src_dir, output)


def test_bundle_creation_produces_valid_tar(tmp_path):
    """Bundle tool should produce a valid tar with manifest."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "main.py").write_text("print('hello from bundle')\n")
    (src / "mylib.py").write_text("VALUE = 42\n")

    bundle = tmp_path / "bundle.tar"
    _create_bundle(src, bundle)

    assert bundle.exists()
    with tarfile.open(bundle) as tar:
        names = tar.getnames()
        assert "main.py" in names
        assert "mylib.py" in names
        assert "__manifest__.json" in names

        manifest = json.loads(tar.extractfile("__manifest__.json").read())
        assert len(manifest["files"]) == 2
        assert manifest["total_bytes"] > 0


@pytest.mark.slow
def test_wasm_build_with_bundle(tmp_path):
    """molt build --target wasm with --bundle should produce artifacts."""
    # Create source
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("x = 1 + 2\n")

    # Create bundle
    bundle = tmp_path / "bundle.tar"
    _create_bundle(src, bundle)

    # Build
    output = tmp_path / "output.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(src / "app.py"),
            "--target",
            "wasm",
            "--output",
            str(output),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    # The build should succeed (bundle integration is env-var based)
    assert result.returncode == 0, f"Build failed: {result.stderr}"
    assert output.exists()


@pytest.mark.slow
def test_wasm_build_with_profile_cloudflare(tmp_path):
    """--profile cloudflare should set optimization defaults."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("x = 1\n")

    output = tmp_path / "output.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(src / "app.py"),
            "--target",
            "wasm",
            "--profile",
            "cloudflare",
            "--output",
            str(output),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    assert result.returncode == 0, f"Build failed: {result.stderr}"


@pytest.mark.slow
def test_snapshot_generation(tmp_path):
    """--snapshot should produce a molt.snapshot.json."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "app.py").write_text("x = 1\n")

    output = tmp_path / "output.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(src / "app.py"),
            "--target",
            "wasm",
            "--snapshot",
            "--output",
            str(output),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=180,
    )
    assert result.returncode == 0, f"Build failed: {result.stderr}"

    # Check snapshot was generated
    snapshot = output.with_name("molt.snapshot.json")
    if snapshot.exists():
        data = json.loads(snapshot.read_text())
        assert "snapshot_version" in data
        assert "module_hash" in data
        assert "integrity_hash" in data


@pytest.mark.slow
def test_wasm_default_build_emits_linked_artifact(tmp_path):
    """Regression: ``molt build --target wasm`` (no flags) must emit BOTH the
    raw ``output.wasm`` AND the linked artifact alongside it.

    History: a prior bug let the wasm-ld step fail silently while the build
    still reported success, leaving callers with an unlinkable raw module.
    The pipeline now turns linking on by default (``MOLT_WASM_LINKED=1``) and
    fail-closes if ``wasm-ld`` returns non-zero. This test pins that contract
    so any silent-skip or silent-swallow regression fails immediately.
    """
    src = tmp_path / "app.py"
    src.write_text('print("hello, wasm")\n')

    output = tmp_path / "output.wasm"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(src),
            "--target",
            "wasm",
            "--output",
            str(output),
        ],
        capture_output=True,
        text=True,
        cwd=PROJECT_ROOT,
        timeout=900,
    )
    assert result.returncode == 0, (
        f"Build failed (exit={result.returncode})\n"
        f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
    # Raw frontend-emitted module must exist.
    assert output.exists(), f"Raw wasm output missing: {output}"
    assert output.stat().st_size > 0, "Raw wasm output is empty"

    # Default flow MUST have produced the linked artifact alongside it.
    linked = output.with_name(f"{output.stem}_linked{output.suffix}")
    assert linked.exists(), (
        f"Linked wasm output missing: {linked}\n"
        f"Raw output exists at {output} ({output.stat().st_size} bytes).\n"
        f"This indicates wasm-ld either failed silently or was skipped — the "
        f"build pipeline must fail-close instead of producing only the raw "
        f"module.\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
    assert linked.stat().st_size > 0, "Linked wasm output is empty"

    # Both artifacts must be valid WASM modules (magic + version header).
    for path in (output, linked):
        header = path.read_bytes()[:8]
        assert header[:4] == b"\x00asm", (
            f"{path} is not a valid WASM module (bad magic: {header[:4]!r})"
        )

    # The success message must explicitly acknowledge the linked artifact so
    # downstream tooling and humans see the linking step actually happened.
    combined = result.stdout + result.stderr
    assert "Successfully linked" in combined, (
        f"Build succeeded but never reported linking step.\n"
        f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
    )
