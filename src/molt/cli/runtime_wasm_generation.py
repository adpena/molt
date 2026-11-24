"""Runtime WASM pair generations: one receipt and two content-named members.

A source build's pair lives in the host's product store
(``molt.cli.product_store``, kind ``wasm-runtime``): one entry per family
digest, which names both member identities, and one variant per observed Cargo
environment, as the native runtime does (design 56, section 2.8). A variant is
staged whole and published by one exclusive rename, so it is immutable and a
reader sees it whole or not at all. An installed Molt's shipped cell keeps its
own retained directory (``molt.cli.installed_runtime``).
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import tempfile
from contextlib import AbstractContextManager, ExitStack
from dataclasses import dataclass, replace
from functools import cached_property
from enum import Enum
from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING, BinaryIO

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.generation_store import (
    GENERATION_STORE_IDLE_SECONDS,
    GenerationStore,
    GenerationStoreLayout,
    generation_store,
)
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.file_deletion import unlink_file
from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_replace,
    staged_file_path,
)
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.exact_json import (
    capture_exact,
    canonical_json_sha256,
    read_exact,
    string_keyed_mapping,
)
from molt.process_environment import ProcessEnvironment
from molt.cli.runtime_identity_schema import (
    RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
    _freeze_json,
    _thaw_json,
    runtime_family_digest,
)
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    open_stable_regular_file,
    iter_stable_regular_file_chunks,
    stable_regular_file_handle_identity,
    stable_regular_file_identity,
    verify_stable_regular_file_content,
    verify_stable_regular_file_identity,
)

if TYPE_CHECKING:
    from molt.wasm_artifact import WasmRuntimeFacts


_RUNTIME_WASM_GENERATION_SCHEMA = "molt.runtime-wasm-generation.v6"
_RUNTIME_WASM_PRODUCT_SCHEMA = "molt.runtime-wasm-product.v1"
_RUNTIME_WASM_GENERATION_NAME = "molt_runtime.generation.json"
_MEMBER_SUFFIX = ".runtime-wasm-member"
_SHARED_RUNTIME_NAME = "molt_runtime.wasm"
_RELOC_RUNTIME_NAME = "molt_runtime_reloc.wasm"

# An installed cell's retained directory is a working set of immutable
# members, held by the shared generation store owner
# (molt.cli.generation_store); the selected pair is never member-evicted.
_STORE_LOCK_NAME = ".runtime-wasm-store.lock"
_STORE_LEDGER_NAME = ".runtime-wasm-store.json"
_STORE_LEDGER_SCHEMA = "molt.runtime-wasm-store.v1"
_STORE_FILE_RE = re.compile(
    r"^molt_runtime(?:_reloc)?\.wasm\.[0-9a-f]{64}" + re.escape(_MEMBER_SUFFIX) + r"$"
)
_RUNTIME_WASM_STORE = GenerationStoreLayout(
    label="runtime WASM",
    lock_name=_STORE_LOCK_NAME,
    ledger_name=_STORE_LEDGER_NAME,
    ledger_schema=_STORE_LEDGER_SCHEMA,
    entry=_STORE_FILE_RE,
    remove=unlink_file,
)


def _runtime_wasm_store(root: Path) -> AbstractContextManager[GenerationStore]:
    """Hold one retained runtime WASM directory's store lock and ledger."""
    return generation_store(root, _RUNTIME_WASM_STORE)


class RuntimeWasmReceiptRole(Enum):
    SOURCE_BUILD = "source-build"
    INSTALLED_PRODUCT = "installed-product"


class RuntimeWasmCodegenUse(Enum):
    LINKED = "linked"
    DEPLOYED_RUNTIME = "deployed-runtime"

    @classmethod
    def for_output(cls, *, linked: bool, split_runtime: bool) -> RuntimeWasmCodegenUse:
        return cls.LINKED if linked and not split_runtime else cls.DEPLOYED_RUNTIME


@dataclass(frozen=True)
class RuntimeWasmGeneration:
    manifest: Path
    shared: Path
    reloc: Path
    shared_identity: RuntimeBuildIdentity
    reloc_identity: RuntimeBuildIdentity
    shared_member_identity: StableRegularFileIdentity
    reloc_member_identity: StableRegularFileIdentity
    payload: Mapping[str, object]
    receipt_identity: StableRegularFileIdentity | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "payload", _freeze_json(self.payload))

    @property
    def receipt_role(self) -> RuntimeWasmReceiptRole:
        return RuntimeWasmReceiptRole.INSTALLED_PRODUCT

    def product_payload(self) -> dict[str, object]:
        """Serialize the canonical product without granting source-build custody."""
        payload = _thaw_json(self.payload)
        assert isinstance(payload, dict)
        return payload

    def _publication_payload(self) -> Mapping[str, object]:
        return self.payload

    def member_observations(
        self, use: RuntimeWasmCodegenUse
    ) -> tuple[tuple[Path, StableRegularFileIdentity], ...]:
        reloc = (self.reloc, self.reloc_member_identity)
        if use is RuntimeWasmCodegenUse.LINKED:
            return (reloc,)
        return ((self.shared, self.shared_member_identity), reloc)

    def verify_members(
        self,
        *,
        hash_content: bool = False,
        use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME,
    ) -> None:
        for path, member in self.member_observations(use):
            if path.absolute() != member.path:
                raise ValueError("runtime WASM member observation changed path")
            verify_stable_regular_file_identity(
                member, label="runtime WASM member", hash_content=hash_content
            )

    @cached_property
    def _shared_facts(self) -> WasmRuntimeFacts:
        from molt.wasm_artifact import read_wasm_runtime_facts

        return read_wasm_runtime_facts(self.shared_member_identity, relocatable=False)

    @cached_property
    def _reloc_facts(self) -> WasmRuntimeFacts:
        from molt.wasm_artifact import read_wasm_runtime_facts

        return read_wasm_runtime_facts(self.reloc_member_identity, relocatable=True)

    def facts(self, *, relocatable: bool = False) -> WasmRuntimeFacts:
        use = (
            RuntimeWasmCodegenUse.LINKED
            if relocatable
            else RuntimeWasmCodegenUse.DEPLOYED_RUNTIME
        )
        self.verify_members(use=use)
        key = "_reloc_facts" if relocatable else "_shared_facts"
        member = (
            self.reloc_member_identity if relocatable else self.shared_member_identity
        )
        if key in self.__dict__:
            verify_stable_regular_file_identity(
                member, label="runtime WASM cached facts", hash_content=True
            )
        facts = self._reloc_facts if relocatable else self._shared_facts
        self.verify_members(use=use)
        return facts

    @cached_property
    def _linking_obligations(self) -> dict[tuple[str, str], bool]:
        return {}

    def linking_names(self, expected_kinds: Mapping[str, str]) -> frozenset[str]:
        from molt.wasm_linking_symbols import wasm_linking_defined_names

        self.verify_members(use=RuntimeWasmCodegenUse.LINKED)
        previous = self._linking_obligations
        missing = {
            name: kind
            for name, kind in expected_kinds.items()
            if (name, kind) not in previous
        }
        if missing:
            available = wasm_linking_defined_names(
                self.reloc, missing, observed=self.reloc_member_identity
            )
            previous.update(
                ((name, kind), name in available) for name, kind in missing.items()
            )
        elif expected_kinds:
            verify_stable_regular_file_identity(
                self.reloc_member_identity,
                label="runtime WASM cached linking symbols",
                hash_content=True,
            )
        self.verify_members(use=RuntimeWasmCodegenUse.LINKED)
        return frozenset(
            name for name, kind in expected_kinds.items() if previous[(name, kind)]
        )

    @cached_property
    def _structurally_validated_members(self) -> set[StableRegularFileIdentity]:
        return set()

    def validate_structure(
        self, *, use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME
    ) -> None:
        from molt.cli.runtime_wasm_validation import _validate_wasm_structural

        self.verify_members(use=use)
        validated = self._structurally_validated_members
        for _, member in self.member_observations(use):
            if member in validated:
                verify_stable_regular_file_identity(
                    member, label="runtime WASM cached structure", hash_content=True
                )
                continue
            with open_stable_regular_file(
                member.path, label="runtime WASM structural input", observed=member
            ) as opened:
                current = stable_regular_file_handle_identity(
                    opened, label="runtime WASM structural input"
                )
                verify_stable_regular_file_content(
                    member,
                    sha256=current.sha256,
                    size=current.size,
                    label="runtime WASM structural input",
                )
                error = _validate_wasm_structural(member.path)
            if error is not None:
                raise ValueError(error)
            validated.add(member)

    def shared_runtime_import_names(self) -> tuple[str, ...]:
        """Generated runtime ABI the shared member's own build must export."""
        from molt._wasm_runtime_exports import wasm_runtime_import_names_for_features
        from molt.cli.runtime_features import runtime_enabled_cargo_features
        from molt.compilation_target import TargetKind, admit_target

        # The recorded identity names its target as text; admit it here.
        target = admit_target(self.shared_identity.effective_target)
        if target is not TargetKind.WASI and target is not TargetKind.WASM_UNKNOWN:
            raise ValueError(
                f"shared WASM runtime records a non-WASM target {target!r}"
            )
        imports = wasm_runtime_import_names_for_features(
            runtime_enabled_cargo_features(self.shared_identity.runtime_features),
            target=target,
        )
        if not imports:
            raise ValueError("runtime generation selects an empty generated ABI")
        return imports


def _source_generation_payload(
    product: Mapping[str, object], cargo_environment: CargoEnvironmentInputs
) -> dict[str, object]:
    return {
        "schema": _RUNTIME_WASM_GENERATION_SCHEMA,
        "product": product,
        "cargo_environment": cargo_environment.to_dict(),
    }


@dataclass(frozen=True, kw_only=True)
class SourceRuntimeWasmGeneration(RuntimeWasmGeneration):
    """A final product and the exact observations required for source reuse."""

    cargo_environment: CargoEnvironmentInputs

    def __post_init__(self) -> None:
        super().__post_init__()
        if not isinstance(self.cargo_environment, CargoEnvironmentInputs):
            raise ValueError("source runtime WASM requires Cargo environment evidence")

    @property
    def receipt_role(self) -> RuntimeWasmReceiptRole:
        return RuntimeWasmReceiptRole.SOURCE_BUILD

    def source_payload(self) -> dict[str, object]:
        return _source_generation_payload(
            self.product_payload(), self.cargo_environment
        )

    def _publication_payload(self) -> Mapping[str, object]:
        # Retain the one frozen family; serialization projects it lazily.
        return _source_generation_payload(self.payload, self.cargo_environment)


@dataclass(frozen=True)
class RuntimeWasmCodegenBinding:
    """The exact runtime receipt and members selected for an app's code generation."""

    generation: RuntimeWasmGeneration
    required_exports: frozenset[str] | None
    use: RuntimeWasmCodegenUse

    def verify(self) -> None:
        self.generation.verify_members(hash_content=True, use=self.use)
        receipt = self.generation.receipt_identity
        if receipt is None or receipt.path != self.generation.manifest.absolute():
            raise ValueError(
                "runtime WASM binding lacks its pinned receipt observation"
            )
        verify_stable_regular_file_identity(
            receipt, label="pinned runtime WASM receipt", hash_content=True
        )

    @property
    def semantic_digest(self) -> str:
        """Bind app caches to selected bytes, independently of source receipts."""
        reloc = self.generation.reloc_member_identity.sha256
        if self.use is RuntimeWasmCodegenUse.LINKED:
            return canonical_json_sha256(
                {"schema": "molt.runtime-wasm-linked-codegen.v1", "reloc": reloc}
            )
        return canonical_json_sha256(
            {
                "schema": "molt.runtime-wasm-codegen.v1",
                "shared": self.generation.shared_member_identity.sha256,
                "reloc": reloc,
            }
        )


def bind_runtime_wasm_codegen(
    generation: RuntimeWasmGeneration,
    required_exports: set[str] | frozenset[str] | None,
    *,
    use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME,
) -> RuntimeWasmCodegenBinding:
    """Pin the exact receipt and selected members for one app's code generation.

    A product store variant and an installed cell's retained receipt are each
    written once. Final link can use the same receipt to admit those members.
    """
    receipt = generation.receipt_identity
    if receipt is None or receipt.path != generation.manifest.absolute():
        raise ValueError("runtime WASM binding requires its admitted receipt")
    verify_stable_regular_file_identity(receipt, label="pinned runtime WASM receipt")
    generation.verify_members(use=use)
    return RuntimeWasmCodegenBinding(
        generation=generation,
        required_exports=None
        if required_exports is None
        else frozenset(required_exports),
        use=use,
    )


def _require_pair(shared: RuntimeBuildIdentity, reloc: RuntimeBuildIdentity) -> None:
    if (
        shared.payload.get("member_kind") != "shared"
        or reloc.payload.get("member_kind") != "reloc"
        or shared.family_digest != reloc.family_digest
    ):
        raise ValueError(
            "runtime WASM identities must name one shared/reloc build pair"
        )


@dataclass(frozen=True, slots=True)
class RuntimeWasmTrust:
    """The final link's expected family, receipt role, and exact receipt bytes."""

    family_digest: str
    receipt_role: RuntimeWasmReceiptRole
    receipt_sha256: str

    @classmethod
    def of(cls, generation: RuntimeWasmGeneration) -> RuntimeWasmTrust:
        receipt = generation.receipt_identity
        if receipt is None:
            raise ValueError("runtime WASM link requires its admitted receipt")
        return cls(
            generation.reloc_identity.family_digest,
            generation.receipt_role,
            receipt.sha256,
        )

    def arguments(self, manifest: Path) -> list[str]:
        return [
            "--runtime-generation",
            os.fspath(manifest),
            "--runtime-family-digest",
            self.family_digest,
            "--runtime-receipt-role",
            self.receipt_role.value,
            "--runtime-receipt-sha256",
            self.receipt_sha256,
        ]


@dataclass(frozen=True, slots=True)
class _TrustedRuntimeWasmReceipt:
    identity: StableRegularFileIdentity
    payload: object
    product: object
    shared: RuntimeBuildIdentity
    reloc: RuntimeBuildIdentity


def _read_trusted_runtime_wasm_receipt(
    manifest: Path, trust: RuntimeWasmTrust
) -> _TrustedRuntimeWasmReceipt | None:
    try:
        receipt_identity, payload = capture_exact(
            manifest,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="trusted runtime WASM receipt",
        )
        if receipt_identity.sha256 != trust.receipt_sha256:
            return None
        source = trust.receipt_role is RuntimeWasmReceiptRole.SOURCE_BUILD
        product = _source_envelope(payload)[0] if source else payload
        shared, reloc = runtime_wasm_product_identities(product)
    except (OSError, UnicodeError, ValueError, TypeError):
        return None
    if shared.family_digest != trust.family_digest:
        return None
    return _TrustedRuntimeWasmReceipt(receipt_identity, payload, product, shared, reloc)


def read_trusted_runtime_wasm_generation(
    manifest: Path, trust: RuntimeWasmTrust
) -> RuntimeWasmGeneration | None:
    """Admit the exact receipt and both immutable members for a split link."""
    receipt = _read_trusted_runtime_wasm_receipt(manifest, trust)
    if receipt is None:
        return None
    source = trust.receipt_role is RuntimeWasmReceiptRole.SOURCE_BUILD
    validate = _validate_generation_payload if source else _validate_product_payload
    return validate(
        manifest,
        receipt.payload,
        expected_shared_identity=receipt.shared,
        expected_reloc_identity=receipt.reloc,
        receipt_identity=receipt.identity,
    )


@dataclass(frozen=True, slots=True)
class RuntimeWasmRelocAdmission:
    reloc_member_identity: StableRegularFileIdentity
    receipt_identity: StableRegularFileIdentity
    # Reserve the family member's publication path without admitting its bytes.
    shared_path: Path


def read_trusted_runtime_wasm_reloc(
    manifest: Path, trust: RuntimeWasmTrust
) -> RuntimeWasmRelocAdmission | None:
    """Admit only the receipt and the immutable reloc member for a linked module."""
    receipt = _read_trusted_runtime_wasm_receipt(manifest, trust)
    if receipt is None:
        return None
    descriptors = _product_record_descriptors(
        manifest,
        receipt.product,
        expected_shared_identity=receipt.shared,
        expected_reloc_identity=receipt.reloc,
    )
    if descriptors is None:
        return None
    try:
        with ExitStack() as owned:
            reloc_member = _admit_artifact_descriptor(
                descriptors[1], observed=None, owned=owned
            )
    except (OSError, ValueError):
        return None
    return RuntimeWasmRelocAdmission(reloc_member, receipt.identity, descriptors[0][0])


def runtime_wasm_generation_path(shared: Path) -> Path:
    return shared.with_name(_RUNTIME_WASM_GENERATION_NAME)


def _stage_artifact(
    source: Path,
    staged_handle: BinaryIO,
    *,
    published_name: str,
    identity: RuntimeBuildIdentity,
    expected_record: dict[str, object] | None = None,
    observed: StableRegularFileIdentity | None = None,
) -> dict[str, object]:
    hasher = hashlib.sha256()
    size = 0
    with open_stable_regular_file(
        source, label="runtime generation source", observed=observed
    ) as opened:
        for chunk in iter_stable_regular_file_chunks(opened, chunk_bytes=1024 * 1024):
            hasher.update(chunk)
            staged_handle.write(chunk)
            size += len(chunk)
        if observed is not None:
            verify_stable_regular_file_content(
                observed,
                sha256=hasher.hexdigest(),
                size=size,
                label="runtime generation source",
            )
        staged_handle.flush()
        staged_stat = os.fstat(staged_handle.fileno())
    digest = hasher.hexdigest()
    if size != opened.stat.st_size or staged_stat.st_size != size:
        raise ValueError(f"runtime artifact mutated while staging: {source.name}")
    if expected_record is not None and (
        expected_record.get("sha256") != digest or expected_record.get("size") != size
    ):
        raise ValueError(
            f"runtime artifact changed after source generation validation: {source.name}"
        )
    member_name = f"{published_name}.{digest}{_MEMBER_SUFFIX}"
    return {
        "name": published_name,
        "member": member_name,
        "sha256": digest,
        "size": size,
        "identity": identity.member_reference(),
    }


def _publish_immutable_member(staged: Path, member: Path, source: Path) -> None:
    """Publish a content-named member; same-name races can only contain same bytes."""

    if member.exists():
        # The pair's canonical owned admission checks existing content against
        # the staged record before publishing the manifest. Do not hash twice.
        staged.unlink()
        return
    durable_replace(staged, member)
    # Content durability and namespace commit precede final (possibly read-only) mode.
    try:
        shutil.copymode(source, member)
    except OSError:
        pass


def runtime_wasm_store(project_root: Path) -> ProductStore:
    """The product store a runtime WASM pair build of ``project_root`` uses."""
    return ProductStore.of("wasm-runtime", project_root)


def stored_runtime_wasm_generation(
    store: ProductStore,
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    environment: ProcessEnvironment,
) -> SourceRuntimeWasmGeneration | None:
    """The stored variant of this pair built under ``environment``, admitted.

    The entry is the pair's family digest. A reader reads each variant's
    receipt envelope and admits only its own variant, the one whose recorded
    Cargo environment matches ``environment``, so another variant's damage
    never fails it. Its own variant, when it does not admit, raises
    ``ProductStoreError``: a variant is published whole, so that is
    corruption. The caller renews the entry first (``ProductStore.ensure``).
    """
    _require_pair(shared_identity, reloc_identity)
    family = shared_identity.family_digest
    for variant in store.variants(family):
        manifest = variant / _RUNTIME_WASM_GENERATION_NAME
        try:
            _product, recorded = _source_envelope(
                read_exact(
                    manifest,
                    max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
                    label="stored runtime WASM receipt",
                )
            )
        except (OSError, UnicodeError, ValueError, TypeError):
            continue
        if recorded.digest != variant.name:
            continue
        try:
            recorded.verify(environment)
        except ValueError:
            continue
        generation = read_runtime_wasm_generation(
            manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        if generation is None or generation.cargo_environment != recorded:
            raise ProductStoreError(
                store, family, "its receipt or members do not admit", variant=variant
            )
        return generation
    return None


def publish_stored_runtime_wasm_generation(
    store: ProductStore,
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    cargo_environment: CargoEnvironmentInputs,
    source_shared: Path,
    source_reloc: Path,
) -> SourceRuntimeWasmGeneration:
    """Publish one built pair as its environment's variant, then admit it.

    The caller holds the family's build lock (``ProductStore.ensure``) and
    found no admissible variant, so an existing variant for this environment
    is corrupt and is refused, never replaced. The variant is staged inside
    its entry and published by one exclusive rename.
    """
    if not isinstance(cargo_environment, CargoEnvironmentInputs):
        raise ValueError("source runtime WASM requires Cargo environment evidence")
    _require_pair(shared_identity, reloc_identity)
    family = shared_identity.family_digest
    variant = store.variant(family, cargo_environment)
    if variant.exists():
        raise ProductStoreError(
            store,
            family,
            "a variant for this environment exists but does not admit",
            variant=variant,
        )
    entry = store.entry(family)
    entry.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".wasm-runtime-", dir=entry) as temp:
        stage = Path(temp) / "variant"
        stage.mkdir()
        _write_runtime_wasm_generation(
            stage / _SHARED_RUNTIME_NAME,
            stage / _RELOC_RUNTIME_NAME,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            publication=cargo_environment,
            source_shared=source_shared,
            source_reloc=source_reloc,
        )
        durable_publish_directory_exclusive(stage, variant)
    generation = read_runtime_wasm_generation(
        variant / _RUNTIME_WASM_GENERATION_NAME,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    if generation is None:
        raise ProductStoreError(
            store, family, "the published variant does not admit", variant=variant
        )
    store.retain(family)
    return generation


def publish_runtime_wasm_product(
    shared: Path,
    reloc: Path,
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    source_shared: Path | None = None,
    source_reloc: Path | None = None,
    expected_source_receipts: dict[str, dict[str, object]] | None = None,
    source_observations: tuple[StableRegularFileIdentity, StableRegularFileIdentity]
    | None = None,
) -> RuntimeWasmGeneration:
    """Publish an installed product into its retained directory.

    This grants no source-build authority. The directory's store lock covers
    member reuse through selection, so eviction never removes a member this
    publication found and selects.
    """
    with _runtime_wasm_store(shared.parent) as store:
        generation = _write_runtime_wasm_generation(
            shared,
            reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            publication=RuntimeWasmReceiptRole.INSTALLED_PRODUCT,
            source_shared=source_shared,
            source_reloc=source_reloc,
            expected_source_receipts=expected_source_receipts,
            source_observations=source_observations,
        )
        store.select(generation.shared.name, generation.reloc.name)
        store.evict_idle(GENERATION_STORE_IDLE_SECONDS)
        return generation


def _write_runtime_wasm_generation(
    shared: Path,
    reloc: Path,
    *,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
    publication: CargoEnvironmentInputs | RuntimeWasmReceiptRole,
    source_shared: Path | None = None,
    source_reloc: Path | None = None,
    expected_source_receipts: dict[str, dict[str, object]] | None = None,
    source_observations: tuple[StableRegularFileIdentity, StableRegularFileIdentity]
    | None = None,
) -> RuntimeWasmGeneration:
    """Write content-named members, then the receipt that selects them.

    The caller owns the directory: a product store stage nobody else sees,
    or an installed retained directory under its store lock. Fixed runtime
    filenames are not materialized; the receipt names the members.
    """
    if not isinstance(publication, CargoEnvironmentInputs) and (
        publication is not RuntimeWasmReceiptRole.INSTALLED_PRODUCT
    ):
        raise ValueError("runtime WASM publication role lacks required evidence")
    _require_pair(shared_identity, reloc_identity)
    if shared.name != _SHARED_RUNTIME_NAME or reloc.name != _RELOC_RUNTIME_NAME:
        raise ValueError("runtime generation coordinates use non-canonical names")
    if shared.parent != reloc.parent:
        raise ValueError("runtime generation members share one directory")
    shared.parent.mkdir(parents=True, exist_ok=True)
    staged_shared = staged_file_path(shared, purpose="generation")
    staged_reloc = staged_file_path(reloc, purpose="generation")
    actual_source_shared = source_shared or shared
    actual_source_reloc = source_reloc or reloc
    owned_stages: list[Path] = []
    try:
        with staged_shared.open("xb") as staged_handle:
            owned_stages.append(staged_shared)
            shared_record = _stage_artifact(
                actual_source_shared,
                staged_handle,
                published_name=shared.name,
                identity=shared_identity,
                expected_record=(expected_source_receipts or {}).get("shared"),
                observed=None
                if source_observations is None
                else source_observations[0],
            )
        with staged_reloc.open("xb") as staged_handle:
            owned_stages.append(staged_reloc)
            reloc_record = _stage_artifact(
                actual_source_reloc,
                staged_handle,
                published_name=reloc.name,
                identity=reloc_identity,
                expected_record=(expected_source_receipts or {}).get("reloc"),
                observed=None
                if source_observations is None
                else source_observations[1],
            )
        shared_member = shared.parent / str(shared_record["member"])
        reloc_member = reloc.parent / str(reloc_record["member"])
        # Both members share one family; the receipt stores it once.
        payload: dict[str, object] = {
            "schema": _RUNTIME_WASM_PRODUCT_SCHEMA,
            "family_digest": shared_identity.family_digest,
            "family": shared_identity.family_payload(),
            "receipts": {"shared": shared_record, "reloc": reloc_record},
        }
        if isinstance(publication, CargoEnvironmentInputs):
            payload = _source_generation_payload(payload, publication)
            validate = _validate_generation_payload
        else:
            validate = _validate_product_payload
        manifest = runtime_wasm_generation_path(shared)
        _publish_immutable_member(staged_shared, shared_member, actual_source_shared)
        _publish_immutable_member(staged_reloc, reloc_member, actual_source_reloc)
        # Self-validation proves the members THIS publication committed against
        # the identities it was given, through the reader's own validation.
        with ExitStack() as owned:
            generation = validate(
                manifest,
                payload,
                expected_shared_identity=shared_identity,
                expected_reloc_identity=reloc_identity,
                _owned=owned,
            )
            if generation is None:
                raise ValueError(
                    "published runtime generation has a corrupt member or record"
                )
            _atomic_write_json(manifest, payload, sort_keys=True)
        receipt = stable_regular_file_identity(
            manifest, label="published runtime WASM receipt"
        )
        return replace(generation, receipt_identity=receipt)
    finally:
        for staged in owned_stages:
            staged.unlink(missing_ok=True)


def _member_path(manifest: Path, record: object) -> Path | None:
    if not isinstance(record, Mapping):
        return None
    raw = record.get("member")
    if not isinstance(raw, str) or not raw or Path(raw).name != raw:
        return None
    path = manifest.parent / raw
    if path.parent != manifest.parent or not raw.endswith(_MEMBER_SUFFIX):
        return None
    return path


def _artifact_record_descriptor(
    record: object,
    *,
    manifest: Path,
    expected_name: str,
    expected_identity: RuntimeBuildIdentity,
) -> tuple[Path, str, int] | None:
    if (
        not isinstance(record, Mapping)
        or set(record) != {"name", "member", "sha256", "size", "identity"}
        or record.get("name") != expected_name
    ):
        return None
    member = _member_path(manifest, record)
    if member is None or record.get("identity") != expected_identity.member_reference():
        return None
    digest, size = record.get("sha256"), record.get("size")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(char not in "0123456789abcdef" for char in digest)
        or type(size) is not int
        or size < 0
        or member.name != f"{expected_name}.{digest}{_MEMBER_SUFFIX}"
    ):
        return None
    return member, digest, size


def _admit_artifact_descriptor(
    descriptor: tuple[Path, str, int],
    *,
    observed: StableRegularFileIdentity | None,
    owned: ExitStack,
) -> StableRegularFileIdentity:
    """Hash one record member under caller custody, preserving I/O diagnostics."""
    member, digest, size = descriptor
    if observed is not None and observed.path != member.absolute():
        raise ValueError("runtime member observation names another path")
    opened = owned.enter_context(
        open_stable_regular_file(
            member, label="immutable runtime WASM member", observed=observed
        )
    )
    member_identity = stable_regular_file_handle_identity(
        opened, label="immutable runtime WASM member"
    )
    if observed is not None:
        verify_stable_regular_file_content(
            observed,
            sha256=member_identity.sha256,
            size=member_identity.size,
            label="runtime member observation",
        )
        member_identity = observed
    if (digest, size) != (member_identity.sha256, member_identity.size):
        raise ValueError(
            f"runtime WASM member content differs from its record: {member}"
        )
    return member_identity


def _generation_receipts(
    value: object,
) -> dict[str, dict[str, object]] | None:
    """Return the exact typed shared/reloc receipt pair or fail closed."""

    receipts = string_keyed_mapping(value)
    if receipts is None or set(receipts) != {"shared", "reloc"}:
        return None
    typed: dict[str, dict[str, object]] = {}
    for kind in ("shared", "reloc"):
        record = string_keyed_mapping(receipts.get(kind))
        if record is None:
            return None
        typed[kind] = dict(record)
    return typed


def _source_envelope(
    payload: object,
) -> tuple[Mapping[str, object], CargoEnvironmentInputs]:
    value = string_keyed_mapping(payload)
    if (
        value is None
        or set(value) != {"schema", "product", "cargo_environment"}
        or value.get("schema") != _RUNTIME_WASM_GENERATION_SCHEMA
    ):
        raise ValueError("runtime WASM source generation receipt is invalid")
    product = string_keyed_mapping(value.get("product"))
    if product is None:
        raise ValueError("runtime WASM source generation product is invalid")
    return product, CargoEnvironmentInputs.from_dict(value.get("cargo_environment"))


def runtime_wasm_generation_identities(
    payload: object,
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    """Rebuild identities only from a complete source-build envelope."""
    product, _environment = _source_envelope(payload)
    return runtime_wasm_product_identities(product)


def runtime_wasm_product_identities(
    payload: object,
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    """Rebuild identities from an installed product, never a build envelope."""
    value = string_keyed_mapping(payload)
    receipts = None if value is None else _generation_receipts(value.get("receipts"))
    if (
        value is None
        or set(value) != {"schema", "family_digest", "family", "receipts"}
        or receipts is None
        or value.get("schema") != _RUNTIME_WASM_PRODUCT_SCHEMA
    ):
        raise ValueError("runtime WASM product receipt is invalid")
    family = _freeze_json(value.get("family"))
    shared, reloc = (
        RuntimeBuildIdentity.from_member_reference(
            receipts[kind].get("identity"),
            family=family,
            family_digest=value.get("family_digest"),
        )
        for kind in ("shared", "reloc")
    )
    _require_pair(shared, reloc)
    return shared, reloc


def read_runtime_wasm_generation(
    manifest: Path,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
) -> SourceRuntimeWasmGeneration | None:
    """Read a strict source envelope; ambient admission belongs to its caller."""
    try:
        _require_pair(expected_shared_identity, expected_reloc_identity)
        receipt_identity, payload = capture_exact(
            manifest,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="runtime WASM source generation",
        )
    except (OSError, UnicodeError, ValueError):
        return None
    return _validate_generation_payload(
        manifest,
        payload,
        expected_shared_identity=expected_shared_identity,
        expected_reloc_identity=expected_reloc_identity,
        receipt_identity=receipt_identity,
    )


def read_runtime_wasm_product(
    manifest: Path,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
) -> RuntimeWasmGeneration | None:
    """Read a strict installed product without granting source-build custody."""
    try:
        _require_pair(expected_shared_identity, expected_reloc_identity)
        receipt_identity, payload = capture_exact(
            manifest,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="runtime WASM product",
        )
    except (OSError, UnicodeError, ValueError):
        return None
    return _validate_product_payload(
        manifest,
        payload,
        expected_shared_identity=expected_shared_identity,
        expected_reloc_identity=expected_reloc_identity,
        receipt_identity=receipt_identity,
    )


def _product_record_descriptors(
    manifest: Path,
    payload: object,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
) -> tuple[tuple[Path, str, int], tuple[Path, str, int]] | None:
    """Validate record semantics without claiming custody of unconsumed bytes."""
    if (
        expected_shared_identity.family_digest != expected_reloc_identity.family_digest
        or not isinstance(payload, Mapping)
        or set(payload) != {"schema", "family_digest", "family", "receipts"}
        or payload.get("schema") != _RUNTIME_WASM_PRODUCT_SCHEMA
        or payload.get("family_digest") != expected_shared_identity.family_digest
    ):
        return None
    try:
        if runtime_family_digest(payload.get("family")) != payload["family_digest"]:
            return None
    except (TypeError, ValueError):
        return None
    receipts = _generation_receipts(payload.get("receipts"))
    if receipts is None:
        return None
    shared = _artifact_record_descriptor(
        receipts["shared"],
        manifest=manifest,
        expected_name=_SHARED_RUNTIME_NAME,
        expected_identity=expected_shared_identity,
    )
    reloc = _artifact_record_descriptor(
        receipts["reloc"],
        manifest=manifest,
        expected_name=_RELOC_RUNTIME_NAME,
        expected_identity=expected_reloc_identity,
    )
    return None if shared is None or reloc is None else (shared, reloc)


def _validate_product_payload(
    manifest: Path,
    payload: object,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
    receipt_identity: StableRegularFileIdentity | None = None,
    observed_members: tuple[StableRegularFileIdentity, StableRegularFileIdentity]
    | None = None,
    _owned: ExitStack | None = None,
) -> RuntimeWasmGeneration | None:
    """Validate one manifest payload's immutable pair against trusted identities.

    ``manifest`` is the path the member records resolve against; the payload is
    validated exactly as given, whether it was just read from that manifest or
    just written to it by the publishing transaction.
    """

    if _owned is None:
        try:
            with ExitStack() as owned:
                return _validate_product_payload(
                    manifest,
                    payload,
                    expected_shared_identity=expected_shared_identity,
                    expected_reloc_identity=expected_reloc_identity,
                    receipt_identity=receipt_identity,
                    observed_members=observed_members,
                    _owned=owned,
                )
        except (OSError, ValueError):
            return None
    descriptors = _product_record_descriptors(
        manifest,
        payload,
        expected_shared_identity=expected_shared_identity,
        expected_reloc_identity=expected_reloc_identity,
    )
    if descriptors is None:
        return None
    assert isinstance(payload, Mapping)
    shared_member_identity = _admit_artifact_descriptor(
        descriptors[0],
        observed=None if observed_members is None else observed_members[0],
        owned=_owned,
    )
    reloc_member_identity = _admit_artifact_descriptor(
        descriptors[1],
        observed=None if observed_members is None else observed_members[1],
        owned=_owned,
    )
    return RuntimeWasmGeneration(
        manifest=manifest,
        shared=shared_member_identity.path,
        reloc=reloc_member_identity.path,
        shared_identity=expected_shared_identity,
        reloc_identity=expected_reloc_identity,
        shared_member_identity=shared_member_identity,
        reloc_member_identity=reloc_member_identity,
        payload=payload,
        receipt_identity=receipt_identity,
    )


def _validate_generation_payload(
    manifest: Path,
    payload: object,
    *,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
    receipt_identity: StableRegularFileIdentity | None = None,
    observed_members: tuple[StableRegularFileIdentity, StableRegularFileIdentity]
    | None = None,
    _owned: ExitStack | None = None,
) -> SourceRuntimeWasmGeneration | None:
    try:
        product, cargo_environment = _source_envelope(payload)
    except (TypeError, ValueError):
        return None
    admitted = _validate_product_payload(
        manifest,
        product,
        expected_shared_identity=expected_shared_identity,
        expected_reloc_identity=expected_reloc_identity,
        receipt_identity=receipt_identity,
        observed_members=observed_members,
        _owned=_owned,
    )
    if admitted is None:
        return None
    return SourceRuntimeWasmGeneration(
        manifest=admitted.manifest,
        shared=admitted.shared,
        reloc=admitted.reloc,
        shared_identity=admitted.shared_identity,
        reloc_identity=admitted.reloc_identity,
        shared_member_identity=admitted.shared_member_identity,
        reloc_member_identity=admitted.reloc_member_identity,
        payload=admitted.payload,
        receipt_identity=admitted.receipt_identity,
        cargo_environment=cargo_environment,
    )


def hydrate_runtime_wasm_product(
    *,
    source_manifest: Path,
    dest_shared: Path,
    dest_reloc: Path,
    expected_shared_identity: RuntimeBuildIdentity,
    expected_reloc_identity: RuntimeBuildIdentity,
    source_generation: RuntimeWasmGeneration | None = None,
) -> RuntimeWasmGeneration:
    """Hydrate an installed product without creating source-build evidence."""

    if source_generation is None:
        payload = read_runtime_wasm_product(
            source_manifest,
            expected_shared_identity=expected_shared_identity,
            expected_reloc_identity=expected_reloc_identity,
        )
    else:
        if type(source_generation) is not RuntimeWasmGeneration:
            raise ValueError("product hydration requires an installed runtime product")
        payload = source_generation
        if (
            payload.manifest != source_manifest
            or payload.shared_identity != expected_shared_identity
            or payload.reloc_identity != expected_reloc_identity
        ):
            raise ValueError("runtime WASM hydration source changed selection")
        payload.verify_members()
    if payload is None:
        raise ValueError(
            "runtime WASM installed product does not match trusted identity"
        )
    source_member_shared = payload.shared
    source_member_reloc = payload.reloc
    receipts = _generation_receipts(payload.payload.get("receipts"))
    if receipts is None:
        raise ValueError("validated runtime generation receipts are invalid")
    return publish_runtime_wasm_product(
        dest_shared,
        dest_reloc,
        shared_identity=expected_shared_identity,
        reloc_identity=expected_reloc_identity,
        source_shared=source_member_shared,
        source_reloc=source_member_reloc,
        expected_source_receipts=receipts,
        source_observations=(
            payload.shared_member_identity,
            payload.reloc_member_identity,
        ),
    )
