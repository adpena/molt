from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil

import pytest

from molt.cli import entrypoint_dispatch, entrypoint_parser
from molt.cli import source_extension_candidate_attestation as candidate_authority
from molt.cli import source_extension_candidate_promotion as promotion
from molt.cli import source_package_seal as seal_api
from molt.cli import source_extension_candidate_transaction as transaction_authority
from molt.cli import source_extension_set_validation as set_validation
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.source_extension_set_registry import (
    SourceExtensionPackage,
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariantExpectation,
)
from molt.cli.source_extension_set_validation import ValidatedSourceExtensionSetSeal
from molt.cli.source_package_seal import verify_source_package_seal
from tests.cli.test_source_extension_manifest_authority import _stage_identity_fixture


def _extension_set(
    receipt: ValidatedSourceExtensionSetSeal, *, registered: bool
) -> SourceExtensionSet:
    recorded = receipt.validation.recorded
    return SourceExtensionSet(
        package=recorded.package,
        package_version=recorded.package_version,
        source=recorded.source,
        name=recorded.name,
        seal_name=recorded.seal_name,
        variants=(
            SourceExtensionVariantExpectation(
                variant=receipt.validation.variant,
                expected_identity_sha256=receipt.canonical_identity.canonical_sha256,
            ),
        )
        if registered
        else (),
        build_dependency_group="source-build-scipy",
        meson_setup_args=recorded.meson_setup_args,
        use_pkg_config=recorded.use_pkg_config,
        required_installed_files=receipt.validation.installed_package_files,
        required_config_tools=recorded.required_config_tools,
        extensions=recorded.extensions,
    )


def _registry(extension_set: SourceExtensionSet, root: Path) -> SourceExtensionRegistry:
    return SourceExtensionRegistry(
        schema_version=1,
        packages=(
            SourceExtensionPackage(
                name=extension_set.package,
                version=extension_set.package_version,
                source=extension_set.source,
                sets=(extension_set,),
            ),
        ),
        path=root / "registry.toml",
    )


def _finalize_candidate_bundle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    registered: bool = False,
) -> tuple[
    candidate_authority.SourceExtensionCandidateAttestation,
    ValidatedSourceExtensionSetSeal,
]:
    receipt, _identity = _stage_identity_fixture(
        tmp_path, label="candidate", artifact="a" * 64
    )
    custody_root = tmp_path / "custody"
    monkeypatch.setattr(
        candidate_authority, "source_extension_custody_root", lambda: custody_root
    )
    output = candidate_authority.resolve_source_extension_candidate_output(
        custody_root / "package-candidates" / "candidate"
    )
    root = output.parent / ".candidate.attest-fixture"
    root.mkdir()
    handle = _acquire_file_lock(
        output.parent / ".candidate.candidate.lock",
        timeout_s=1.0,
        timeout_message="fixture candidate lock unavailable",
    )
    custody = transaction_authority.source_extension_candidate_transaction_custody(
        output, handle
    )
    extension_set = _extension_set(receipt, registered=registered)
    variant = receipt.validation.variant
    try:
        transaction_authority.begin_source_extension_candidate_transaction(
            root,
            custody=custody,
            package=extension_set.package,
            package_version=extension_set.package_version,
            module_set=extension_set.name,
            cpython=variant.cpython,
            abi_tier=variant.abi_tier,
            target_triple=variant.target_triple,
        )
        attestation = (
            candidate_authority.finalize_source_extension_candidate_attestation(
                transaction_root=root,
                output=output,
                custody=custody,
                validated_candidate=receipt,
                extension_set=extension_set,
                variant=variant,
                registry=_registry(extension_set, tmp_path),
            )
        )
        assert not root.exists()
        return attestation, receipt
    finally:
        _release_file_lock(handle)


def _dispatch(args: argparse.Namespace, tmp_path: Path) -> int:
    return entrypoint_dispatch._dispatch_entrypoint_command(
        args,
        build_fn=lambda **_: 0,
        config_root=tmp_path,
        config={},
        build_cfg={},
        run_cfg={},
        compare_cfg={},
        test_cfg={},
        diff_cfg={},
        extension_cfg={},
        publish_cfg={},
        cfg_capabilities=None,
    )


def test_candidate_cli_separates_build_from_promotion(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    common = [
        "--package",
        "demo",
        "--package-version",
        "1.2.3",
        "--module-set",
        "core",
        "--python-version",
        "3.12",
        "--source",
        "repos/demo",
        "--build-root",
        "build/demo",
    ]
    attest = parser.parse_args(
        [
            "extension",
            "attest-set-candidate",
            *common,
            "--output",
            "candidate",
        ]
    )
    assert attest.target == "wasm"
    assert not hasattr(attest, "expected_identity_sha256")
    assert not hasattr(attest, "candidate")
    calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        entrypoint_dispatch,
        "attest_source_extension_set_candidate",
        lambda **kwargs: calls.append(kwargs) or 0,
    )
    assert _dispatch(attest, tmp_path) == 0
    assert calls[0]["output"] == "candidate"
    assert parser.parse_args(["extension", "produce-set", *common]).target == "wasm"
    publish = parser.parse_args(
        ["extension", "publish-set-candidate", "--candidate", "candidate"]
    )
    assert not hasattr(publish, "source")
    assert not hasattr(publish, "build_root")
    calls.clear()
    monkeypatch.setattr(
        entrypoint_dispatch,
        "publish_source_extension_set_candidate",
        lambda **kwargs: calls.append(kwargs) or 0,
    )
    assert _dispatch(publish, tmp_path) == 0
    assert calls == [
        {
            "candidate": "candidate",
            "expected_incumbent_seal_sha256": None,
            "expected_incumbent_identity_sha256": None,
            "json_output": False,
        }
    ]
    with pytest.raises(SystemExit):
        parser.parse_args(
            [
                "extension",
                "publish-set-candidate",
                "--candidate",
                "candidate",
                "--source",
                "repo",
            ]
        )


def test_candidate_validator_derives_identity_without_variant_registration(
    tmp_path: Path,
) -> None:
    receipt, _identity = _stage_identity_fixture(
        tmp_path, label="unregistered", artifact="a" * 64
    )
    extension_set = _extension_set(receipt, registered=False)
    registry = _registry(extension_set, tmp_path)
    validated = set_validation.validate_source_extension_set_candidate_seal(
        receipt.seal.root,
        extension_set,
        variant=receipt.validation.variant,
        registry=registry,
    )
    assert validated.canonical_identity == receipt.canonical_identity
    with pytest.raises(ValueError, match="no canonical identity is registered"):
        set_validation.validate_source_extension_set_seal(
            receipt.seal.root,
            extension_set,
            variant=receipt.validation.variant,
            registry=registry,
        )


def test_candidate_bundle_is_exact_recomputable_and_nonpublishing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    assert {path.name for path in attestation.root.iterdir()} == {
        candidate_authority.CANDIDATE_SEAL_DIRECTORY,
        candidate_authority.CANDIDATE_ATTESTATION_NAME,
    }
    report = json.loads(attestation.report_path.read_text(encoding="utf-8"))
    assert report["canonical_identity"] == receipt.identity_payload()
    assert report["registry_admission"] == {
        "required_identity_sha256": receipt.canonical_identity.canonical_sha256,
    }
    assert report["publication"] == {
        "performed": False,
        "publication_custody_acquired": False,
    }
    assert attestation.registered_identity_sha256 is None
    assert report == candidate_authority.source_extension_candidate_attestation_payload(
        validated_candidate=set_validation.rebind_source_extension_set_receipt(
            receipt, attestation.seal
        ),
        extension_set=_extension_set(receipt, registered=False),
        variant=receipt.validation.variant,
    )


@pytest.mark.parametrize(
    "section, field, value",
    [
        (None, "ambient_validator", "other"),
        (None, "schema_version", True),
        ("validation", "schema_version", True),
        ("canonical_identity", "schema_version", True),
        ("publication", "performed", 0),
        ("publication", "publication_custody_acquired", 0),
    ],
)
def test_candidate_attestation_schema_rejects_unknown_or_coerced_fields(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    section: str | None,
    field: str,
    value: object,
) -> None:
    attestation, _receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    report = json.loads(attestation.report_path.read_text(encoding="utf-8"))
    container = report if section is None else report[section]
    container[field] = value
    with pytest.raises(candidate_authority.SourceExtensionCandidateAttestationError):
        candidate_authority.validate_source_extension_candidate_attestation_payload(
            report
        )


def test_candidate_report_loader_rejects_indirect_report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, _receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    real_is_link_like = promotion.is_link_like
    monkeypatch.setattr(
        promotion,
        "is_link_like",
        lambda path: path == attestation.report_path or real_is_link_like(path),
    )
    with pytest.raises(
        promotion.SourceExtensionCandidatePromotionError, match="not a regular file"
    ):
        promotion._load_candidate_report(attestation.root)


def _promotion_registry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    receipt: ValidatedSourceExtensionSetSeal,
    *,
    registered: bool = True,
) -> Path:
    registry = _registry(_extension_set(receipt, registered=registered), tmp_path)
    destination = tmp_path / "canonical" / "extension-set"
    monkeypatch.setattr(promotion, "load_source_extension_registry", lambda: registry)
    monkeypatch.setattr(
        promotion, "source_extension_set_root", lambda *_args, **_kwargs: destination
    )
    return destination


def test_promotion_reuses_exact_candidate_seal_without_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)
    copies: list[tuple[Path, Path]] = []
    original_copy = seal_api._copy_seal_candidate

    def observed_copy(source: Path, candidate: Path, digest: str) -> None:
        copies.append((source, candidate))
        original_copy(source, candidate, digest)

    monkeypatch.setattr(seal_api, "_copy_seal_candidate", observed_copy)
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root)
        )
        == 0
    )
    assert (
        verify_source_package_seal(destination).seal_sha256
        == attestation.seal.seal_sha256
    )
    assert not hasattr(promotion, "_build_source_extension_set")
    assert len(copies) == 1
    assert copies[0][0] == attestation.seal.root
    assert "commit-candidates" in copies[0][1].parts
    assert verify_source_package_seal(attestation.seal.root).files == receipt.seal.files
    assert attestation.root.is_dir()


def test_promotion_replacement_requires_exact_incumbent_seal_and_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    incumbent, _identity = _stage_identity_fixture(
        tmp_path, label="incumbent", artifact="b" * 64
    )
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)
    destination.parent.mkdir()
    shutil.copytree(incumbent.seal.root, destination)
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root),
            expected_incumbent_identity_sha256=incumbent.canonical_identity.canonical_sha256,
        )
        == 2
    )
    assert (
        verify_source_package_seal(destination).seal_sha256
        == incumbent.seal.seal_sha256
    )
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root),
            expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
            expected_incumbent_identity_sha256=incumbent.canonical_identity.canonical_sha256,
        )
        == 0
    )
    assert (
        verify_source_package_seal(destination).seal_sha256
        == attestation.seal.seal_sha256
    )


def test_promotion_refuses_unregistered_identity_before_publication_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    _promotion_registry(monkeypatch, tmp_path, receipt, registered=False)
    monkeypatch.setattr(
        promotion,
        "_acquire_file_lock",
        lambda *_args, **_kwargs: pytest.fail(
            "unregistered candidate acquired publication custody"
        ),
    )
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root)
        )
        == 2
    )


def test_candidate_finalize_requires_live_exact_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    output = attestation.root.parent / "second"
    root = output.parent / ".second.attest-fixture"
    root.mkdir()
    handle = _acquire_file_lock(
        output.parent / ".second.candidate.lock", timeout_s=1, timeout_message="lock"
    )
    custody = transaction_authority.source_extension_candidate_transaction_custody(
        output, handle
    )
    _release_file_lock(handle)
    extension_set = _extension_set(receipt, registered=False)
    with pytest.raises(
        transaction_authority.SourceExtensionCandidateTransactionError,
        match="live exclusive",
    ):
        candidate_authority.finalize_source_extension_candidate_attestation(
            transaction_root=root,
            output=output,
            custody=custody,
            validated_candidate=receipt,
            extension_set=extension_set,
            variant=receipt.validation.variant,
            registry=_registry(extension_set, tmp_path),
        )
    assert not output.exists()


def test_promotion_rejects_duplicate_json_and_retains_candidate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)
    original = attestation.report_path.read_text(encoding="utf-8")
    attestation.report_path.write_text(
        original.replace(
            '"schema_version": 1', '"schema_version": 1, "schema_version": 1', 1
        )
    )
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root)
        )
        == 2
    )
    assert not destination.exists()
    assert attestation.root.exists()


def test_promotion_cleanup_failure_reports_committed_namespace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)

    def fail_cleanup(_path: Path) -> None:
        raise OSError("injected cleanup failure")

    monkeypatch.setattr(promotion, "durable_remove_path", fail_cleanup)
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root), json_output=True
        )
        == 2
    )
    output = capsys.readouterr().out
    assert "publication committed" in output
    assert "cleanup failed" in output
    assert (
        verify_source_package_seal(destination).seal_sha256
        == attestation.seal.seal_sha256
    )
