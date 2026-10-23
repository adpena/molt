"""Pure, exact runtime compilation/family/member receipt authority."""

from __future__ import annotations

import math
import re
from molt.wasi_sdk_identity import WASI_C_ABI_PLAN_ENV, validate_wasi_c_abi_identity

from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path, PurePosixPath, PureWindowsPath
from types import MappingProxyType
from collections.abc import Iterator, Mapping, Sequence
from typing import cast, overload

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.runtime_artifact_selection import (
    RuntimeArtifactSelection,
    RuntimeCrateType,
)
from molt.exact_json import (
    canonical_json_bytes,
    canonical_json_sha256,
    read_exact,
    string_keyed_mapping,
)
from molt.python_identity_common import _valid_sha256

_SCHEMA = "molt.runtime-build-member-identity.v5"
RUNTIME_ARTIFACT_METADATA_MAX_BYTES = 16 * 1024 * 1024
_FAMILY_SCHEMA = "molt.runtime-build-family.v5"
_TOOLCHAIN_MANIFEST_SCHEMA = "molt.runtime-toolchain-content.v4"
_RUNTIME_BUILD_OUTER_FIELDS = {
    "schema",
    "digest",
    "compile_digest",
    "family_digest",
    "payload",
}
_RUNTIME_BUILD_MEMBER_REFERENCE_FIELDS = {"digest", "compile_digest", "member_kind"}
_RUNTIME_BUILD_FAMILY_FIELDS = {
    "schema",
    "compile_digest",
    "compile",
    "members",
}
_RUNTIME_BUILD_COMPILE_FIELDS = {"sources", "toolchain", "common_config"}
_RUNTIME_BUILD_MEMBER_FIELDS = {
    "kind",
    "resolved_rustflags",
    "link_args",
    "publication",
}


@dataclass(frozen=True, slots=True, eq=False)
class _FrozenJsonObject(Mapping[str, object]):
    """Owned immutable JSON object; hashes and admissions share its lifetime."""

    _values: Mapping[str, object]
    _cached_digest: str | None = field(default=None, init=False, repr=False)
    _validated_schemas: frozenset[tuple[str, ...]] = field(
        default=frozenset(), init=False, repr=False
    )

    def __post_init__(self) -> None:
        if not isinstance(self._values, Mapping):
            raise TypeError("runtime identity JSON object keys must be strings")
        owned: dict[str, object] = {}
        for key, item in self._values.items():
            # A str subclass can carry mutable state and override hash/equality.
            # Check each key in the same traversal that owns its frozen value.
            if type(key) is not str:
                raise TypeError("runtime identity JSON object keys must be strings")
            owned[key] = _freeze_json(item)
        object.__setattr__(
            self,
            "_values",
            MappingProxyType(owned),
        )

    def __getitem__(self, key: str) -> object:
        return self._values[key]

    def __iter__(self) -> Iterator[str]:
        return iter(self._values)

    def __len__(self) -> int:
        return len(self._values)


@overload
def _freeze_json(value: Mapping[str, object]) -> Mapping[str, object]: ...


@overload
def _freeze_json(value: object) -> object: ...


def _freeze_json(value: object) -> object:
    # Only our exact owning type proves recursive immutability. A caller's
    # mapping proxy can still wrap a mutable dictionary and must be copied.
    if type(value) is _FrozenJsonObject:
        return value
    if isinstance(value, Mapping):
        return _FrozenJsonObject(value)
    if isinstance(value, (list, tuple)):
        return tuple(_freeze_json(item) for item in value)
    if type(value) is float and not math.isfinite(value):
        raise ValueError("runtime identity contains non-finite JSON number")
    # Only exact JSON scalars are recursively immutable facts. Subclasses may
    # carry mutable behavior; never retain them or call their coercion methods.
    if value is None or type(value) in (str, int, float, bool):
        return value
    raise TypeError(f"runtime identity contains non-JSON value: {type(value).__name__}")


def _thaw_json(value: object) -> object:
    if isinstance(value, Mapping):
        typed = string_keyed_mapping(value)
        if typed is None:
            raise TypeError("runtime identity JSON object keys must be strings")
        return {key: _thaw_json(item) for key, item in typed.items()}
    if isinstance(value, (list, tuple)):
        return [_thaw_json(item) for item in value]
    return value


def _frozen_json_projection(value: object) -> dict[str, object]:
    # The canonical encoder owns traversal/number formatting. Project just one
    # immutable object, rather than thawing its entire descendants in Python.
    if type(value) is not _FrozenJsonObject:
        raise TypeError(
            f"runtime identity contains non-JSON value: {type(value).__name__}"
        )
    return dict(value._values)


def _canonical_json(value: object) -> str:
    return canonical_json_bytes(
        _freeze_json(value), default=_frozen_json_projection
    ).decode("utf-8")


def runtime_family_digest(family: object) -> str:
    """The digest a member identity's ``family_digest`` names for ``family``."""
    return _digest(family)


def _digest(value: object) -> str:
    frozen = _freeze_json(value)
    if type(frozen) is _FrozenJsonObject and frozen._cached_digest is not None:
        return frozen._cached_digest
    digest = canonical_json_sha256(frozen, default=_frozen_json_projection)
    if type(frozen) is _FrozenJsonObject:
        # Only a scalar is retained, never canonical bytes or a second graph.
        object.__setattr__(frozen, "_cached_digest", digest)
    return digest


def _has_schema_admission(value: object, schema: tuple[str, ...]) -> bool:
    # Foreign mappings and subclasses cannot attest recursive ownership, even
    # when they expose a copied success marker. Wire content never carries it.
    return type(value) is _FrozenJsonObject and schema in value._validated_schemas


def _remember_schema_admission(value: object, schema: tuple[str, ...]) -> None:
    if type(value) is _FrozenJsonObject:
        # Store only successful schema facts on this captured graph. A target
        # dependent fact includes its target; no live inputs or failures persist.
        object.__setattr__(
            value, "_validated_schemas", value._validated_schemas | {schema}
        )


def _portable_filename(value: str) -> bool:
    return (
        value not in {".", ".."}
        and PurePosixPath(value).name == value
        and PureWindowsPath(value).name == value
        and not PureWindowsPath(value).drive
        and "\x00" not in value
    )


def _string_sequence(value: object, *, label: str) -> tuple[str, ...]:
    if not isinstance(value, (list, tuple)) or not all(
        isinstance(item, str) and item and "\x00" not in item for item in value
    ):
        raise ValueError(f"runtime {label} must be an array of non-empty strings")
    return tuple(cast(Sequence[str], value))


def _validated_common_config(payload: object) -> Mapping[str, object]:
    value = string_keyed_mapping(payload)
    required = {
        "cargo_profile",
        "target_triple",
        "runtime_features",
        "producer_artifact_selection",
        "cargo_command",
        "environment",
        "build_script_environment",
    }
    optional = {"base_rustflags", "ambient_c_build_environment"}
    if (
        value is None
        or not required.issubset(value)
        or set(value) - required - optional
    ):
        raise ValueError("runtime common configuration shape is invalid")
    for field_name in ("cargo_profile", "target_triple", "producer_artifact_selection"):
        if not isinstance(value[field_name], str) or not value[field_name]:
            raise ValueError(f"runtime common configuration {field_name} is invalid")
    features = _string_sequence(value["runtime_features"], label="features")
    if features != tuple(sorted(set(features))):
        raise ValueError("runtime features must be sorted and unique")
    command = _string_sequence(value["cargo_command"], label="Cargo command")
    if not command or command[0] != "cargo":
        raise ValueError("runtime Cargo command must use its content-attested role")
    if "base_rustflags" in value:
        _string_sequence(value["base_rustflags"], label="base Rust flags")
    environment = string_keyed_mapping(value["environment"])
    if environment is None or any(
        not key or not _valid_sha256(item) for key, item in environment.items()
    ):
        raise ValueError("runtime build environment identity is invalid")
    if "ambient_c_build_environment" in value:
        ambient = string_keyed_mapping(value["ambient_c_build_environment"])
        if ambient is None:
            raise ValueError("runtime C build environment is invalid")
        for key, item in ambient.items():
            if not key:
                raise ValueError("runtime C environment key is empty")
            _string_sequence(item, label=f"C environment {key}")
    scripts = string_keyed_mapping(value["build_script_environment"])
    if scripts is None or not scripts:
        raise ValueError("runtime build-script environment is invalid")
    return value


def _validated_build_script_environment(payload: object, *, target: str) -> None:
    value = string_keyed_mapping(payload)
    if value is None:
        raise ValueError("runtime build-script environment must be an object")
    schema = value.get("schema")
    fields = {"schema", WASI_C_ABI_PLAN_ENV}
    runtime = schema == "molt.runtime-build-script-environment.v4"
    if runtime:
        fields |= {
            "MOLT_WASM_CPYTHON_ABI_EXPORTS",
            "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS",
        }
    elif schema != "molt.cpython-abi-build-script-environment.v4":
        raise ValueError("runtime build-script schema is invalid")
    if set(value) != fields:
        raise ValueError("runtime build-script environment shape is invalid")
    c_abi = value[WASI_C_ABI_PLAN_ENV]
    if target in {"wasm32-wasip1", "wasm32-unknown-unknown"}:
        if (
            not isinstance(c_abi, Mapping)
            or set(c_abi) != {"state", "content"}
            or c_abi["state"] != "selected"
        ):
            raise ValueError("WASM build-script requires selected C-runtime content")
        validate_wasi_c_abi_identity(c_abi["content"])
    elif c_abi != {"state": "ignored-for-target"}:
        raise ValueError("non-WASI build-script has WASI C-runtime inputs")
    if not runtime:
        return
    wasm = target.startswith("wasm32-")
    symbols: list[tuple[str, ...]] = []
    for name in ("MOLT_WASM_CPYTHON_ABI_EXPORTS", "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS"):
        if not wasm:
            if value[name] != "ignored-for-target":
                raise ValueError("native runtime has WASM build-script symbols")
            continue
        items = _string_sequence(value[name], label=name)
        if items != tuple(sorted(set(items))) or any(
            re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", item) is None for item in items
        ):
            raise ValueError("runtime build-script symbol sequence is invalid")
        symbols.append(items)
    if wasm and not set(symbols[1]).issubset(symbols[0]):
        raise ValueError("runtime build-script data exports are unowned")


def _validated_tree_summary(payload: object, *, label: str) -> Mapping[str, object]:
    value = string_keyed_mapping(payload)
    if value is None or set(value) != {
        "digest",
        "file_count",
        "missing",
        "roots",
        "total_size",
    }:
        raise ValueError(f"runtime {label} tree identity shape is invalid")
    roots = value.get("roots")
    missing = value.get("missing")
    file_count = value.get("file_count")
    total_size = value.get("total_size")
    if (
        not _valid_sha256(value.get("digest"))
        or not isinstance(file_count, int)
        or isinstance(file_count, bool)
        or file_count < 0
        or not isinstance(total_size, int)
        or isinstance(total_size, bool)
        or total_size < 0
        or not isinstance(roots, (list, tuple))
        or not all(isinstance(item, str) and item for item in roots)
        or list(roots) != sorted(set(roots))
        or not isinstance(missing, (list, tuple))
        or not all(isinstance(item, str) and item for item in missing)
        or list(missing) != sorted(set(missing))
        or not set(missing).issubset(roots)
    ):
        raise ValueError(f"runtime {label} tree identity is invalid")
    return value


def _validated_executable_content_identity(
    payload: object,
    *,
    logical_name: str,
) -> Mapping[str, object]:
    value = string_keyed_mapping(payload)
    base_fields = {
        "content_filename",
        "entrypoint",
        "logical_name",
        "sha256",
        "size",
    }
    if value is None or frozenset(value) not in {
        frozenset(base_fields),
        frozenset({*base_fields, "version"}),
    }:
        raise ValueError(f"runtime tool {logical_name} identity shape is invalid")
    entrypoint = value.get("entrypoint")
    content_filename = value.get("content_filename")
    size = value.get("size")
    version = value.get("version")
    if (
        value.get("logical_name") != logical_name
        or not isinstance(entrypoint, str)
        or not entrypoint
        or not _portable_filename(entrypoint)
        or not isinstance(content_filename, str)
        or not content_filename
        or not _portable_filename(content_filename)
        or not isinstance(size, int)
        or isinstance(size, bool)
        or size < 0
        or not _valid_sha256(value.get("sha256"))
        or ("version" in value and (not isinstance(version, str) or not version))
    ):
        raise ValueError(f"runtime tool {logical_name} identity is invalid")
    return value


def _validated_runtime_toolchain_content(
    payload: object,
    *,
    target_triple: str,
) -> Mapping[str, object]:
    if type(target_triple) is not str or not target_triple:
        raise ValueError("runtime toolchain target is invalid")
    admission = (_TOOLCHAIN_MANIFEST_SCHEMA, target_triple)
    if _has_schema_admission(payload, admission):
        return cast(Mapping[str, object], payload)
    value = string_keyed_mapping(payload)
    if value is None or set(value) != {
        "archives",
        "cargo_configuration",
        "effective_target",
        "rust_resources",
        "sysroots",
        "tools",
        "wrappers",
    }:
        raise ValueError("runtime toolchain content shape is invalid")
    tools = string_keyed_mapping(value.get("tools"))
    allowed_tools = {
        "ar",
        "cargo",
        "cc",
        "cxx",
        "final_linker",
        "host_cc",
        "host_cxx",
        "host_ar",
        "host_ranlib",
        "linker",
        "ranlib",
        "rustc",
        "wasm_linker",
    }
    if (
        tools is None
        or not {"cargo", "rustc"}.issubset(tools)
        or not set(tools).issubset(allowed_tools)
    ):
        raise ValueError("runtime toolchain tools are invalid")
    for name, raw in tools.items():
        _validated_executable_content_identity(raw, logical_name=name)
    wrappers = string_keyed_mapping(value.get("wrappers"))
    if wrappers is None:
        raise ValueError("runtime toolchain wrapper identities are invalid")
    for name, raw in wrappers.items():
        if name not in {"RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}:
            raise ValueError("runtime toolchain wrapper name is invalid")
        _validated_executable_content_identity(raw, logical_name=name.casefold())
    _validated_tree_summary(
        value.get("cargo_configuration"), label="Cargo configuration"
    )
    resources = string_keyed_mapping(value.get("rust_resources"))
    if resources is None or set(resources) != {
        "content",
        "host_triple",
        "selected_target",
    }:
        raise ValueError("runtime Rust resource identity shape is invalid")
    host_triple = resources.get("host_triple")
    selected_target = resources.get("selected_target")
    effective_target = value.get("effective_target")
    if (
        not isinstance(host_triple, str)
        or not host_triple
        or not isinstance(selected_target, str)
        or not selected_target
        or effective_target != selected_target
        or (target_triple == "native" and selected_target != host_triple)
        or (target_triple != "native" and selected_target != target_triple)
    ):
        raise ValueError("runtime Rust target identity is invalid")
    _validated_tree_summary(resources.get("content"), label="Rust resources")

    sysroots = string_keyed_mapping(value.get("sysroots"))
    expected_sysroots = {"wasi"} if target_triple.startswith("wasm32-") else set()
    if sysroots is None or set(sysroots) != expected_sysroots:
        raise ValueError("runtime sysroot identity is invalid")
    for raw in sysroots.values():
        from molt.wasi_sdk_identity import validate_wasi_c_abi_identity

        validate_wasi_c_abi_identity(raw)
    archives = value.get("archives")
    if not isinstance(archives, (list, tuple)):
        raise ValueError("runtime archive identities are invalid")
    archive_names: set[str] = set()
    for raw in archives:
        archive = string_keyed_mapping(raw)
        if archive is None or set(archive) != {"logical_name", "sha256", "size"}:
            raise ValueError("runtime archive identity shape is invalid")
        name = archive.get("logical_name")
        size = archive.get("size")
        if (
            not isinstance(name, str)
            or not name
            or name in archive_names
            or not isinstance(size, int)
            or isinstance(size, bool)
            or size < 0
            or not _valid_sha256(archive.get("sha256"))
        ):
            raise ValueError("runtime archive identity is invalid")
        archive_names.add(name)
    if target_triple == "native" and archives:
        raise ValueError("native runtime toolchain has target archives")
    _remember_schema_admission(payload, admission)
    return value


def _validated_toolchain_manifest_payload(
    payload: object,
) -> Mapping[str, object]:
    admission = (_TOOLCHAIN_MANIFEST_SCHEMA,)
    if _has_schema_admission(payload, admission):
        return cast(Mapping[str, object], payload)
    value = string_keyed_mapping(payload)
    if value is None or set(value) != {"target_triple", "toolchain"}:
        raise ValueError("runtime toolchain manifest content shape is invalid")
    target = value.get("target_triple")
    toolchain = string_keyed_mapping(value.get("toolchain"))
    if not isinstance(target, str) or not target or toolchain is None or not toolchain:
        raise ValueError("runtime toolchain manifest content is invalid")
    _validated_runtime_toolchain_content(toolchain, target_triple=target)
    _remember_schema_admission(payload, admission)
    return value


def _validated_runtime_build_payload(
    payload: object,
    *,
    digest: object,
    compile_digest: object,
    family_digest: object,
) -> Mapping[str, object]:
    value = string_keyed_mapping(payload)
    if value is None or set(value) != {"family", "member_kind"}:
        raise ValueError("runtime build identity payload shape is invalid")
    family = string_keyed_mapping(value.get("family"))
    member_kind = value.get("member_kind")
    if (
        family is None
        or set(family) != _RUNTIME_BUILD_FAMILY_FIELDS
        or family.get("schema") != _FAMILY_SCHEMA
        or not isinstance(member_kind, str)
        or not member_kind
    ):
        raise ValueError("runtime build identity family shape is invalid")
    compile_payload = string_keyed_mapping(family.get("compile"))
    members = string_keyed_mapping(family.get("members"))
    if (
        compile_payload is None
        or set(compile_payload) != _RUNTIME_BUILD_COMPILE_FIELDS
        or string_keyed_mapping(compile_payload.get("sources")) is None
        or string_keyed_mapping(compile_payload.get("toolchain")) is None
        or string_keyed_mapping(compile_payload.get("common_config")) is None
        or members is None
        or not members
    ):
        raise ValueError("runtime build identity compile/family shape is invalid")
    config = _validated_common_config(compile_payload["common_config"])
    target = cast(str, config["target_triple"])
    _validated_tree_summary(compile_payload["sources"], label="source")
    _validated_runtime_toolchain_content(
        compile_payload["toolchain"], target_triple=target
    )
    _validated_build_script_environment(
        config["build_script_environment"],
        target=target,
    )
    expected_members = (
        ({"shared", "reloc"}, {"staticlib"})
        if target.startswith("wasm32-")
        else ({"staticlib"},)
    )
    if set(members) not in expected_members:
        raise ValueError("runtime family member kinds do not match the target")
    if set(members) == {"shared", "reloc"}:
        tools = cast(Mapping[str, object], compile_payload["toolchain"])["tools"]
        if not {"cc", "cxx", "ar", "ranlib", "wasm_linker"}.issubset(
            cast(Mapping[str, object], tools)
        ):
            raise ValueError("runtime WASM family toolchain is incomplete")
        archives = cast(Mapping[str, object], compile_payload["toolchain"])["archives"]
        if tuple(
            cast(Mapping[str, object], item)["logical_name"]
            for item in cast(Sequence[object], archives)
        ) != (
            "wasi-libc",
            "wasi-long-double",
            "clang-rt-builtins",
        ):
            raise ValueError("runtime WASM family archive closure is incomplete")
    for kind, raw_member in members.items():
        member = string_keyed_mapping(raw_member)
        if (
            not kind
            or member is None
            or set(member) != _RUNTIME_BUILD_MEMBER_FIELDS
            or member.get("kind") != kind
        ):
            raise ValueError("runtime build identity member shape is invalid")
        action = RuntimePublicationAction.from_dict(member.get("publication"))
        expected_transform = (
            RuntimePublicationTransform.SHARED
            if kind == "shared"
            else RuntimePublicationTransform.RELOC
            if kind == "reloc"
            else RuntimePublicationTransform.WASM_STATICLIB
            if target.startswith("wasm32-")
            else RuntimePublicationTransform.NATIVE_STATICLIB
        )
        if action.transform is not expected_transform:
            raise ValueError("runtime publication action does not match its member")
        for field_name in ("resolved_rustflags", "link_args"):
            items = member.get(field_name)
            if not isinstance(items, (list, tuple)) or not all(
                isinstance(item, str) and item for item in items
            ):
                raise ValueError("runtime build identity member shape is invalid")
    member = string_keyed_mapping(members.get(member_kind))
    if member is None:
        raise ValueError("runtime build identity selected member is absent")
    if not all(
        type(item) is str and _valid_sha256(item)
        for item in (digest, compile_digest, family_digest)
    ):
        raise ValueError("runtime build identity digest shape is invalid")
    if (
        digest != _digest(value)
        or compile_digest != _digest(compile_payload)
        or family.get("compile_digest") != compile_digest
        or family_digest != _digest(family)
    ):
        raise ValueError("runtime build identity digest is invalid")
    return value


@dataclass(frozen=True)
class RuntimeToolchainContentManifest(Mapping[str, object]):
    digest: str
    payload: Mapping[str, object]

    def __post_init__(self) -> None:
        frozen = _freeze_json(self.payload)
        _validated_toolchain_manifest_payload(frozen)
        if (
            type(self.digest) is not str
            or not _valid_sha256(self.digest)
            or self.digest != _digest(frozen)
        ):
            raise ValueError("runtime toolchain manifest digest is invalid")
        object.__setattr__(self, "payload", frozen)

    def __iter__(self) -> Iterator[str]:
        return iter(("schema", "digest", "payload"))

    def __len__(self) -> int:
        return 3

    def __getitem__(self, key: str) -> object:
        # Scalar lookup must not materialize the full source/toolchain closure.
        if key == "schema":
            return _TOOLCHAIN_MANIFEST_SCHEMA
        if key == "digest":
            return self.digest
        if key == "payload":
            return _thaw_json(self.payload)
        raise KeyError(key)

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _TOOLCHAIN_MANIFEST_SCHEMA,
            "digest": self.digest,
            "payload": _thaw_json(self.payload),
        }

    @classmethod
    def from_payload(
        cls, payload: Mapping[str, object]
    ) -> RuntimeToolchainContentManifest:
        """Own one graph for canonical hashing and complete schema admission."""
        frozen = _freeze_json(payload)
        return cls(digest=_digest(frozen), payload=frozen)

    @classmethod
    def from_dict(cls, value: object) -> RuntimeToolchainContentManifest:
        # Exact instances are recursively frozen and validated at construction.
        # Foreign mappings and subclasses still cross the full admission boundary.
        if (
            cls is RuntimeToolchainContentManifest
            and type(value) is RuntimeToolchainContentManifest
        ):
            return cast(RuntimeToolchainContentManifest, value)
        outer = string_keyed_mapping(value)
        if (
            outer is None
            or any(type(key) is not str for key in outer)
            or set(outer) != {"schema", "digest", "payload"}
            or type(outer.get("schema")) is not str
            or outer.get("schema") != _TOOLCHAIN_MANIFEST_SCHEMA
        ):
            raise ValueError("runtime toolchain manifest schema is invalid")
        digest = outer.get("digest")
        payload = string_keyed_mapping(outer.get("payload"))
        if type(digest) is not str or not _valid_sha256(digest) or payload is None:
            raise ValueError("runtime toolchain manifest digest is invalid")
        frozen = _freeze_json(payload)
        if digest != _digest(frozen):
            raise ValueError("runtime toolchain manifest digest is invalid")
        assert isinstance(digest, str)
        return cls(digest=digest, payload=frozen)

    @classmethod
    def read(cls, path: Path) -> RuntimeToolchainContentManifest:
        try:
            value = read_exact(
                path,
                max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
                label="runtime toolchain manifest",
            )
        except (OSError, UnicodeError, ValueError) as exc:
            raise ValueError(
                f"runtime toolchain manifest is unreadable: {path.name}: {exc}"
            ) from exc
        return cls.from_dict(value)

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_text(path, _canonical_json(self.to_dict()) + "\n")


class RuntimeBuildIdentityMismatch(ValueError):
    """A valid recorded identity differs from the caller's admitted identity."""


@dataclass(frozen=True)
class RuntimeBuildIdentity(Mapping[str, object]):
    digest: str
    compile_digest: str
    family_digest: str
    payload: Mapping[str, object]

    @property
    def effective_target(self) -> str:
        """Actual Cargo target, including the resolved implicit-native host."""
        family = cast(Mapping[str, object], self.payload["family"])
        compilation = cast(Mapping[str, object], family["compile"])
        toolchain = cast(Mapping[str, object], compilation["toolchain"])
        return cast(str, toolchain["effective_target"])

    @property
    def runtime_features(self) -> tuple[str, ...]:
        """The recorded Cargo feature request, including its default marker."""
        family = cast(Mapping[str, object], self.payload["family"])
        compilation = cast(Mapping[str, object], family["compile"])
        configuration = cast(Mapping[str, object], compilation["common_config"])
        return tuple(cast(Sequence[str], configuration["runtime_features"]))

    def __post_init__(self) -> None:
        frozen = _freeze_json(self.payload)
        _validated_runtime_build_payload(
            frozen,
            digest=self.digest,
            compile_digest=self.compile_digest,
            family_digest=self.family_digest,
        )
        object.__setattr__(self, "payload", frozen)

    def __iter__(self) -> Iterator[str]:
        return iter(("schema", "digest", "compile_digest", "family_digest", "payload"))

    def __len__(self) -> int:
        return 5

    def __getitem__(self, key: str) -> object:
        # Scalar lookup must not materialize the full source/toolchain closure.
        if key == "schema":
            return _SCHEMA
        if key == "digest":
            return self.digest
        if key == "compile_digest":
            return self.compile_digest
        if key == "family_digest":
            return self.family_digest
        if key == "payload":
            return _thaw_json(self.payload)
        raise KeyError(key)

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "digest": self.digest,
            "compile_digest": self.compile_digest,
            "family_digest": self.family_digest,
            "payload": _thaw_json(self.payload),
        }

    def member_reference(self) -> dict[str, object]:
        """This member's identity without the family payload its siblings share.

        A record that holds the family once pairs it with each member's
        reference; ``from_member_reference`` rebuilds and validates the member.
        """
        return {
            "digest": self.digest,
            "compile_digest": self.compile_digest,
            "member_kind": self.payload["member_kind"],
        }

    def family_payload(self) -> dict[str, object]:
        """The family payload, mutable, for a record that stores it once."""
        return cast(dict[str, object], _thaw_json(self.payload["family"]))

    @classmethod
    def from_member_reference(
        cls, reference: object, *, family: object, family_digest: object
    ) -> RuntimeBuildIdentity:
        """Rebuild one member identity from its reference and the shared family."""
        value = string_keyed_mapping(reference)
        if value is None or set(value) != _RUNTIME_BUILD_MEMBER_REFERENCE_FIELDS:
            raise ValueError("runtime build member reference shape is invalid")
        return cls.from_dict(
            {
                "schema": _SCHEMA,
                "digest": value.get("digest"),
                "compile_digest": value.get("compile_digest"),
                "family_digest": family_digest,
                "payload": {"family": family, "member_kind": value.get("member_kind")},
            }
        )

    @property
    def toolchain_manifest(self) -> RuntimeToolchainContentManifest:
        """Portable evidence projected from this captured family, not live input."""
        family = cast(Mapping[str, object], self.payload["family"])
        compilation = cast(Mapping[str, object], family["compile"])
        configuration = cast(Mapping[str, object], compilation["common_config"])
        payload = {
            "target_triple": configuration["target_triple"],
            "toolchain": compilation["toolchain"],
        }
        return RuntimeToolchainContentManifest.from_payload(payload)

    @classmethod
    def from_dict(
        cls, value: object, *, expected: RuntimeBuildIdentity | None = None
    ) -> RuntimeBuildIdentity:
        # A caller-trusted identity already proves the complete schema. Matching
        # a wire claim requires the same canonical content, not a second semantic
        # admission of its toolchain/family descendants. Return that owner;
        # the untrusted mapping never becomes an authority or a retained cache.
        if expected is not None:
            expected = RuntimeBuildIdentity.from_dict(expected)
        # Exact instances are recursively frozen and validated at construction.
        # Foreign mappings and subclasses still cross the full admission boundary.
        if cls is RuntimeBuildIdentity and type(value) is RuntimeBuildIdentity:
            admitted = cast(RuntimeBuildIdentity, value)
            if expected is not None:
                if (
                    admitted.digest != expected.digest
                    or admitted.compile_digest != expected.compile_digest
                    or admitted.family_digest != expected.family_digest
                ):
                    raise RuntimeBuildIdentityMismatch(
                        "runtime build identity mismatch"
                    )
                return expected
            return admitted
        outer = string_keyed_mapping(value)
        if (
            outer is None
            or any(type(key) is not str for key in outer)
            or set(outer) != _RUNTIME_BUILD_OUTER_FIELDS
            or type(outer.get("schema")) is not str
            or outer.get("schema") != _SCHEMA
        ):
            raise ValueError("runtime build identity schema is invalid")
        payload = string_keyed_mapping(outer.get("payload"))
        digest = outer.get("digest")
        compile_digest = outer.get("compile_digest")
        family_digest = outer.get("family_digest")
        if (
            payload is None
            or type(digest) is not str
            or type(compile_digest) is not str
            or type(family_digest) is not str
        ):
            raise ValueError("runtime build identity is incomplete")
        if expected is not None:
            if (
                digest != expected.digest
                or compile_digest != expected.compile_digest
                or family_digest != expected.family_digest
                or _digest(payload) != expected.digest
            ):
                # Preserve precise malformed-input diagnostics on rejection. A
                # valid but different claim has a distinct typed failure; only
                # the failure path needs another complete semantic admission.
                RuntimeBuildIdentity.from_dict(value)
                raise RuntimeBuildIdentityMismatch("runtime build identity mismatch")
            return expected
        return cls(
            digest=digest,
            compile_digest=compile_digest,
            family_digest=family_digest,
            payload=payload,
        )


def runtime_build_fingerprint(
    build_identity: RuntimeBuildIdentity,
    *,
    scope: str = "member",
) -> dict[str, object]:
    """Project an exact build identity into the generic artifact-sidecar protocol."""

    build_identity = RuntimeBuildIdentity.from_dict(build_identity)
    if scope == "member":
        content_digest = build_identity.digest
        return {
            "hash": content_digest,
            "rustc": build_identity.compile_digest,
            "inputs_digest": build_identity.compile_digest,
            "meta_digest": build_identity.family_digest,
            "build_identity": build_identity.to_dict(),
            "build_identity_scope": scope,
        }
    elif scope == "member-output":
        family = cast(Mapping[str, object], build_identity.payload["family"])
        member_kind = cast(str, build_identity.payload["member_kind"])
        members = cast(Mapping[str, object], family["members"])
        output_payload = {
            "schema": "molt.runtime-build-member-output.v1",
            "compile_digest": build_identity.compile_digest,
            "compile": family["compile"],
            "member": members[member_kind],
        }
        content_digest = _digest(output_payload)
        return {
            "hash": content_digest,
            "rustc": build_identity.compile_digest,
            "inputs_digest": build_identity.compile_digest,
            "meta_digest": content_digest,
            "build_identity": build_identity.to_dict(),
            "build_identity_scope": scope,
        }
    elif scope == "compile":
        content_digest = build_identity.compile_digest
    else:
        raise ValueError(f"unknown runtime build fingerprint scope: {scope}")
    return {
        "hash": content_digest,
        "rustc": build_identity.compile_digest,
        "inputs_digest": build_identity.compile_digest,
        "meta_digest": build_identity.compile_digest,
        "build_identity": build_identity.to_dict(),
        "build_identity_scope": scope,
    }


def require_native_runtime_staticlib_identity(
    value: RuntimeBuildIdentity | object,
    *,
    cargo_profile: str,
    target_triple: str | None,
    artifact_selection: RuntimeArtifactSelection,
) -> RuntimeBuildIdentity:
    """Return a validated native staticlib receipt or fail closed.

    Native-link admission uses this accessor so profile, target, Cargo artifact
    selection, and member-output semantics cannot be reimplemented by consumers.
    """

    build_identity = RuntimeBuildIdentity.from_dict(value)
    family = cast(Mapping[str, object], build_identity.payload["family"])
    compile_payload = cast(Mapping[str, object], family["compile"])
    common_config = string_keyed_mapping(compile_payload.get("common_config"))
    members = string_keyed_mapping(family.get("members"))
    member = (
        string_keyed_mapping(members.get("staticlib")) if members is not None else None
    )
    logical_target = target_triple or "native"
    if (
        not cargo_profile
        or artifact_selection.crate_types != (RuntimeCrateType.STATICLIB,)
        or common_config is None
        or common_config.get("cargo_profile") != cargo_profile
        or common_config.get("target_triple") != logical_target
        or common_config.get("producer_artifact_selection")
        != artifact_selection.source_identity
        or build_identity.payload.get("member_kind") != "staticlib"
        or members is None
        or set(members) != {"staticlib"}
        or member is None
        or member.get("kind") != "staticlib"
        or tuple(cast(Sequence[object], member.get("link_args", ())))
        != ("--print", "native-static-libs")
        or RuntimePublicationAction.from_dict(member.get("publication")).transform
        is not RuntimePublicationTransform.NATIVE_STATICLIB
    ):
        raise ValueError(
            "runtime build identity does not match the requested native staticlib"
        )
    return build_identity


@dataclass(frozen=True)
class RuntimeBuildMemberPlan:
    kind: str
    resolved_rustflags: tuple[str, ...]
    link_args: tuple[str, ...]
    publication: RuntimePublicationAction

    def __post_init__(self) -> None:
        if type(self.publication) is not RuntimePublicationAction:
            raise TypeError("runtime member requires its resolved publication action")
        for field_name in ("resolved_rustflags", "link_args"):
            tokens = getattr(self, field_name)
            if type(tokens) is not tuple or not all(
                type(token) is str and token for token in tokens
            ):
                raise TypeError(
                    f"runtime build member {field_name} must be a tuple of tokens"
                )


class RuntimePublicationTransform(Enum):
    """Closed publication protocols; changes to byte semantics version the protocol."""

    NATIVE_STATICLIB = "native-staticlib-and-link-manifest-v1"
    WASM_STATICLIB = "wasm-cpython-abi-staticlib-v1"
    SHARED = "shared-runtime-publication-v3"
    RELOC = "relocatable-runtime-publication-v2"


def runtime_publication_executor_paths(
    transform: RuntimePublicationTransform,
) -> tuple[str, ...]:
    """Actual byte/sidecar executors, not the transitive planning import graph.

    Native Cargo-note interpretation is retained, not rederived on reuse. WASM
    rewrites and response encoding likewise produce bytes. Include their actual
    dispatchers; copy/atomic transport and validators that run on reuse do not
    define additional output semantics. Cargo compilation has a separate key.
    """
    match transform:
        case RuntimePublicationTransform.NATIVE_STATICLIB:
            return (
                "src/molt/cli/diagnostic_text.py",
                "src/molt/cli/native_link_custody.py",
                "src/molt/cli/native_link_manifest.py",
                "src/molt/cli/runtime_native_generation.py",
                "src/molt/native_target.py",
                "src/molt/native_target_shape.py",
            )
        case RuntimePublicationTransform.SHARED:
            return (
                "src/molt/cli/runtime_wasm_build.py",
                "src/molt/wasm_artifact.py",
            )
        case RuntimePublicationTransform.RELOC:
            return (
                "src/molt/cli/runtime_identity_schema.py",
                "src/molt/cli/runtime_wasm_build.py",
                "src/molt/cli/runtime_wasm_build_support.py",
                "src/molt/cli/wasm_link_args.py",
                "src/molt/wasm_artifact.py",
            )
        case RuntimePublicationTransform.WASM_STATICLIB:
            # Cargo emits the complete CAPI archive; publication preserves bytes.
            return ()
    raise ValueError("invalid runtime publication transform")


class RuntimePublicationInput(Enum):
    STATICLIB = "staticlib"
    LONG_DOUBLE = "long_double"
    COMPILER_RT = "compiler_rt"
    LIBC = "libc"
    OUTPUT = "output"
    EXPORT_RESPONSE = "export_response"


@dataclass(frozen=True, slots=True)
class RuntimePublicationAction:
    """One member's resolved post-Cargo action, shared by identity and execution.

    File operands are roles whose bytes belong to the Cargo/toolchain identity;
    the output is transaction-local. Literal ordering and response-file content
    remain exact inputs. This is not permission to trust a detached file digest.
    """

    transform: RuntimePublicationTransform
    preserve_debug: bool
    executor: Mapping[str, object] | None
    link_command: tuple[str | RuntimePublicationInput, ...] = ()
    link_exports: tuple[str, ...] = ()
    export_renames: tuple[tuple[str, str], ...] = ()

    def __post_init__(self) -> None:
        if (
            type(self.transform) is not RuntimePublicationTransform
            or type(self.preserve_debug) is not bool
        ):
            raise ValueError("invalid runtime publication transform")
        paths = runtime_publication_executor_paths(self.transform)
        if paths:
            executor = _validated_tree_summary(
                self.executor, label="publication executor"
            )
            if (
                tuple(cast(Sequence[str], executor["roots"])) != paths
                or executor["missing"]
                or executor["file_count"] != len(paths)
            ):
                raise ValueError("runtime publication executor closure is incomplete")
            object.__setattr__(self, "executor", _freeze_json(executor))
        elif self.executor is not None:
            raise ValueError("byte-preserving publication has no executor transform")
        if type(self.link_command) is not tuple or any(
            not (
                type(token) is RuntimePublicationInput
                or type(token) is str
                and token
                and "\0" not in token
            )
            for token in self.link_command
        ):
            raise ValueError("invalid runtime publication command")
        if type(self.link_exports) is not tuple or any(
            type(token) is not str or not token or "\0" in token
            for token in self.link_exports
        ):
            raise ValueError("invalid runtime publication export arguments")
        if (
            type(self.export_renames) is not tuple
            or any(
                type(pair) is not tuple
                or len(pair) != 2
                or any(
                    type(name) is not str or not name or "\0" in name for name in pair
                )
                for pair in self.export_renames
            )
            or tuple(sorted(self.export_renames)) != self.export_renames
            or len({pair[0] for pair in self.export_renames})
            != len(self.export_renames)
        ):
            raise ValueError("invalid runtime publication export renames")
        roles = [
            token
            for token in self.link_command
            if type(token) is RuntimePublicationInput
        ]
        if self.transform is RuntimePublicationTransform.RELOC:
            expected = set(RuntimePublicationInput) - (
                {RuntimePublicationInput.EXPORT_RESPONSE}
                if not self.link_exports
                else set()
            )
            if (
                set(roles) != expected
                or len(roles) != len(expected)
                or not self.preserve_debug
                or self.export_renames
            ):
                raise ValueError("incomplete runtime reloc publication action")
        elif (
            self.link_command
            or self.link_exports
            or (
                self.transform is not RuntimePublicationTransform.SHARED
                and self.export_renames
            )
        ):
            raise ValueError("unexpected runtime publication command or renames")

    def to_dict(self) -> dict[str, object]:
        return {
            "transform": self.transform.value,
            "preserve_debug": self.preserve_debug,
            "executor": _thaw_json(self.executor),
            "link_command": [
                {"input": token.value}
                if type(token) is RuntimePublicationInput
                else {"literal": token}
                for token in self.link_command
            ],
            "link_exports": list(self.link_exports),
            "export_renames": [list(pair) for pair in self.export_renames],
        }

    @classmethod
    def from_dict(cls, value: object) -> RuntimePublicationAction:
        payload = string_keyed_mapping(value)
        if payload is None or set(payload) != {
            "transform",
            "preserve_debug",
            "executor",
            "link_command",
            "link_exports",
            "export_renames",
        }:
            raise ValueError("runtime publication action shape is invalid")
        command = payload["link_command"]
        exports, renames = payload["link_exports"], payload["export_renames"]
        transform, preserve_debug = payload["transform"], payload["preserve_debug"]
        if (
            not isinstance(command, (list, tuple))
            or not isinstance(exports, (list, tuple))
            or not isinstance(renames, (list, tuple))
            or type(transform) is not str
            or type(preserve_debug) is not bool
        ):
            raise ValueError("runtime publication action values are invalid")
        tokens: list[str | RuntimePublicationInput] = []
        for raw in command:
            token = string_keyed_mapping(raw)
            if (
                token is not None
                and set(token) == {"input"}
                and type(token["input"]) is str
            ):
                tokens.append(RuntimePublicationInput(token["input"]))
            elif (
                token is not None
                and set(token) == {"literal"}
                and type(token["literal"]) is str
            ):
                tokens.append(token["literal"])
            else:
                raise ValueError("runtime publication argument is invalid")
        pairs: list[tuple[str, str]] = []
        for pair in renames:
            if (
                not isinstance(pair, (list, tuple))
                or len(pair) != 2
                or any(type(name) is not str for name in pair)
            ):
                raise ValueError("runtime publication rename is invalid")
            pairs.append((cast(str, pair[0]), cast(str, pair[1])))
        if any(type(item) is not str for item in exports):
            raise ValueError("runtime publication exports are invalid")
        return cls(
            RuntimePublicationTransform(transform),
            preserve_debug,
            cast(Mapping[str, object] | None, payload["executor"]),
            tuple(tokens),
            tuple(cast(Sequence[str], exports)),
            tuple(pairs),
        )

    def render_link_command(
        self, inputs: Mapping[RuntimePublicationInput, str]
    ) -> tuple[str, ...]:
        roles = {
            token
            for token in self.link_command
            if type(token) is RuntimePublicationInput
        }
        if set(inputs) != roles or any(
            type(value) is not str or not value or "\0" in value
            for value in inputs.values()
        ):
            raise ValueError(
                "runtime publication operands differ from its resolved roles"
            )
        return tuple(
            inputs[token] if type(token) is RuntimePublicationInput else token
            for token in self.link_command
        )


def runtime_publication_action(
    identity: RuntimeBuildIdentity,
) -> RuntimePublicationAction:
    value = RuntimeBuildIdentity.from_dict(identity)
    family = cast(Mapping[str, object], value.payload["family"])
    members = cast(Mapping[str, object], family["members"])
    member = cast(
        Mapping[str, object], members[cast(str, value.payload["member_kind"])]
    )
    return RuntimePublicationAction.from_dict(member["publication"])
