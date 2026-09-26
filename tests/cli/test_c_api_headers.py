from __future__ import annotations

from pathlib import Path

import pytest

from molt.c_api_headers import CAPIHeaderClosureError, c_api_header_closure
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.source_extension_toolchain import (
    _source_extension_include_dirs_for_abi_tier,
    _source_extension_python_header_for_abi_tier,
)


def test_owned_header_closure_preserves_search_order_cycles_and_system_boundary(
    tmp_path: Path,
) -> None:
    public, shared = tmp_path / "public", tmp_path / "shared"
    public.mkdir()
    shared.mkdir()
    header = public / "Python.h"
    header.write_text(
        '#include "local.h"\n#include <_exports.h>\n#include <sys/types.h>\n'
        '/* #include "missing-comment.h" */\n// #include "missing-line.h"\n',
        encoding="utf-8",
    )
    (public / "local.h").write_text('#include "Python.h"\n', encoding="utf-8")
    (shared / "local.h").write_text(
        '#include "wrong-search-order.h"\n', encoding="utf-8"
    )
    (shared / "_exports.h").write_text("extern int PyOwned(void);\n", encoding="utf-8")
    (shared / "unrelated.h").write_text(
        '#include "must-not-be-scanned.h"\n', encoding="utf-8"
    )

    assert c_api_header_closure(header, include_dirs=(shared, public)) == tuple(
        sorted(
            (header, public / "local.h", shared / "_exports.h"),
            key=lambda p: p.as_posix(),
        )
    )


@pytest.mark.parametrize("include", ['"missing.h"', "<_missing.h>"])
def test_missing_owned_include_fails_closed(tmp_path: Path, include: str) -> None:
    header = tmp_path / "Python.h"
    header.write_text(f"#include {include}\n", encoding="utf-8")
    with pytest.raises(CAPIHeaderClosureError, match="missing local header"):
        c_api_header_closure(header, include_dirs=(tmp_path,))
    surface, returned_header, error = _load_c_api_scan_surface(
        tmp_path, header_path=header
    )
    assert surface is None
    assert returned_header == header
    assert error is not None and "missing local header" in error


@pytest.mark.parametrize("abi_tier", ["source-compat", "cpython-abi"])
def test_scan_surface_follows_selected_tier_private_declarations(
    tmp_path: Path,
    abi_tier: str,
) -> None:
    header = _source_extension_python_header_for_abi_tier(
        molt_root=tmp_path, abi_tier=abi_tier
    )
    roots = _source_extension_include_dirs_for_abi_tier(
        molt_root=tmp_path, abi_tier=abi_tier
    )
    header.parent.mkdir(parents=True, exist_ok=True)
    shared = tmp_path / "include" / "molt" / "shared"
    shared.mkdir(parents=True, exist_ok=True)
    private = shared / "_module_callable_exports.h"
    private.write_text(
        "extern void *PyModule_Create2(void *, int);\n", encoding="utf-8"
    )
    (shared / "unrelated.h").write_text(
        "extern void PyUnrelated(void);\n", encoding="utf-8"
    )
    include = (
        "<_module_callable_exports.h>"
        if abi_tier == "cpython-abi"
        else '"shared/_module_callable_exports.h"'
    )
    header.write_text(f"#include {include}\n#include <stdint.h>\n", encoding="utf-8")

    surface, returned_header, error = _load_c_api_scan_surface(
        tmp_path, header_path=header
    )

    assert error is None and surface is not None
    assert returned_header == header
    assert surface.status_for("PyModule_Create2") == "runtime_backed"
    assert surface.status_for("PyUnrelated") == "missing"
    assert private in c_api_header_closure(header, include_dirs=roots)


def test_explicit_fixture_header_does_not_borrow_a_canonical_sdk(
    tmp_path: Path,
) -> None:
    canonical = _source_extension_python_header_for_abi_tier(
        molt_root=tmp_path, abi_tier="source-compat"
    )
    canonical.parent.mkdir(parents=True)
    canonical.write_text("extern int PyCanonicalOnly(void);\n", encoding="utf-8")
    custom = tmp_path / "fixture" / "Python.h"
    custom.parent.mkdir()
    custom.write_text('#include "_custom.h"\n', encoding="utf-8")
    (custom.parent / "_custom.h").write_text(
        "extern int PyCustomOnly(void);\n", encoding="utf-8"
    )

    surface, _, error = _load_c_api_scan_surface(tmp_path, header_path=custom)

    assert error is None and surface is not None
    assert surface.status_for("PyCustomOnly") == "runtime_backed"
    assert surface.status_for("PyCanonicalOnly") == "missing"
