from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "update_status_blocks.py"


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "update_status_blocks_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _write_file(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_write_updates_generated_compat_summary_block(
    tmp_path: Path,
) -> None:
    module = _load_module()
    status_doc = tmp_path / "docs/spec/STATUS.md"
    audit_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md"
    )
    platform_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md"
    )
    _write_file(
        status_doc,
        "\n".join(
            [
                "# STATUS",
                "",
                "<!-- GENERATED:compat-summary:start -->",
                "stale",
                "<!-- GENERATED:compat-summary:end -->",
                "",
            ]
        ),
    )
    _write_file(
        audit_doc,
        "\n".join(
            [
                "# Stdlib Intrinsics Audit",
                "",
                "## Progress Summary (Generated)",
                "- Total audited modules: `877`",
                "- `intrinsic-backed`: `41`",
                "- `intrinsic-partial`: `836`",
                "- `probe-only`: `0`",
                "- `python-only`: `0`",
                "",
            ]
        ),
    )
    _write_file(
        platform_doc,
        "\n".join(
            [
                "# Stdlib Platform Availability",
                "",
                "## Summary",
                "- Modules with explicit Availability metadata: `66`",
                "- WASI blocked (any lane): `41`",
                "- Emscripten blocked (any lane): `37`",
                "",
            ]
        ),
    )

    module.STATUS_DOC = status_doc
    module.STDLIB_AUDIT_DOC = audit_doc
    module.STDLIB_PLATFORM_DOC = platform_doc

    assert module.main(["--write"]) == 0

    updated = status_doc.read_text(encoding="utf-8")
    assert "Stdlib lowering audit" in updated
    assert "`877` modules audited" in updated
    assert "`41` intrinsic-backed" in updated
    assert "`836` intrinsic-partial" in updated
    assert "`66` modules with explicit availability notes" in updated
    assert "`41` WASI-blocked" in updated
    assert "`37` Emscripten-blocked" in updated


def test_check_fails_when_generated_compat_summary_block_is_stale(
    tmp_path: Path,
) -> None:
    module = _load_module()
    status_doc = tmp_path / "docs/spec/STATUS.md"
    audit_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md"
    )
    platform_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md"
    )
    _write_file(
        status_doc,
        "\n".join(
            [
                "# STATUS",
                "",
                "<!-- GENERATED:compat-summary:start -->",
                "- old summary",
                "<!-- GENERATED:compat-summary:end -->",
                "",
            ]
        ),
    )
    _write_file(
        audit_doc,
        "\n".join(
            [
                "# Stdlib Intrinsics Audit",
                "",
                "## Progress Summary (Generated)",
                "- Total audited modules: `877`",
                "- `intrinsic-backed`: `41`",
                "- `intrinsic-partial`: `836`",
                "- `probe-only`: `0`",
                "- `python-only`: `0`",
                "",
            ]
        ),
    )
    _write_file(
        platform_doc,
        "\n".join(
            [
                "# Stdlib Platform Availability",
                "",
                "## Summary",
                "- Modules with explicit Availability metadata: `66`",
                "- WASI blocked (any lane): `41`",
                "- Emscripten blocked (any lane): `37`",
                "",
            ]
        ),
    )

    module.STATUS_DOC = status_doc
    module.STDLIB_AUDIT_DOC = audit_doc
    module.STDLIB_PLATFORM_DOC = platform_doc

    assert module.main(["--check"]) == 1


def test_check_passes_after_write(
    tmp_path: Path,
) -> None:
    module = _load_module()
    status_doc = tmp_path / "docs/spec/STATUS.md"
    audit_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md"
    )
    platform_doc = (
        tmp_path
        / "docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md"
    )
    _write_file(
        status_doc,
        "\n".join(
            [
                "# STATUS",
                "",
                "<!-- GENERATED:compat-summary:start -->",
                "placeholder",
                "<!-- GENERATED:compat-summary:end -->",
                "",
            ]
        ),
    )
    _write_file(
        audit_doc,
        "\n".join(
            [
                "# Stdlib Intrinsics Audit",
                "",
                "## Progress Summary (Generated)",
                "- Total audited modules: `877`",
                "- `intrinsic-backed`: `41`",
                "- `intrinsic-partial`: `836`",
                "- `probe-only`: `0`",
                "- `python-only`: `0`",
                "",
            ]
        ),
    )
    _write_file(
        platform_doc,
        "\n".join(
            [
                "# Stdlib Platform Availability",
                "",
                "## Summary",
                "- Modules with explicit Availability metadata: `66`",
                "- WASI blocked (any lane): `41`",
                "- Emscripten blocked (any lane): `37`",
                "",
            ]
        ),
    )

    module.STATUS_DOC = status_doc
    module.STDLIB_AUDIT_DOC = audit_doc
    module.STDLIB_PLATFORM_DOC = platform_doc

    assert module.main(["--write"]) == 0
    assert module.main(["--check"]) == 0
