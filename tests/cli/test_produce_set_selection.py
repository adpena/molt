"""Teeth for produce-set selector derivation from single authorities."""

from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli import source_extension_producer as producer


class _Stack:
    numpy = "2.5.1"
    scipy = "1.18.0"
    cpython = "3.12"


def _stub_stack(monkeypatch: pytest.MonkeyPatch) -> None:
    import molt.scientific_stack_versions as ssv

    monkeypatch.setattr(ssv, "resolve_scientific_stack", lambda: _Stack())


def test_explicit_selectors_are_honored_unchanged(tmp_path: Path) -> None:
    selection = producer.resolve_produce_set_selection(
        package="numpy",
        package_version="9.9.9",
        python_version="3.13",
        source=str(tmp_path),
    )
    assert selection == producer.ProduceSetSelection("9.9.9", "3.13", str(tmp_path))


def test_versions_derive_from_the_selected_scientific_stack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stub_stack(monkeypatch)
    selection = producer.resolve_produce_set_selection(
        package="scipy", package_version=None, python_version=None, source=str(tmp_path)
    )
    assert (selection.package_version, selection.python_version) == ("1.18.0", "3.12")


def test_non_scientific_packages_require_an_explicit_version(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stub_stack(monkeypatch)
    with pytest.raises(
        producer.SourceExtensionProducerError, match="--package-version is required"
    ):
        producer.resolve_produce_set_selection(
            package="pandas",
            package_version=None,
            python_version="3.12",
            source=str(tmp_path),
        )


def test_source_derives_from_registry_commit_under_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commit = "5e1d03ffac5f2c0a9c39bfcaa9fc853b2b83151e"
    checkout = tmp_path / "custody" / "package-sources" / f"numpy-2.5.1-{commit[:12]}"
    checkout.mkdir(parents=True)

    class Source:
        pass

    class Package:
        source = Source()

    Package.source.commit = commit

    class Registry:
        def package(self, name: str, version: str) -> Package:
            assert (name, version) == ("numpy", "2.5.1")
            return Package()

    import molt.cli.source_extension_set_registry as registry_module
    import molt.dx as dx

    class Custody:
        custody_root = tmp_path / "custody"

    monkeypatch.setattr(
        registry_module, "load_source_extension_registry", lambda: Registry()
    )
    monkeypatch.setattr(dx, "checkout_custody", lambda *_a, **_k: Custody())
    monkeypatch.setattr(producer, "_git_head", lambda root: commit)
    selection = producer.resolve_produce_set_selection(
        package="numpy", package_version="2.5.1", python_version="3.12", source=None
    )
    assert selection.source == str(checkout)

    monkeypatch.setattr(producer, "_git_head", lambda root: "0" * 40)
    with pytest.raises(
        producer.SourceExtensionProducerError, match="not at the registered commit"
    ):
        producer.resolve_produce_set_selection(
            package="numpy", package_version="2.5.1", python_version="3.12", source=None
        )

    monkeypatch.setattr(
        registry_module, "load_source_extension_registry", lambda: Registry()
    )
    checkout.rmdir()
    with pytest.raises(producer.SourceExtensionProducerError, match="is absent"):
        producer.resolve_produce_set_selection(
            package="numpy", package_version="2.5.1", python_version="3.12", source=None
        )
