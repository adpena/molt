from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


def _load_tool():
    root = Path(__file__).resolve().parents[2]
    path = root / "tools" / "check_tk_import_authority.py"
    spec = importlib.util.spec_from_file_location(
        "check_tk_import_authority_under_test", path
    )
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def _write_minimal_tree(tmp_path: Path, *, root_use: bool = False) -> tuple[Path, Path]:
    root = tmp_path / "runtime" / "molt-runtime-tk" / "src" / "tk.rs"
    tk_dir = tmp_path / "runtime" / "molt-runtime-tk" / "src" / "tk"
    tk_dir.mkdir(parents=True)
    root.write_text(
        "\n".join(
            [
                "mod args;",
                "pub use intrinsics::*;",
                "use args::*;" if root_use else "",
            ]
        ),
        encoding="utf-8",
    )
    (tk_dir / "args.rs").write_text(
        "use super::state::tk_registry;\n",
        encoding="utf-8",
    )
    widgets_dir = tk_dir / "widgets"
    widgets_dir.mkdir()
    (widgets_dir / "common.rs").write_text(
        "use super::super::state::tk_registry;\n",
        encoding="utf-8",
    )
    return root, tk_dir


def test_accepts_explicit_import_authority(tmp_path: Path) -> None:
    module = _load_tool()
    root, tk_dir = _write_minimal_tree(tmp_path)

    assert module.find_tk_import_authority_violations(tk_root=root, tk_dir=tk_dir) == []


def test_rejects_private_root_import_authority(tmp_path: Path) -> None:
    module = _load_tool()
    root, tk_dir = _write_minimal_tree(tmp_path, root_use=True)

    violations = module.find_tk_import_authority_violations(tk_root=root, tk_dir=tk_dir)

    assert [(v.path, v.reason) for v in violations] == [
        (
            "runtime/molt-runtime-tk/src/tk.rs",
            "tk.rs must not be a private import authority",
        )
    ]


def test_rejects_tk_prelude_module_and_file(tmp_path: Path) -> None:
    module = _load_tool()
    root, tk_dir = _write_minimal_tree(tmp_path)
    root.write_text("mod args;\nmod prelude;\npub use intrinsics::*;\n", encoding="utf-8")
    (tk_dir / "prelude.rs").write_text("", encoding="utf-8")

    violations = module.find_tk_import_authority_violations(tk_root=root, tk_dir=tk_dir)

    assert [(v.path, v.reason) for v in violations] == [
        (
            "runtime/molt-runtime-tk/src/tk.rs",
            "tk.rs must not declare a Tk prelude module",
        ),
        (
            "runtime/molt-runtime-tk/src/tk/prelude.rs",
            "Tk must not have a prelude.rs ambient import authority",
        ),
    ]


def test_rejects_child_and_widget_prelude_imports(tmp_path: Path) -> None:
    module = _load_tool()
    root, tk_dir = _write_minimal_tree(tmp_path)
    (tk_dir / "args.rs").write_text("use super::prelude::*;\n", encoding="utf-8")
    (tk_dir / "widgets" / "common.rs").write_text(
        "use super::super::prelude::*;\n",
        encoding="utf-8",
    )

    violations = module.find_tk_import_authority_violations(tk_root=root, tk_dir=tk_dir)

    assert [v.reason for v in violations] == [
        "Tk child modules must not import a Tk prelude authority",
        "Tk nested modules must not import a Tk prelude authority",
    ]


def test_rejects_root_wildcards_and_widget_common_reexport(tmp_path: Path) -> None:
    module = _load_tool()
    root, tk_dir = _write_minimal_tree(tmp_path)
    (tk_dir / "args.rs").write_text("use super::*;\n", encoding="utf-8")
    (tk_dir / "widgets" / "common.rs").write_text(
        "\n".join(
            [
                "use super::super::*;",
                "pub(super) use self::common::*;",
            ]
        ),
        encoding="utf-8",
    )

    violations = module.find_tk_import_authority_violations(tk_root=root, tk_dir=tk_dir)

    assert [v.reason for v in violations] == [
        "Tk child modules must not import the root as wildcard authority",
        "Tk nested modules must not import the root as wildcard authority",
        "Tk widget modules must not reexport common as wildcard authority",
    ]
