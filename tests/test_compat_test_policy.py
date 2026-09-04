from __future__ import annotations

from pathlib import Path

import pytest

from tools import verified_subset
from tools.compat import test_policy


ROOT = Path(__file__).resolve().parents[1]


def _source(tmp_path: Path, metadata: str, *, name: str = "case.py") -> Path:
    path = tmp_path / name
    path.write_text(f"# MOLT_META: {metadata}\nprint('ok')\n", encoding="utf-8")
    return path


def test_metadata_is_one_frozen_typed_value(tmp_path: Path) -> None:
    metadata = test_policy.parse_metadata(
        _source(
            tmp_path,
            "verified_subset_scope=capability_policy "
            "expect_fail=molt expect_fail_reason=requires_ffi "
            "min_py=3.15 max_py=3.100 platforms=posix "
            "architectures=aarch64,x86_64 backends=llvm,luau,native "
            "stdout=pyperformance stderr=exception_signature stdlib_profile=full",
        )
    )

    assert metadata == test_policy.TestMetadata(
        verification_scope="capability_policy",
        expect_molt_fail=True,
        expected_failure_reason="requires_ffi",
        min_python=(3, 15),
        max_python=(3, 100),
        platforms=frozenset({"posix"}),
        architectures=frozenset({"aarch64", "x86_64"}),
        backends=frozenset({"llvm", "luau", "native"}),
        stdout_mode="pyperformance",
        stderr_mode="exception_signature",
        stdlib_profile="full",
    )
    with pytest.raises(AttributeError):
        metadata.stdout_mode = "exact"  # type: ignore[misc]


@pytest.mark.parametrize(
    "metadata, message",
    [
        ("unknown=value", "unknown MOLT_META key"),
        ("bare", "malformed MOLT_META token"),
        ("min_py=", "empty MOLT_META value"),
        ("platforms=linux,,macos", "empty MOLT_META value"),
        ("min_py=3.12 min_py=3.13", "duplicate MOLT_META key"),
        ("min_py=3.12,3.13", "must select exactly one value"),
        ("backends=native,native", "duplicate MOLT_META value"),
        ("backends=native,llvm", "values must be sorted and unique"),
        ("expect_fail=molt", "must be declared together"),
        ("expect_fail_reason=compiler_gap", "must be declared together"),
        ("expect_fail=python expect_fail_reason=compiler_gap", "exactly 'molt'"),
        ("expect_fail=molt expect_fail_reason=Bad-Reason", "lowercase identifier"),
        ("min_py=3.14 max_py=3.13", "must not exceed"),
        ("platforms=unix", "unknown values"),
        ("platforms=linux,posix", "platforms=posix must not duplicate"),
        ("architectures=amd64", "unknown values"),
        ("backends=python", "unknown values"),
        ("normalize=paths", "unknown MOLT_META key"),
        ("stdout=approximately", "unknown values"),
        ("stdout=relaxed", "unknown values"),
        ("stderr=traceback", "unknown values"),
        ("stdlib_profile=wide", "unknown values"),
        ("stdlib_profile=micro", "unknown values"),
        ("pep=312", "unknown MOLT_META key"),
        ("stdlib=urllib.request", "unknown MOLT_META key"),
    ],
)
def test_metadata_rejects_malformed_or_ambiguous_rows(
    tmp_path: Path, metadata: str, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        test_policy.parse_metadata(_source(tmp_path, metadata))


@pytest.mark.parametrize(
    "legacy",
    [
        "wasm=no",
        "xfail=molt",
        "xfail_reason=gap",
        "platform=windows",
        "architecture=x86_64",
        "arch=x86_64",
        "backend=native",
        "py=3.12",
        "python=3.12",
        "skip=true",
    ],
)
def test_metadata_rejects_deleted_legacy_keys(tmp_path: Path, legacy: str) -> None:
    with pytest.raises(ValueError, match="unknown MOLT_META key"):
        test_policy.parse_metadata(_source(tmp_path, legacy))


@pytest.mark.parametrize(
    "version",
    ["3.11", "3", "3.12.1", "3.12rc1", "03.12", "3.012", "3.１２"],
)
def test_metadata_rejects_noncanonical_python_minor(
    tmp_path: Path, version: str
) -> None:
    with pytest.raises(ValueError, match="exact 3.<minor>"):
        test_policy.parse_metadata(_source(tmp_path, f"min_py={version}"))


def test_metadata_uses_python_comment_tokens_not_string_contents(
    tmp_path: Path,
) -> None:
    path = tmp_path / "string_marker.py"
    path.write_text(
        'TEXT = """\n# MOLT_META: wasm=no\n"""\n# MOLT_META: backends=wasm\n',
        encoding="utf-8",
    )
    assert test_policy.parse_metadata(path).backends == frozenset({"wasm"})


def test_metadata_rejects_malformed_or_multiple_comments(tmp_path: Path) -> None:
    malformed = tmp_path / "malformed.py"
    malformed.write_text("# MOLT_META backends=wasm\n", encoding="utf-8")
    with pytest.raises(ValueError, match="malformed MOLT_META comment"):
        test_policy.parse_metadata(malformed)

    repeated = tmp_path / "repeated.py"
    repeated.write_text(
        "# MOLT_META: backends=wasm\n# MOLT_META: platforms=windows\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="multiple MOLT_META declarations"):
        test_policy.parse_metadata(repeated)


def test_metadata_fails_closed_on_missing_or_invalid_source(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="cannot read differential metadata source"):
        test_policy.parse_metadata(tmp_path / "missing.py")

    invalid = tmp_path / "invalid.py"
    invalid.write_bytes(b"# coding: utf-8\n# MOLT_META: backends=wasm\n\xff")
    with pytest.raises(ValueError, match="cannot decode differential metadata source"):
        test_policy.parse_metadata(invalid)


def test_backend_selector_replaces_wasm_gate_without_semantic_loss(
    tmp_path: Path,
) -> None:
    no_wasm = test_policy.parse_metadata(
        _source(tmp_path, "backends=llvm,luau,native", name="no_wasm.py")
    )
    only_wasm = test_policy.parse_metadata(
        _source(tmp_path, "backends=wasm", name="only_wasm.py")
    )
    tags = test_policy.coordinate_platform_tags(platform="linux")

    for backend in test_policy.ALL_BACKENDS:
        no_wasm_reason = no_wasm.exclusion_reason(
            python_version=(3, 12),
            platform_tags=tags,
            architecture="x86_64",
            backend=backend,
        )
        only_wasm_reason = only_wasm.exclusion_reason(
            python_version=(3, 12),
            platform_tags=tags,
            architecture="x86_64",
            backend=backend,
        )
        assert (no_wasm_reason is None) is (backend != "wasm")
        assert (only_wasm_reason is None) is (backend == "wasm")


def test_every_verified_physical_source_has_valid_typed_metadata() -> None:
    files = verified_subset.verified_subset_test_files(verified_subset.load_manifest())

    parsed = tuple(test_policy.parse_metadata(path) for path in files)

    assert len(parsed) == len(files)
    assert all(isinstance(metadata, test_policy.TestMetadata) for metadata in parsed)
    assert not any("wasm" in metadata.as_record() for metadata in parsed)
