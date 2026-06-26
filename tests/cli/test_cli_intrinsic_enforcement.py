from pathlib import Path

from molt.cli import module_stdlib_policy as cli_module_stdlib_policy


def _write_module(tmp_path: Path, name: str, source: str) -> Path:
    path = tmp_path / name
    path.write_text(source, encoding="utf-8")
    return path


def test_marker_literal_does_not_count_as_intrinsic_usage(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "marker_only.py",
        '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n',
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_require_intrinsic_call_is_intrinsic_backed(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "intrinsic_backed.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_capabilities_has", globals())\n'
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "intrinsic-backed"
    )


def test_probe_only_module_status(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "probe_only.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_stdlib_probe", globals())\n'
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module) == "probe-only"
    )


def test_fail_closed_import_policy_gate_is_not_python_only(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "policy_gate.py",
        (
            '"""namespace reservation"""\n'
            "raise ImportError('not supported; use the explicit adapter')\n"
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "policy-gate"
    )


def test_policy_gate_classifier_rejects_executable_python_body(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "not_policy_gate.py",
        ('"""not a pure gate"""\nVALUE = 1\nraise ImportError(\'not supported\')\n'),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_syntax_error_with_intrinsic_marker_is_python_only(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "invalid.py",
        (
            '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n'
            "def broken(:\n"
            "    return 1\n"
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_same_package_wrapper_importing_intrinsic_root_is_not_python_only(
    tmp_path: Path,
) -> None:
    stdlib_root = tmp_path / "stdlib"
    package = stdlib_root / "pkg"
    package.mkdir(parents=True)
    root = package / "__init__.py"
    wrapper = package / "widgets.py"
    root.write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_WIDGET_BIND = _require_intrinsic("molt_tk_widget_bind_callback_register")\n',
        encoding="utf-8",
    )
    wrapper.write_text(
        "from . import _WIDGET_BIND\nclass Widget:\n    pass\n",
        encoding="utf-8",
    )

    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            {"pkg": root, "pkg.widgets": wrapper},
            stdlib_root,
            json_output=False,
        )
        is None
    )


def test_private_support_module_loaded_by_intrinsic_owner_is_not_python_only(
    tmp_path: Path,
) -> None:
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    owner = stdlib_root / "_pyio.py"
    support = stdlib_root / "_pyio_text.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_READY = _require_intrinsic("molt_import_smoke_runtime_ready")\n'
        "def _load_text_io_classes():\n"
        "    import _pyio_text as text_module\n"
        "    return text_module\n",
        encoding="utf-8",
    )
    support.write_text(
        "class TextIOBase:\n    pass\n",
        encoding="utf-8",
    )

    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            {"_pyio": owner, "_pyio_text": support},
            stdlib_root,
            json_output=False,
        )
        is None
    )
