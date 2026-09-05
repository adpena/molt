from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli.extension_manifest import (
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _manifest_errors,
    _module_parts,
    _validate_extension_manifest,
)
from molt.cli.python_module_names import (
    canonical_python_module_name,
    canonical_python_module_names,
    encode_python_module_names,
)
from molt.native_callable_abi import NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1


@pytest.mark.parametrize(
    "value",
    [
        None,
        1,
        True,
        b"pkg",
        "",
        " pkg",
        "pkg ",
        "pkg..core",
        ".pkg",
        "pkg.",
        "pkg.core;import os",
        "pkg.core\nimport os",
        "pkg.class",
        "pkg.\N{KELVIN SIGN}",
        "pkg.\x00core",
        "pkg.3core",
    ],
)
def test_module_name_codec_rejects_noncanonical_values(value: object) -> None:
    with pytest.raises(ValueError, match="modules"):
        canonical_python_module_name(value, field="modules")


@pytest.mark.parametrize(
    "value", ["pkg", "pkg._core", "pkg.π", "pkg.match", "pkg.type"]
)
def test_module_name_codec_preserves_source_spelling(value: str) -> None:
    assert canonical_python_module_name(value, field="modules") == value


@pytest.mark.parametrize(
    "value",
    [
        None,
        "pkg",
        ("pkg",),
        {"pkg": True},
        [None],
        [1],
        [True],
        ["pkg.b", "pkg.a"],
        ["pkg", "pkg"],
        ["pkg "],
    ],
)
def test_manifest_module_list_rejects_noncanonical_values(value: object) -> None:
    with pytest.raises(ValueError, match="modules"):
        canonical_python_module_names(value, field="modules")


def test_encoder_owns_sorting_and_deduplication_without_mutating_input() -> None:
    values = ["pkg.b", "pkg", "pkg.b"]
    encoded = encode_python_module_names(values, field="modules")
    assert encoded == ["pkg", "pkg.b"]
    assert values == ["pkg.b", "pkg", "pkg.b"]
    assert canonical_python_module_names(encoded, field="modules") == ("pkg", "pkg.b")
    assert encode_python_module_names((), field="modules") == []
    assert canonical_python_module_names([], field="modules") == ()


def test_encoder_rejects_a_bare_string_and_invalid_member() -> None:
    with pytest.raises(ValueError, match="sequence"):
        encode_python_module_names("pkg", field="modules")
    with pytest.raises(ValueError, match="module name"):
        encode_python_module_names(["pkg", "pkg "], field="modules")


@pytest.mark.parametrize(
    "value", [None, ["pkg.b", "pkg.a"], ["pkg", "pkg"], ["pkg.class"]]
)
def test_manifest_runtime_import_field_is_exact_when_present(value: object) -> None:
    errors = _manifest_errors({"runtime_python_import_modules": value})
    assert any("runtime_python_import_modules" in error for error in errors)


@pytest.mark.parametrize("manifest", [{}, {"runtime_python_import_modules": []}])
def test_general_manifest_validation_does_not_invent_an_import_requirement(
    manifest: dict[str, object],
) -> None:
    # The publishing/admission boundary owns presence; generic manifests also
    # cover non-source-extension artifacts.
    assert not any(
        "runtime_python_import_modules" in error for error in _manifest_errors(manifest)
    )


@pytest.mark.parametrize("value", [None, ["pkg.b", "pkg.a"], ["pkg", "pkg"], ["pkg "]])
def test_python_export_list_does_not_normalize_consumer_input(value: object) -> None:
    errors: list[str] = []
    assert (
        _manifest_dotted_name_tuple(
            {"python_exports": value}, "python_exports", package="pkg", errors=errors
        )
        == ()
    )
    assert errors and "python_exports" in errors[0]


def test_python_exports_preserve_package_boundary_without_partial_result() -> None:
    errors: list[str] = []
    assert (
        _manifest_dotted_name_tuple(
            {"python_exports": ["pkg", "pkgx.child"]},
            "python_exports",
            package="pkg",
            errors=errors,
        )
        == ()
    )
    assert len(errors) == 1 and "escapes admitted package" in errors[0]


def test_python_exports_do_not_consume_unrelated_diagnostics() -> None:
    errors = ["earlier error"]
    assert _manifest_dotted_name_tuple(
        {"python_exports": ["pkg", "pkg.π"]},
        "python_exports",
        package="pkg",
        errors=errors,
    ) == ("pkg", "pkg.π")
    assert errors == ["earlier error"]
    assert (
        _manifest_dotted_name_tuple({}, "python_exports", package="pkg", errors=errors)
        == ()
    )


def _callable_export(**overrides: object) -> dict[str, object]:
    return {
        "module": "pkg.api",
        "name": "run",
        "binding": "module_attr",
        "abi": NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
        "provider_module": "pkg._native",
        "effects": [],
        "deterministic": True,
        **overrides,
    }


@pytest.mark.parametrize("field", ["module", "provider_module"])
@pytest.mark.parametrize(
    "value", [" pkg.api", "pkg.api ", "pkg.class", "pkg.\N{KELVIN SIGN}", "pkgx.api"]
)
def test_callable_module_fields_share_canonical_package_scoped_validation(
    field: str,
    value: str,
) -> None:
    errors: list[str] = []
    result = _manifest_callable_exports(
        {"callable_exports": [_callable_export(**{field: value})]},
        package="pkg",
        errors=errors,
    )
    assert result == ()
    assert errors and field in errors[0]


def test_callable_unicode_module_names_remain_source_spellable() -> None:
    errors: list[str] = []
    result = _manifest_callable_exports(
        {
            "callable_exports": [
                _callable_export(module="pkg.π", provider_module="pkg.σ")
            ]
        },
        package="pkg",
        errors=errors,
    )
    assert errors == []
    assert result[0].module == "pkg.π"
    assert result[0].provider_module == "pkg.σ"


@pytest.mark.parametrize("value", [" pkg", "pkg ", "pkg.class", "pkg.π", "pkg..core"])
def test_extension_module_parts_preserve_ascii_entrypoint_boundary(value: str) -> None:
    assert _module_parts(value) is None


def test_extension_module_parts_use_exact_canonical_spelling() -> None:
    assert _module_parts("pkg._native") == ["pkg", "_native"]


@pytest.mark.parametrize("value", [None, " pkg._native", "pkg.class", "pkg..core"])
def test_extension_manifest_rejects_noncanonical_module_identity(
    value: object,
    tmp_path: Path,
) -> None:
    validation = _validate_extension_manifest(
        {"module": value},
        manifest_dir=tmp_path,
        wheel_path=None,
        require_nonempty_capabilities=False,
        required_abi=None,
    )
    assert any(
        "module" in error and "Python module name" in error
        for error in validation.errors
    )
