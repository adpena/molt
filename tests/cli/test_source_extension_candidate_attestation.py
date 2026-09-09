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
from molt.file_locks import _acquire_file_lock, _release_file_lock
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

    def fail_cleanup(_path: Path, **_kwargs) -> None:
        raise OSError("injected cleanup failure")

    monkeypatch.setattr(
        promotion, "complete_source_extension_publication_transaction", fail_cleanup
    )
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


def test_promotion_real_recovery_reclaims_partially_deleted_committed_transaction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    from molt import file_publication

    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)
    real_remove = file_publication.shutil.rmtree
    interrupted = []

    def partial_remove(path, *args, **kwargs):
        path = Path(path)
        # Nested temporary cleanups remain real. Interrupt only the retired
        # completed promotion whose package-store holds its publication journal.
        if (
            path.name.startswith(".molt-retired-v1-")
            and (path / "package-store").exists()
        ):
            journals = list((path / "package-store" / "commits").glob("*.json"))
            assert journals
            journals[0].unlink()
            interrupted.append(path)
            raise OSError("injected after actual promotion journal unlink")
        return real_remove(path, *args, **kwargs)

    with monkeypatch.context() as faults:
        faults.setattr(file_publication.shutil, "rmtree", partial_remove)
        assert (
            promotion.publish_source_extension_set_candidate(
                candidate=str(attestation.root), json_output=True
            )
            == 2
        )
    first = capsys.readouterr().out
    assert "publication committed" in first and "retirement committed" in first
    assert len(interrupted) == 1 and interrupted[0].exists()
    assert (
        verify_source_package_seal(destination).seal_sha256
        == attestation.seal.seal_sha256
    )
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root),
            json_output=True,
            expected_incumbent_seal_sha256=receipt.seal.seal_sha256,
            expected_incumbent_identity_sha256=receipt.canonical_identity.canonical_sha256,
        )
        == 0
    )
    assert not interrupted[0].exists()
    assert (
        verify_source_package_seal(destination).seal_sha256
        == attestation.seal.seal_sha256
    )


@pytest.mark.parametrize("operation", ["produce", "promote"])
def test_publication_recovery_reclaims_only_its_retirement_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    from molt import file_publication
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    destination = tmp_path / "installed"
    prefix = f".{destination.name}.{operation}-"
    source = tmp_path / (prefix + "old")
    foreign = tmp_path / ".candidate.attest-old"
    for root in (source, foreign):
        root.mkdir()
        (root / "journal").write_bytes(b"retired evidence")
    handle = _acquire_file_lock(
        tmp_path / ".installed.producer.lock",
        timeout_s=1.0,
        timeout_message="fixture lock",
    )
    try:
        custody = promotion._source_extension_publication_custody(destination, handle)
        retired = []
        with monkeypatch.context() as faults:

            def partial_remove(path):
                (path / "journal").unlink()
                raise OSError("injected partial reclamation")

            faults.setattr(file_publication.shutil, "rmtree", partial_remove)
            for root, scope in ((source, prefix), (foreign, ".candidate.attest-")):
                with pytest.raises(file_publication.RetirementError) as caught:
                    file_publication.durable_remove_path(root, retirement_scope=scope)
                retired.append(caught.value.retired_path)
        source.mkdir()
        (source / "new-live").write_bytes(b"preserve")
        with pytest.warns(RuntimeWarning, match="preserved extension transaction"):
            promotion.recover_and_prune_source_extension_transactions(
                destination, custody=custody
            )
        assert not retired[0].exists() and retired[1].exists()
        assert (source / "new-live").read_bytes() == b"preserve"
    finally:
        _release_file_lock(handle)


@pytest.mark.parametrize("boundary", ["before-retirement", "partial-retirement"])
def test_producer_public_result_waits_for_real_transaction_retirement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    boundary: str,
) -> None:
    from molt import file_publication
    from molt.cli import source_extension_producer as producer
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    attestation, _receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = tmp_path / "installed"
    root = tmp_path / ".installed.produce-result"
    handle = _acquire_file_lock(
        tmp_path / ".installed.producer.lock",
        timeout_s=1.0,
        timeout_message="fixture publication lock",
    )
    try:
        custody = promotion._source_extension_publication_custody(destination, handle)
        commit = seal_api.prepare_source_package_seal_commit(
            root / "package-store", attestation.seal, destination
        )
        seal_api.commit_source_package_seal(commit)
        capsys.readouterr()
        with monkeypatch.context() as faults:
            if boundary == "before-retirement":

                def fail_rename(*_args):
                    raise OSError("injected before retirement")

                faults.setattr(
                    file_publication,
                    "_namespace_publish_leaf_exclusive_once",
                    fail_rename,
                )
            else:

                def partial_remove(path):
                    journal = next((path / "package-store" / "commits").glob("*.json"))
                    journal.unlink()
                    # Retired-generation cleanup must not reacquire the old name.
                    root.mkdir()
                    (root / "new-live").write_bytes(b"preserve")
                    raise OSError("injected after actual producer journal unlink")

                faults.setattr(file_publication.shutil, "rmtree", partial_remove)
            rc = producer._complete_producer_publication(
                root,
                custody=custody,
                command="produce-set",
                data={"root": str(destination)},
                messages=("must not emit success",),
                json_output=True,
            )
        records = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
        assert rc == 2 and len(records) == 1
        assert records[0]["status"] == "error"
        assert records[0]["data"]["publication_committed"] is True
        assert records[0]["data"]["cleanup_complete"] is False
        assert records[0]["data"]["returncode"] == 2
        assert (
            verify_source_package_seal(destination).seal_sha256
            == attestation.seal.seal_sha256
        )
        assert not handle.file.closed and handle.entry.mutex.locked()
        if boundary == "partial-retirement":
            retired = Path(records[0]["data"]["retired_path"])
            assert records[0]["data"]["namespace_retirement_committed"] is True
            with pytest.warns(RuntimeWarning, match="preserved extension transaction"):
                promotion.recover_and_prune_source_extension_transactions(
                    destination, custody=custody
                )
            assert not retired.exists()
            assert (root / "new-live").read_bytes() == b"preserve"
        else:
            assert "namespace_retirement_committed" not in records[0]["data"]
            assert (root / "package-store" / "commits").is_dir()
            promotion.recover_and_prune_source_extension_transactions(
                destination, custody=custody
            )
            assert not root.exists()
    finally:
        _release_file_lock(handle)
    assert handle.file.closed


def test_producer_prior_retirement_failure_preserves_current_live_transaction(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    from molt import file_publication
    from molt.cli import source_extension_producer as producer
    from molt.file_locks import _acquire_file_lock, _release_file_lock

    attestation, _receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = tmp_path / "installed"
    root = tmp_path / ".installed.produce-current"
    prior = tmp_path / ".installed.produce-prior"
    handle = _acquire_file_lock(
        tmp_path / ".installed.producer.lock",
        timeout_s=1.0,
        timeout_message="fixture publication lock",
    )
    try:
        custody = promotion._source_extension_publication_custody(destination, handle)
        commit = seal_api.prepare_source_package_seal_commit(
            root / "package-store", attestation.seal, destination
        )
        seal_api.commit_source_package_seal(commit)
        prior.mkdir()
        (prior / "journal").write_bytes(b"old journal")
        (prior / "residue").write_bytes(b"old residue")
        with monkeypatch.context() as faults:

            def partial_prior_remove(path):
                (path / "journal").unlink()
                raise OSError("injected after actual prior journal unlink")

            faults.setattr(file_publication.shutil, "rmtree", partial_prior_remove)
            with pytest.raises(file_publication.RetirementError) as caught:
                file_publication.durable_remove_path(
                    prior, retirement_scope=".installed.produce-"
                )
        retired = caught.value.retired_path
        assert not prior.exists() and retired.exists()
        capsys.readouterr()
        with monkeypatch.context() as faults:

            def fail_prior_reclamation(path):
                assert path == retired
                raise OSError("injected prior residue reclamation failure")

            faults.setattr(file_publication.shutil, "rmtree", fail_prior_reclamation)
            rc = producer._complete_producer_publication(
                root,
                custody=custody,
                command="produce-set",
                data={"root": str(destination)},
                messages=("must not emit success",),
                json_output=True,
            )
        records = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
        assert rc == 2 and len(records) == 1
        assert records[0]["status"] == "error"
        outcome = records[0]["data"]
        assert outcome["publication_committed"] is True
        assert outcome["cleanup_complete"] is False
        assert outcome["namespace_retirement_committed"] is False
        assert outcome["transaction_root"] == str(root)
        assert outcome["retired_path"] == str(retired)
        assert outcome["retirement_phase"] == "physical reclamation"
        assert outcome["returncode"] == 2
        assert commit.record_path.is_file()
        assert root.is_dir() and not prior.exists() and retired.is_dir()
        assert (
            verify_source_package_seal(destination).seal_sha256
            == attestation.seal.seal_sha256
        )
        assert not handle.file.closed and handle.entry.mutex.locked()
        promotion.recover_and_prune_source_extension_transactions(
            destination, custody=custody
        )
        assert not retired.exists() and not root.exists() and not prior.exists()
    finally:
        _release_file_lock(handle)
    assert handle.file.closed


@pytest.mark.parametrize("boundary", ["before-call", "inside-call", "verify", "rebind"])
def test_promotion_publication_outcome_tracks_real_call_boundary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    boundary: str,
) -> None:
    attestation, receipt = _finalize_candidate_bundle(tmp_path, monkeypatch)
    destination = _promotion_registry(monkeypatch, tmp_path, receipt)

    def fail_boundary(*_args, **_kwargs):
        if boundary != "before-call":
            assert destination.is_dir()
        raise ValueError(f"injected publication {boundary} failure")

    real_commit = promotion.commit_source_package_seal

    def commit_then_fail(commit):
        real_commit(commit)
        # The caller cannot infer the outcome of an interrupted publication call,
        # even though this fixture knows its real namespace commit completed.
        fail_boundary()

    if boundary == "before-call":
        monkeypatch.setattr(promotion, "_load_candidate_report", fail_boundary)
    elif boundary == "inside-call":
        monkeypatch.setattr(promotion, "commit_source_package_seal", commit_then_fail)
    else:
        attribute = (
            "verify_source_package_seal"
            if boundary == "verify"
            else "rebind_source_extension_set_receipt"
        )
        monkeypatch.setattr(promotion, attribute, fail_boundary)
    capsys.readouterr()
    assert (
        promotion.publish_source_extension_set_candidate(
            candidate=str(attestation.root), json_output=True
        )
        == 2
    )
    records = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert len(records) == 1 and records[0]["status"] == "error"
    outcome = records[0]["data"]
    expected = {
        "before-call": False,
        "inside-call": None,
        "verify": True,
        "rebind": True,
    }
    assert outcome["publication_committed"] is expected[boundary]
    assert outcome["returncode"] == 2
    if boundary == "before-call":
        assert outcome["transaction_root"] is None and not destination.exists()
    else:
        assert (
            verify_source_package_seal(destination).seal_sha256
            == attestation.seal.seal_sha256
        )
        root = Path(outcome["transaction_root"])
        assert root.parent == destination.parent and root.is_dir()
        assert list((root / "package-store" / "commits").glob("*.json"))
