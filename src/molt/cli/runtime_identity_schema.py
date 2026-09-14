"""Pure, exact runtime compilation/family/member receipt authority."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from types import MappingProxyType
from typing import Iterator, Mapping, Sequence, cast

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.runtime_artifact_selection import (
    RuntimeArtifactSelection,
    RuntimeCrateType,
)
from molt.exact_json import canonical_json_bytes, canonical_json_sha256, read_exact
from molt.python_identity_common import PythonEnvironmentIdentityError, _valid_sha256
from molt.python_runtime_identity import validate_python_runtime_identity

_SCHEMA = "molt.runtime-build-member-identity.v3"
RUNTIME_ARTIFACT_METADATA_MAX_BYTES = 16 * 1024 * 1024
_FAMILY_SCHEMA = "molt.runtime-build-family.v3"
_TOOLCHAIN_MANIFEST_SCHEMA = "molt.runtime-toolchain-content.v3"
_BUILD_PYTHON_SCHEMA = "molt.runtime-build-python.v1"
_RUNTIME_BUILD_OUTER_FIELDS = {
    "schema",
    "digest",
    "compile_digest",
    "family_digest",
    "payload",
}
_RUNTIME_BUILD_FAMILY_FIELDS = {
    "schema",
    "compile_digest",
    "compile",
    "publication_authority",
    "members",
}
_RUNTIME_BUILD_COMPILE_FIELDS = {"sources", "toolchain", "common_config"}
_RUNTIME_BUILD_MEMBER_FIELDS = {
    "kind",
    "resolved_rustflags",
    "link_args",
    "publication_transform",
    "preserve_debug",
}


def _freeze_json(value: object) -> object:
    if isinstance(value, Mapping):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("runtime identity JSON object keys must be strings")
        typed = cast(Mapping[str, object], value)
        return MappingProxyType(
            {key: _freeze_json(item) for key, item in typed.items()}
        )
    if isinstance(value, (list, tuple)):
        return tuple(_freeze_json(item) for item in value)
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("runtime identity contains non-finite JSON number")
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise TypeError(f"runtime identity contains non-JSON value: {type(value).__name__}")


def _thaw_json(value: object) -> object:
    if isinstance(value, Mapping):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("runtime identity JSON object keys must be strings")
        typed = cast(Mapping[str, object], value)
        return {key: _thaw_json(item) for key, item in typed.items()}
    if isinstance(value, (list, tuple)):
        return [_thaw_json(item) for item in value]
    return value


def _canonical_json(value: object) -> str:
    return canonical_json_bytes(_thaw_json(value)).decode("utf-8")


def _digest(value: object) -> str:
    return canonical_json_sha256(_thaw_json(value))


def _json_object_mapping(value: object) -> Mapping[str, object] | None:
    """Narrow one validated JSON object without coercing or aliasing keys."""

    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        return None
    return cast(Mapping[str, object], value)


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
    value = _json_object_mapping(payload)
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
    for field in ("cargo_profile", "target_triple", "producer_artifact_selection"):
        if not isinstance(value[field], str) or not value[field]:
            raise ValueError(f"runtime common configuration {field} is invalid")
    features = _string_sequence(value["runtime_features"], label="features")
    if features != tuple(sorted(set(features))):
        raise ValueError("runtime features must be sorted and unique")
    command = _string_sequence(value["cargo_command"], label="Cargo command")
    if not command or command[0] != "cargo":
        raise ValueError("runtime Cargo command must use its content-attested role")
    if "base_rustflags" in value:
        _string_sequence(value["base_rustflags"], label="base Rust flags")
    environment = _json_object_mapping(value["environment"])
    if environment is None or any(
        not key or not _valid_sha256(item) for key, item in environment.items()
    ):
        raise ValueError("runtime build environment identity is invalid")
    if "ambient_c_build_environment" in value:
        ambient = _json_object_mapping(value["ambient_c_build_environment"])
        if ambient is None:
            raise ValueError("runtime C build environment is invalid")
        for key, item in ambient.items():
            if not key:
                raise ValueError("runtime C environment key is empty")
            _string_sequence(item, label=f"C environment {key}")
    scripts = _json_object_mapping(value["build_script_environment"])
    if scripts is None or not scripts:
        raise ValueError("runtime build-script environment is invalid")
    return value


def _validated_build_script_environment(
    payload: object, *, target: str, build_python: object
) -> None:
    value = _json_object_mapping(payload)
    if value is None:
        raise ValueError("runtime build-script environment must be an object")
    schema = value.get("schema")
    fields = {"schema", "build_python", "python_import_policy"}
    runtime = schema == "molt.runtime-build-script-environment.v2"
    if runtime:
        fields |= {
            "MOLT_WASM_CPYTHON_ABI_EXPORTS",
            "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS",
            "MOLT_WASM_LONGDOUBLE_ARCHIVE",
            "MOLT_WASM_BUILTINS_ARCHIVE",
        }
    elif schema != "molt.cpython-abi-build-script-environment.v2":
        raise ValueError("runtime build-script schema is invalid")
    if set(value) != fields:
        raise ValueError("runtime build-script environment shape is invalid")
    python = _json_object_mapping(value["build_python"])
    if python is None or set(python) != {"selected_by", "selectors", "content_digest"}:
        raise ValueError("runtime build-script Python selection is invalid")
    selectors = _json_object_mapping(python["selectors"])
    selected = python["selected_by"]
    if selectors is None or set(selectors) != {"MOLT_BUILD_PYTHON", "PYTHON"}:
        raise ValueError("runtime build-script Python selectors are invalid")
    if not isinstance(selected, str) or selected not in {
        *selectors,
        "platform-default",
    }:
        raise ValueError("runtime build-script Python selector is unknown")
    if any(
        not isinstance(state, str) or state not in {"selected", "shadowed", "unset"}
        for state in selectors.values()
    ):
        raise ValueError("runtime build-script Python selector state is invalid")
    if tuple(name for name, state in selectors.items() if state == "selected") != (
        () if selected == "platform-default" else (selected,)
    ):
        raise ValueError("runtime build-script selected interpreter is inconsistent")
    if python["content_digest"] != _digest(build_python):
        raise ValueError("runtime build-script Python differs from toolchain custody")
    if value["python_import_policy"] != "isolated-no-site-v1":
        raise ValueError("runtime build-script Python import policy is invalid")
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
    for name in ("MOLT_WASM_LONGDOUBLE_ARCHIVE", "MOLT_WASM_BUILTINS_ARCHIVE"):
        archive = _json_object_mapping(value[name])
        if archive is None:
            raise ValueError("runtime build-script archive state is invalid")
        state = archive.get("state")
        if not wasm:
            if archive != {"state": "ignored-for-target"}:
                raise ValueError("native runtime has WASM archive inputs")
        elif state in ("unset", "empty", "fallback"):
            if set(archive) != {"state"}:
                raise ValueError("runtime absent archive has content")
        elif state == "resolved":
            content = _json_object_mapping(archive.get("content"))
            if (
                set(archive) != {"state", "content"}
                or content is None
                or set(content) != {"logical_name", "sha256", "size"}
                or content["logical_name"] != name.lower()
                or not _valid_sha256(content["sha256"])
                or type(content["size"]) is not int
                or content["size"] < 0
            ):
                raise ValueError("runtime build-script archive content is invalid")
        else:
            raise ValueError("runtime build-script archive selector is invalid")


def _validated_publication_authority(payload: object) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
    if value is None or not isinstance(value.get("schema"), str) or not value["schema"]:
        raise ValueError("runtime publication authority schema is invalid")
    if "files" in value:
        if set(value) != {"schema", "digest", "files"} or not _valid_sha256(
            value["digest"]
        ):
            raise ValueError("runtime publication file authority shape is invalid")
        files = value["files"]
        if not isinstance(files, (list, tuple)):
            raise ValueError("runtime publication files must be an array")
        paths = []
        for raw in files:
            item = _json_object_mapping(raw)
            if item is None or set(item) != {"path", "size", "sha256"}:
                raise ValueError("runtime publication file record is invalid")
            path, size = item["path"], item["size"]
            if (
                not isinstance(path, str)
                or not path
                or PurePosixPath(path).is_absolute()
                or PureWindowsPath(path).drive
                or "\\" in path
                or ".." in PurePosixPath(path).parts
                or type(size) is not int
                or size < 0
                or not _valid_sha256(item["sha256"])
            ):
                raise ValueError("runtime publication file content is invalid")
            paths.append(path)
        if paths != sorted(set(paths)):
            raise ValueError("runtime publication files must be sorted and unique")
    else:
        if set(value) != {
            "schema",
            "digest",
            "file_count",
            "missing",
            "roots",
            "total_size",
        }:
            raise ValueError("runtime publication tree authority shape is invalid")
        _validated_tree_summary(
            {key: item for key, item in value.items() if key != "schema"},
            label="publication",
        )
    return value


def _validated_tree_summary(payload: object, *, label: str) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
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
    value = _json_object_mapping(payload)
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


def _validated_build_python_identity(payload: object) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
    if value is None or set(value) != {
        "identity_sha256",
        "logical_name",
        "runtime",
        "schema",
        "selected_executable",
    }:
        raise ValueError("runtime build Python identity shape is invalid")
    selected = _json_object_mapping(value.get("selected_executable"))
    if selected is None or set(selected) != {
        "content_filename",
        "entrypoint",
        "sha256",
        "size",
    }:
        raise ValueError("runtime build Python executable identity shape is invalid")
    entrypoint = selected.get("entrypoint")
    content_filename = selected.get("content_filename")
    size = selected.get("size")
    if (
        value.get("schema") != _BUILD_PYTHON_SCHEMA
        or value.get("logical_name") != "build_python"
        or not isinstance(entrypoint, str)
        or not entrypoint
        or not _portable_filename(entrypoint)
        or not isinstance(content_filename, str)
        or not content_filename
        or not _portable_filename(content_filename)
        or not isinstance(size, int)
        or isinstance(size, bool)
        or size < 0
        or not _valid_sha256(selected.get("sha256"))
    ):
        raise ValueError("runtime build Python executable identity is invalid")
    runtime = _thaw_json(value.get("runtime"))
    try:
        validate_python_runtime_identity(runtime)
    except PythonEnvironmentIdentityError as exc:
        raise ValueError(f"runtime build Python closure is invalid: {exc}") from exc
    material = {key: value[key] for key in value if key != "identity_sha256"}
    if value.get("identity_sha256") != _digest(material):
        raise ValueError("runtime build Python identity digest is invalid")
    return value


def _validated_runtime_toolchain_content(
    payload: object,
    *,
    target_triple: str,
) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
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
    tools = _json_object_mapping(value.get("tools"))
    allowed_tools = {
        "ar",
        "build_python",
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
        or not {"build_python", "cargo", "rustc"}.issubset(tools)
        or not set(tools).issubset(allowed_tools)
    ):
        raise ValueError("runtime toolchain tools are invalid")
    for name, raw in tools.items():
        if name == "build_python":
            _validated_build_python_identity(raw)
        else:
            _validated_executable_content_identity(raw, logical_name=name)
    wrappers = _json_object_mapping(value.get("wrappers"))
    if wrappers is None:
        raise ValueError("runtime toolchain wrapper identities are invalid")
    for name, raw in wrappers.items():
        if name not in {"RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}:
            raise ValueError("runtime toolchain wrapper name is invalid")
        _validated_executable_content_identity(raw, logical_name=name.casefold())
    _validated_tree_summary(
        value.get("cargo_configuration"), label="Cargo configuration"
    )
    resources = _json_object_mapping(value.get("rust_resources"))
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

    sysroots = _json_object_mapping(value.get("sysroots"))
    expected_sysroots = {"wasi"} if target_triple.startswith("wasm32-") else set()
    if sysroots is None or set(sysroots) != expected_sysroots:
        raise ValueError("runtime sysroot identity is invalid")
    for name, raw in sysroots.items():
        _validated_tree_summary(raw, label=f"{name} sysroot")
    archives = value.get("archives")
    if not isinstance(archives, (list, tuple)):
        raise ValueError("runtime archive identities are invalid")
    archive_names: set[str] = set()
    for raw in archives:
        archive = _json_object_mapping(raw)
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
    return value


def _validated_toolchain_manifest_payload(
    payload: object,
) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
    if value is None or set(value) != {"target_triple", "toolchain"}:
        raise ValueError("runtime toolchain manifest content shape is invalid")
    target = value.get("target_triple")
    toolchain = _json_object_mapping(value.get("toolchain"))
    if not isinstance(target, str) or not target or toolchain is None or not toolchain:
        raise ValueError("runtime toolchain manifest content is invalid")
    _validated_runtime_toolchain_content(toolchain, target_triple=target)
    return value


def _runtime_toolchain_build_python(
    manifest: RuntimeToolchainContentManifest,
) -> Mapping[str, object]:
    toolchain = cast(Mapping[str, object], manifest.payload["toolchain"])
    tools = cast(Mapping[str, object], toolchain["tools"])
    return _validated_build_python_identity(tools["build_python"])


def _validated_runtime_build_payload(
    payload: object,
    *,
    digest: object,
    compile_digest: object,
    family_digest: object,
) -> Mapping[str, object]:
    value = _json_object_mapping(payload)
    if value is None or set(value) != {"family", "member_kind"}:
        raise ValueError("runtime build identity payload shape is invalid")
    family = _json_object_mapping(value.get("family"))
    member_kind = value.get("member_kind")
    if (
        family is None
        or set(family) != _RUNTIME_BUILD_FAMILY_FIELDS
        or family.get("schema") != _FAMILY_SCHEMA
        or not isinstance(member_kind, str)
        or not member_kind
    ):
        raise ValueError("runtime build identity family shape is invalid")
    compile_payload = _json_object_mapping(family.get("compile"))
    publication = _json_object_mapping(family.get("publication_authority"))
    members = _json_object_mapping(family.get("members"))
    if (
        compile_payload is None
        or set(compile_payload) != _RUNTIME_BUILD_COMPILE_FIELDS
        or _json_object_mapping(compile_payload.get("sources")) is None
        or _json_object_mapping(compile_payload.get("toolchain")) is None
        or _json_object_mapping(compile_payload.get("common_config")) is None
        or publication is None
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
    toolchain = cast(Mapping[str, object], compile_payload["toolchain"])
    _validated_build_script_environment(
        config["build_script_environment"],
        target=target,
        build_python=cast(Mapping[str, object], toolchain["tools"])["build_python"],
    )
    _validated_publication_authority(publication)
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
            "rust-compiler-builtins",
            "wasi-long-double",
            "clang-rt-builtins",
        ):
            raise ValueError("runtime WASM family archive closure is incomplete")
    for kind, raw_member in members.items():
        member = _json_object_mapping(raw_member)
        if (
            not kind
            or member is None
            or set(member) != _RUNTIME_BUILD_MEMBER_FIELDS
            or member.get("kind") != kind
            or not isinstance(member.get("publication_transform"), str)
            or not member.get("publication_transform")
            or type(member.get("preserve_debug")) is not bool
        ):
            raise ValueError("runtime build identity member shape is invalid")
        for field in ("resolved_rustflags", "link_args"):
            items = member.get(field)
            if not isinstance(items, (list, tuple)) or not all(
                isinstance(item, str) and item for item in items
            ):
                raise ValueError("runtime build identity member shape is invalid")
    member = _json_object_mapping(members.get(member_kind))
    if member is None:
        raise ValueError("runtime build identity selected member is absent")
    if not all(_valid_sha256(item) for item in (digest, compile_digest, family_digest)):
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
        if not _valid_sha256(self.digest) or self.digest != _digest(frozen):
            raise ValueError("runtime toolchain manifest digest is invalid")
        object.__setattr__(self, "payload", frozen)

    def __iter__(self) -> Iterator[str]:
        return iter(("schema", "digest", "payload"))

    def __len__(self) -> int:
        return 3

    def __getitem__(self, key: str) -> object:
        return self.to_dict()[key]

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _TOOLCHAIN_MANIFEST_SCHEMA,
            "digest": self.digest,
            "payload": _thaw_json(self.payload),
        }

    @classmethod
    def from_dict(cls, value: object) -> RuntimeToolchainContentManifest:
        outer = _json_object_mapping(value)
        if (
            outer is None
            or set(outer) != {"schema", "digest", "payload"}
            or outer.get("schema") != _TOOLCHAIN_MANIFEST_SCHEMA
        ):
            raise ValueError("runtime toolchain manifest schema is invalid")
        digest = outer.get("digest")
        payload = _json_object_mapping(outer.get("payload"))
        if not _valid_sha256(digest) or payload is None or digest != _digest(payload):
            raise ValueError("runtime toolchain manifest digest is invalid")
        assert isinstance(digest, str)
        return cls(digest=digest, payload=payload)

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
        return self.to_dict()[key]

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "digest": self.digest,
            "compile_digest": self.compile_digest,
            "family_digest": self.family_digest,
            "payload": _thaw_json(self.payload),
        }

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
        return RuntimeToolchainContentManifest(_digest(payload), payload)

    @classmethod
    def from_dict(cls, value: object) -> RuntimeBuildIdentity:
        outer = _json_object_mapping(value)
        if (
            outer is None
            or set(outer) != _RUNTIME_BUILD_OUTER_FIELDS
            or outer.get("schema") != _SCHEMA
        ):
            raise ValueError("runtime build identity schema is invalid")
        payload = _json_object_mapping(outer.get("payload"))
        digest = outer.get("digest")
        compile_digest = outer.get("compile_digest")
        family_digest = outer.get("family_digest")
        if (
            payload is None
            or not isinstance(digest, str)
            or not isinstance(compile_digest, str)
            or not isinstance(family_digest, str)
        ):
            raise ValueError("runtime build identity is incomplete")
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

    build_identity = RuntimeBuildIdentity.from_dict(build_identity.to_dict())
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

    build_identity = RuntimeBuildIdentity.from_dict(
        value.to_dict() if isinstance(value, RuntimeBuildIdentity) else value
    )
    family = cast(Mapping[str, object], build_identity.payload["family"])
    compile_payload = cast(Mapping[str, object], family["compile"])
    common_config = _json_object_mapping(compile_payload.get("common_config"))
    members = _json_object_mapping(family.get("members"))
    member = (
        _json_object_mapping(members.get("staticlib")) if members is not None else None
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
        or member.get("publication_transform")
        != "native-staticlib-and-link-manifest-v1"
    ):
        raise ValueError(
            "runtime build identity does not match the requested native staticlib"
        )
    return build_identity


@dataclass(frozen=True)
class RuntimeBuildMemberPlan:
    kind: str
    resolved_rustflags: str | tuple[str, ...]
    link_args: tuple[str, ...]
    publication_transform: str
    preserve_debug: bool
