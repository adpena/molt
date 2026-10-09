"""Content identity for one manifest-provisioned wasi-sdk installation.

An installation is an identity-addressed prefix owned by
``tools/provision_wasi_sdk.py``::

    <prefix>/.molt-wasi-sdk.json   provision receipt (host asset + tree identity)
    <prefix>/sdk/                  the extracted wasi-sdk archive root

This module is stdlib-only so CI can verify an installation before the optional
Python dependency set exists.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path, PurePosixPath
import re
from typing import Any, Mapping, Sequence, TypeGuard

from molt.rust_toolchain import rust_flag_spans
from molt.exact_json import canonical_json_bytes, dumps_exact, read_exact
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import (
    open_stable_regular_file,
    stable_regular_file_identity,
    stable_regular_file_handle_identity,
)


INSTALL_RECEIPT_FILENAME = ".molt-wasi-sdk.json"
INSTALL_RECEIPT_SCHEMA = "molt.wasi-sdk-install.v2"
SDK_RESOURCE_ROOTS = ("lib", "share/wasi-sysroot")
TREE_IDENTITY_SCHEMA = "molt.wasi-sdk-tree.v1"
WASI_C_ABI_PLAN_ENV = "MOLT_WASI_C_ABI_PLAN"
# Installed data is the single cross-language wire declaration. The Rust
# build helper includes these same bytes; neither owns a parallel role table.
_C_ABI_PROTOCOL = dict(
    line.split("=", 1)
    for line in Path(__file__)
    .with_name("wasi_c_abi_protocol.txt")
    .read_text(encoding="ascii")
    .splitlines()
)
WASI_C_ABI_PLAN_SCHEMA = _C_ABI_PROTOCOL["schema"]
WASI_C_ABI_TARGET = _C_ABI_PROTOCOL["target"]
WASI_C_ABI_VARIANT = _C_ABI_PROTOCOL["variant"]
WASI_C_ABI_HEADER = tuple(_C_ABI_PROTOCOL["header"].split(","))
WASI_C_ABI_ROLES = tuple(_C_ABI_PROTOCOL["members"].split(","))
WASI_C_ABI_PLAN_MAX_CHARS = int(_C_ABI_PROTOCOL["max_chars"])
WASI_C_ABI_MAX_MEMBER_BYTES = int(_C_ABI_PROTOCOL["max_member_bytes"])
WASI_C_ABI_MAX_VERSION_CHARS = int(_C_ABI_PROTOCOL["max_version_chars"])
WASI_C_ABI_MAX_PATH_CHARS = int(_C_ABI_PROTOCOL["max_path_chars"])
SDK_DIRNAME = "sdk"
SDK_CARGO_TOOLS = (
    ("CC", "clang"),
    ("CXX", "clang++"),
    ("AR", "llvm-ar"),
    ("RANLIB", "llvm-ranlib"),
)
SDK_BUILD_TOOL_NAMES = (*(name for _, name in SDK_CARGO_TOOLS), "wasm-ld")
SDK_TOOL_NAMES = (*SDK_BUILD_TOOL_NAMES, "llvm-nm")
SDK_OPTIONAL_TOOL_NAMES = ("llvm-strip",)
SDK_CARGO_TARGETS = ("wasm32-wasip1", "wasm32-unknown-unknown")
# Admission bounds, not measured sizes: the largest pinned archive (Windows)
# materializes its tool aliases as copies, so the byte bound leaves headroom.
MAX_TREE_ENTRIES = 100_000
MAX_TREE_BYTES = 8 * 1024 * 1024 * 1024
ASSET_RECORD_KEYS = frozenset(
    {
        "id",
        "sdk_version",
        "llvm_version",
        "url",
        "size",
        "sha256",
        "archive_root",
        "provenance_url",
        "record_sha256",
    }
)
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_HOST_ID_RE = re.compile(r"(?:linux|macos|windows)-(?:x86_64|aarch64)")
_SDK_VERSION_RE = re.compile(r"[0-9]+\.[0-9]+(?:\+[A-Za-z0-9][A-Za-z0-9._-]*)?")
_LLVM_VERSION_RE = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


class WasiSdkIdentityError(ValueError):
    """Raised when an SDK tree or its provision receipt is not exact."""


@dataclass(frozen=True, slots=True)
class WasiCAbiProjection:
    """Finite build-script transport, not a substitute for SDK content custody.

    Header fields, followed by five (role, absolute path, byte size, SHA256)
    records, are NUL-separated UTF-8 encoded as lowercase hex. This keeps the
    environment value single-line on every host and needs no runtime or build
    dependency in Rust. Size admission precedes encoding/decoding allocations.
    """

    sdk_version: str
    llvm_version: str
    tree_sha256: str
    sdk: Path
    sysroot: Path
    include: Path
    driver: Path
    linker: Path
    files: tuple[tuple[str, Path, int, str], ...]

    @classmethod
    def from_facts(
        cls,
        sdk: Path,
        *,
        sdk_version: str,
        llvm_version: str,
        tree_sha256: str,
        facts: Mapping[str, Any],
    ) -> WasiCAbiProjection:
        """Project the admitted receipt's finite facts, without filesystem reads."""
        sysroot = sdk / "share/wasi-sysroot"
        return cls(
            sdk_version,
            llvm_version,
            tree_sha256,
            sdk,
            sysroot,
            sysroot / "include/wasm32-wasip1",
            sdk / facts["tools"]["clang"]["path"],
            sdk / facts["tools"]["wasm-ld"]["path"],
            tuple(
                (
                    role,
                    sdk / facts["members"][role]["path"],
                    facts["members"][role]["size"],
                    facts["members"][role]["sha256"],
                )
                for role in WASI_C_ABI_ROLES
            ),
        )

    def path(self, role: str) -> Path:
        for name, path, _size, _digest in self.files:
            if name == role:
                return path
        raise WasiSdkIdentityError(f"unknown WASI C-runtime role: {role}")

    def native_search_directories(self) -> tuple[Path, ...]:
        """Ordered SDK search context, projected without reading managed files."""
        return tuple(
            dict.fromkeys(path.parent for _role, path, _size, _digest in self.files)
        )

    def rustflags(
        self, flags: Sequence[str], *, include_search: bool = True
    ) -> tuple[str, ...]:
        """Keep the selected C runtime available before any repository build script.

        Cargo dependency flags own the search context. Final-crate arguments
        validate the same link mode without adding a second directory prefix.
        Other user search paths retain their order after the selected SDK.
        """
        normalized = _wasi_external_libc_mode_flags(flags)
        if not include_search:
            return normalized
        directories = self.native_search_directories()
        retained: list[str] = []
        for span in rust_flag_spans(normalized):
            original = normalized[span.start : span.stop]
            if normalized[span.start] == "--":
                retained.extend(original)
                break
            if span.option == "-L":
                value = span.value
                if not value:
                    raise WasiSdkIdentityError("-L requires a Rust search path")
            else:
                retained.extend(original)
                continue
            kind, separator, path = value.partition("=")
            if separator and kind == "native" and Path(path) in directories:
                retained.extend(span.leading)
                continue
            retained.extend(original)
        return (
            *tuple(
                part for path in directories for part in ("-L", "native=" + str(path))
            ),
            *retained,
        )

    def encode(self) -> str:
        if type(self.files) is not tuple or len(self.files) != len(WASI_C_ABI_ROLES):
            raise WasiSdkIdentityError(
                "WASI C-runtime projection requires exactly five members"
            )
        for role, row in zip(WASI_C_ABI_ROLES, self.files, strict=True):
            if (
                type(row) is not tuple
                or len(row) != 4
                or type(row[0]) is not str
                or row[0] != role
                or not isinstance(row[1], Path)
                or type(row[2]) is not int
                or not 0 <= row[2] <= WASI_C_ABI_MAX_MEMBER_BYTES
                or not _is_sha256(row[3])
            ):
                raise WasiSdkIdentityError("invalid WASI C-runtime projection member")
        if (
            type(self.sdk_version) is not str
            or len(self.sdk_version) > WASI_C_ABI_MAX_VERSION_CHARS
            or not is_wasi_sdk_version(self.sdk_version)
            or type(self.llvm_version) is not str
            or len(self.llvm_version) > WASI_C_ABI_MAX_VERSION_CHARS
            or not is_wasi_sdk_llvm_version(self.llvm_version)
            or not _is_sha256(self.tree_sha256)
        ):
            raise WasiSdkIdentityError("invalid WASI C-runtime projection producer")
        for path in (
            self.sdk,
            self.sysroot,
            self.include,
            self.driver,
            self.linker,
            *(row[1] for row in self.files),
        ):
            if not isinstance(path, Path) or len(str(path)) > WASI_C_ABI_MAX_PATH_CHARS:
                raise WasiSdkIdentityError("invalid or oversized WASI C-runtime path")
        fields = [
            WASI_C_ABI_PLAN_SCHEMA,
            WASI_C_ABI_TARGET,
            WASI_C_ABI_VARIANT,
            *(str(getattr(self, name)) for name in WASI_C_ABI_HEADER),
        ]
        for role, path, size, digest in self.files:
            fields.extend((role, str(path), str(size), digest))
        if any("\0" in value for value in fields):
            raise WasiSdkIdentityError("WASI C-runtime projection contains NUL")
        # Count exact UTF-8 bytes without creating an oversized encoded copy.
        extent = len(fields) - 1
        for value in fields:
            for character in value:
                codepoint = ord(character)
                if 0xD800 <= codepoint <= 0xDFFF:
                    raise WasiSdkIdentityError("WASI C-runtime projection is not UTF-8")
                extent += (
                    1
                    if codepoint < 0x80
                    else 2
                    if codepoint < 0x800
                    else 3
                    if codepoint < 0x10000
                    else 4
                )
                if extent * 2 > WASI_C_ABI_PLAN_MAX_CHARS:
                    raise WasiSdkIdentityError(
                        "WASI C-runtime projection exceeds its transport bound"
                    )
        encoded = "\0".join(fields).encode("utf-8").hex()
        if WasiCAbiProjection.decode(encoded) != self:
            raise WasiSdkIdentityError("WASI C-runtime projection is not canonical")
        return encoded

    @classmethod
    def decode(cls, value: str) -> WasiCAbiProjection:
        if (
            type(value) is not str
            or not value
            or len(value) > WASI_C_ABI_PLAN_MAX_CHARS
            or len(value) % 2
            or re.fullmatch(r"[0-9a-f]+", value) is None
        ):
            raise WasiSdkIdentityError("invalid WASI C-runtime projection encoding")
        try:
            fields = bytes.fromhex(value).decode("utf-8", errors="strict").split("\0")
        except UnicodeError as exc:
            raise WasiSdkIdentityError(
                "WASI C-runtime projection is not UTF-8"
            ) from exc
        member_start = 3 + len(WASI_C_ABI_HEADER)
        if len(fields) != member_start + 4 * len(WASI_C_ABI_ROLES) or fields[:3] != [
            WASI_C_ABI_PLAN_SCHEMA,
            WASI_C_ABI_TARGET,
            WASI_C_ABI_VARIANT,
        ]:
            raise WasiSdkIdentityError(
                "unsupported WASI C-runtime projection schema/target/variant"
            )
        header = dict(zip(WASI_C_ABI_HEADER, fields[3:member_start], strict=True))
        if (
            len(header["sdk_version"]) > WASI_C_ABI_MAX_VERSION_CHARS
            or len(header["llvm_version"]) > WASI_C_ABI_MAX_VERSION_CHARS
            or not is_wasi_sdk_version(header["sdk_version"])
            or not is_wasi_sdk_llvm_version(header["llvm_version"])
            or not _is_sha256(header["tree_sha256"])
        ):
            raise WasiSdkIdentityError("invalid WASI C-runtime producer identity")

        def path_value(raw: str) -> Path:
            if len(raw) > WASI_C_ABI_MAX_PATH_CHARS:
                raise WasiSdkIdentityError("WASI C-runtime path exceeds bound")
            path = Path(raw)
            if (
                not path.is_absolute()
                or ".." in path.parts
                or str(path) != raw
                or (os.name != "nt" and raw.startswith("//"))
            ):
                raise WasiSdkIdentityError(
                    "WASI C-runtime path must be canonical absolute syntax"
                )
            return path

        sdk, sysroot, include, driver, linker = (
            path_value(header[name])
            for name in ("sdk", "sysroot", "include", "driver", "linker")
        )
        if (
            not sysroot.is_relative_to(sdk)
            or not include.is_relative_to(sysroot)
            or not driver.is_relative_to(sdk)
            or not linker.is_relative_to(sdk)
        ):
            raise WasiSdkIdentityError("WASI C-runtime roots escape their SDK")
        files: list[tuple[str, Path, int, str]] = []
        for index, role in enumerate(WASI_C_ABI_ROLES):
            start = member_start + 4 * index
            name, raw_path, size, digest = fields[start : start + 4]
            path = path_value(raw_path)
            if (
                name != role
                or not path.is_relative_to(sdk)
                or not _is_sha256(digest)
                or re.fullmatch(r"0|[1-9][0-9]{0,19}", size) is None
                or int(size) > WASI_C_ABI_MAX_MEMBER_BYTES
            ):
                raise WasiSdkIdentityError(f"invalid WASI C-runtime member: {role}")
            files.append((role, path, int(size), digest))
        return cls(
            header["sdk_version"],
            header["llvm_version"],
            header["tree_sha256"],
            sdk,
            sysroot,
            include,
            driver,
            linker,
            tuple(files),
        )

    def content_identity(self) -> dict[str, object]:
        """Relocatable receipt projection; physical paths stay in custody."""
        return {
            "schema": "molt.wasi-c-abi-identity.v1",
            "target": WASI_C_ABI_TARGET,
            "variant": WASI_C_ABI_VARIANT,
            "sdk_version": self.sdk_version,
            "llvm_version": self.llvm_version,
            "tree_sha256": self.tree_sha256,
            "members": {
                role: {"size": size, "sha256": digest}
                for role, _path, size, digest in self.files
            },
        }


def validate_wasi_c_abi_identity(value: object) -> None:
    """Validate exactly the content projection produced above."""
    if not isinstance(value, Mapping) or set(value) != {
        "schema",
        "target",
        "variant",
        "sdk_version",
        "llvm_version",
        "tree_sha256",
        "members",
    }:
        raise WasiSdkIdentityError("invalid WASI C-runtime content identity")
    if (
        value["schema"] != "molt.wasi-c-abi-identity.v1"
        or value["target"] != WASI_C_ABI_TARGET
        or value["variant"] != WASI_C_ABI_VARIANT
        or type(value["sdk_version"]) is not str
        or len(value["sdk_version"]) > WASI_C_ABI_MAX_VERSION_CHARS
        or type(value["llvm_version"]) is not str
        or len(value["llvm_version"]) > WASI_C_ABI_MAX_VERSION_CHARS
        or not is_wasi_sdk_version(value["sdk_version"])
        or not is_wasi_sdk_llvm_version(value["llvm_version"])
        or not _is_sha256(value["tree_sha256"])
    ):
        raise WasiSdkIdentityError("invalid WASI C-runtime identity coordinate")
    members = value["members"]
    if not isinstance(members, Mapping) or set(members) != set(WASI_C_ABI_ROLES):
        raise WasiSdkIdentityError("invalid WASI C-runtime identity members")
    for member in members.values():
        if (
            not isinstance(member, Mapping)
            or set(member) != {"size", "sha256"}
            or type(member["size"]) is not int
            or not 0 <= member["size"] <= WASI_C_ABI_MAX_MEMBER_BYTES
            or not _is_sha256(member["sha256"])
        ):
            raise WasiSdkIdentityError("invalid WASI C-runtime identity member content")


def _wasi_external_libc_mode_flags(flags: Sequence[str]) -> tuple[str, ...]:
    """Select raw WASM linking with SDK-owned libc, without overriding conflicts."""
    required = {
        "link-self-contained": _C_ABI_PROTOCOL["link_self_contained"],
        "linker-flavor": _C_ABI_PROTOCOL["linker_flavor"],
    }
    seen: set[str] = set()
    result: list[str] = []
    trailing: Sequence[str] = ()
    try:
        spans = tuple(rust_flag_spans(flags))
    except ValueError as exc:
        raise WasiSdkIdentityError(str(exc)) from exc
    for span in spans:
        value = span.codegen
        if value is None:
            if flags[span.start] == "--":
                trailing = flags[span.start : span.stop]
                break
            result.extend(flags[span.start : span.stop])
            continue
        key, separator, operand = value.partition("=")
        if key in required:
            if not separator or operand != required[key] or key in seen:
                raise WasiSdkIdentityError(
                    f"WASI external-libc mode conflicts with {key}"
                )
            seen.add(key)
        result.extend((*span.leading, "-C", value))
    for key, value in required.items():
        if key not in seen:
            result.extend(("-C", f"{key}={value}"))
    return (*result, *trailing)


def wasi_c_abi_projection(environment: Mapping[str, str]) -> WasiCAbiProjection:
    value = environment.get(WASI_C_ABI_PLAN_ENV)
    if value is None:
        raise WasiSdkIdentityError(
            "WASI C-runtime plan is missing; provision the pinned SDK and use the "
            "project WASM toolchain environment projection before Cargo"
        )
    return WasiCAbiProjection.decode(value)


def is_wasi_sdk_version(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _SDK_VERSION_RE.fullmatch(value) is not None


def is_wasi_sdk_llvm_version(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _LLVM_VERSION_RE.fullmatch(value) is not None


def is_wasi_sdk_host_id(value: object) -> TypeGuard[str]:
    return isinstance(value, str) and _HOST_ID_RE.fullmatch(value) is not None


def _is_sha256(value: object) -> bool:
    return (
        type(value) is str
        and len(value) == 64
        and _SHA256_RE.fullmatch(value) is not None
    )


def executable_filename(name: str, host_id: str) -> str:
    """Spell one SDK executable for the host that runs it."""

    return f"{name}.exe" if host_id.startswith("windows-") else name


@dataclass(frozen=True, slots=True)
class WasiSdkVersionIdentity:
    sdk_version: str
    llvm_version: str


@dataclass(frozen=True, slots=True)
class WasiSdkTreeIdentity:
    entries: int
    total_bytes: int
    sha256: str
    records: tuple[tuple[str, str, int, str], ...]
    version: WasiSdkVersionIdentity | None = None

    def as_record(self) -> dict[str, object]:
        return {
            "schema": TREE_IDENTITY_SCHEMA,
            "entries": self.entries,
            "total_bytes": self.total_bytes,
            "sha256": self.sha256,
        }

    def subtree(self, relative: str) -> WasiSdkTreeIdentity:
        nodes = {row[0]: row for row in self.records}
        if nodes.get(relative, (None, None))[1] != "directory":
            raise WasiSdkIdentityError(f"WASI SDK resource root is absent: {relative}")
        prefix = relative + "/"
        return _tree_from_records(
            tuple(
                (path[len(prefix) :], kind, size, content)
                for path, kind, size, content in self.records
                if path.startswith(prefix)
            )
        )

    def member(self, relative: str) -> dict[str, object]:
        return _record_member({row[0]: row for row in self.records}, relative)


def _record_node(
    nodes: Mapping[str, tuple[str, str, int, str]], relative: str
) -> tuple[str, tuple[str, str, int, str]]:
    """Resolve aliases only through the tree generation already captured."""
    original = portable_relative_path(relative).as_posix()
    parts = list(PurePosixPath(original).parts)
    links = 0
    index = 0
    while index < len(parts):
        key = "/".join(parts[: index + 1])
        row = nodes.get(key)
        if row is None:
            raise WasiSdkIdentityError(f"WASI SDK member is missing: {original}")
        if row[1] == "link":
            links += 1
            target = PurePosixPath(row[3])
            if links > 64 or target.is_absolute() or "\\" in row[3]:
                raise WasiSdkIdentityError(f"WASI SDK link is invalid: {key}")
            normalized: list[str] = []
            for part in [*parts[:index], *target.parts, *parts[index + 1 :]]:
                if part == "..":
                    if not normalized:
                        raise WasiSdkIdentityError(
                            f"WASI SDK link escapes its root: {key}"
                        )
                    normalized.pop()
                elif part != ".":
                    normalized.append(part)
            parts, index = normalized, 0
            continue
        if index < len(parts) - 1 and row[1] != "directory":
            raise WasiSdkIdentityError(f"WASI SDK member crosses a file: {original}")
        index += 1
    content = "/".join(parts)
    row = nodes.get(content)
    if row is None:
        raise WasiSdkIdentityError(f"WASI SDK link dangles: {original}")
    return content, row


def _record_member(
    nodes: Mapping[str, tuple[str, str, int, str]], relative: str
) -> dict[str, object]:
    content, row = _record_node(nodes, relative)
    if row[1] != "file":
        raise WasiSdkIdentityError(f"WASI SDK member is not a regular file: {relative}")
    return {"path": relative, "content_path": content, "size": row[2], "sha256": row[3]}


def _tree_from_records(
    records: tuple[tuple[str, str, int, str], ...],
    *,
    version: WasiSdkVersionIdentity | None = None,
) -> WasiSdkTreeIdentity:
    records = tuple(sorted(records))
    digest = hashlib.sha256()
    for row in records:
        digest.update(canonical_json_bytes(list(row)))
        digest.update(b"\n")
    return WasiSdkTreeIdentity(
        len(records),
        sum(row[2] for row in records if row[1] == "file"),
        digest.hexdigest(),
        records,
        version,
    )


def wasi_c_abi_member_paths(
    sdk: Path, sysroot: Path, llvm_version: str
) -> tuple[Path, ...]:
    """The canonical finite member layout for this SDK release."""
    lib = sysroot / "lib" / "wasm32-wasip1"
    return (
        lib / "libc.a",
        lib / "libc-printscan-long-double.a",
        sdk
        / "lib"
        / "clang"
        / llvm_version.split(".")[0]
        / "lib"
        / "wasm32-unknown-wasip1"
        / "libclang_rt.builtins.a",
        lib / "crt1-command.o",
        lib / "crt1-reactor.o",
    )


def wasi_sdk_receipt_facts(
    asset: Mapping[str, Any], tree: WasiSdkTreeIdentity
) -> dict[str, object]:
    """Derive finite identities from the same authenticated tree records."""
    if tree.version is None or tree.version.sdk_version != asset["sdk_version"]:
        raise WasiSdkIdentityError(
            "WASI SDK VERSION identity differs from its manifest asset"
        )
    if tree.version.llvm_version != asset["llvm_version"]:
        raise WasiSdkIdentityError(
            "WASI SDK LLVM producer identity differs from its manifest asset"
        )
    nodes = {row[0]: row for row in tree.records}
    sdk = Path(SDK_DIRNAME)
    tree.member("share/wasi-sysroot/include/wasm32-wasip1/errno.h")
    return {
        "tools": {
            role: (
                _record_member(nodes, "bin/" + executable_filename(role, asset["id"]))
                if role in SDK_TOOL_NAMES
                or "bin/" + executable_filename(role, asset["id"]) in nodes
                else None
            )
            for role in (*SDK_TOOL_NAMES, *SDK_OPTIONAL_TOOL_NAMES)
        },
        "members": {
            role: _record_member(nodes, path.relative_to(sdk).as_posix())
            for role, path in zip(
                WASI_C_ABI_ROLES,
                wasi_c_abi_member_paths(
                    sdk, sdk / "share/wasi-sysroot", asset["llvm_version"]
                ),
                strict=True,
            )
        },
        "resources": {
            relative: tree.subtree(relative).as_record()
            for relative in SDK_RESOURCE_ROOTS
        },
    }


def _parse_wasi_sdk_version(raw: bytes, *, label: str) -> WasiSdkVersionIdentity:
    version_text = raw.decode("utf-8", errors="strict")
    version_lines = version_text.splitlines()
    sdk_version = version_lines[0].strip() if version_lines else ""
    llvm_versions = tuple(
        match.group(1)
        for line in version_lines[1:]
        if (match := re.fullmatch(r"llvm-version:\s*(\d+\.\d+\.\d+)\s*", line))
    )
    if not is_wasi_sdk_version(sdk_version) or len(llvm_versions) != 1:
        raise WasiSdkIdentityError(f"WASI SDK has no exact VERSION identity: {label}")
    return WasiSdkVersionIdentity(sdk_version, llvm_versions[0])


def read_wasi_sdk_version_identity(version_file: Path) -> WasiSdkVersionIdentity:
    """Read the exact SDK and LLVM producer identities from ``VERSION``."""

    try:
        with open_stable_regular_file(version_file, label="WASI SDK VERSION") as opened:
            if opened.stat.st_size > 64 * 1024:
                raise WasiSdkIdentityError("WASI SDK VERSION exceeds its size limit")
            raw = opened.stream.read(opened.stat.st_size + 1)
            if len(raw) > 64 * 1024:
                raise WasiSdkIdentityError("WASI SDK VERSION exceeds its size limit")
            if len(raw) != opened.stat.st_size:
                raise WasiSdkIdentityError("WASI SDK VERSION size changed")
        return _parse_wasi_sdk_version(raw, label=str(version_file))
    except (OSError, ValueError) as exc:
        raise WasiSdkIdentityError(
            f"WASI SDK VERSION is unavailable: {version_file}: {exc}"
        ) from exc


def wasi_sdk_tree_identity(root: Path) -> WasiSdkTreeIdentity:
    """Hash every SDK path, file byte, and link target without following links."""

    lexical_root = root.absolute()
    if (
        not lexical_root.is_dir()
        or lexical_root.is_symlink()
        or lexical_root.is_junction()
    ):
        raise WasiSdkIdentityError(
            f"WASI SDK root is not a real directory: {lexical_root}"
        )
    root = lexical_root.resolve(strict=True)

    pending = [root]
    identities: set[str] = set()
    records: list[tuple[str, str, int, str]] = []
    total_bytes = 0
    version = None
    while pending:
        directory = pending.pop()
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda item: item.name)
        except OSError as exc:
            raise WasiSdkIdentityError(
                f"WASI SDK directory is unreadable: {directory}: {exc}"
            ) from exc
        for entry in entries:
            path = Path(entry.path)
            relative_text = path.relative_to(root).as_posix()
            try:
                relative = portable_relative_path(relative_text)
                portable_identity = portable_path_identity(relative_text)
            except ValueError as exc:
                raise WasiSdkIdentityError(
                    f"WASI SDK path is not portable: {relative_text}"
                ) from exc
            if portable_identity in identities:
                raise WasiSdkIdentityError(
                    f"WASI SDK has a portable path collision: {relative_text}"
                )
            identities.add(portable_identity)
            if len(identities) > MAX_TREE_ENTRIES:
                raise WasiSdkIdentityError("WASI SDK tree exceeds its entry policy")

            if entry.is_symlink():
                try:
                    link_target = os.readlink(path)
                except (OSError, RuntimeError, ValueError) as exc:
                    raise WasiSdkIdentityError(
                        f"WASI SDK link escapes its root or dangles: {relative_text}"
                    ) from exc
                records.append((relative.as_posix(), "link", 0, link_target))
            elif path.is_junction():
                raise WasiSdkIdentityError(
                    f"WASI SDK contains an unsupported junction: {relative_text}"
                )
            elif entry.is_dir(follow_symlinks=False):
                records.append((relative.as_posix(), "directory", 0, ""))
                pending.append(path)
            elif entry.is_file(follow_symlinks=False):
                try:
                    with open_stable_regular_file(
                        path, label="WASI SDK file"
                    ) as opened:
                        size = opened.stat.st_size
                        if total_bytes + size > MAX_TREE_BYTES:
                            raise WasiSdkIdentityError(
                                "WASI SDK tree exceeds its total-byte policy"
                            )
                        if relative_text == "VERSION" and root.name == SDK_DIRNAME:
                            if size > 64 * 1024:
                                raise WasiSdkIdentityError(
                                    "WASI SDK VERSION exceeds its size limit"
                                )
                            raw = opened.stream.read(opened.stat.st_size + 1)
                            if len(raw) != size:
                                raise WasiSdkIdentityError(
                                    "WASI SDK VERSION size changed"
                                )
                            version = _parse_wasi_sdk_version(raw, label=relative_text)
                            digest = hashlib.sha256(raw).hexdigest()
                        else:
                            digest = stable_regular_file_handle_identity(
                                opened, label="WASI SDK file"
                            ).sha256
                except (OSError, ValueError) as exc:
                    raise WasiSdkIdentityError(
                        f"WASI SDK file is unreadable: {relative_text}: {exc}"
                    ) from exc
                total_bytes += size
                records.append((relative.as_posix(), "file", size, digest))
            else:
                raise WasiSdkIdentityError(
                    f"WASI SDK contains a special node: {relative_text}"
                )

    nodes = {row[0]: row for row in records}
    for row in records:
        if row[1] == "link":
            _record_node(nodes, row[0])
    return _tree_from_records(tuple(records), version=version)


def render_wasi_sdk_install_receipt(
    asset: dict[str, object],
    tree: WasiSdkTreeIdentity,
) -> str:
    """Render the one canonical receipt encoding the loader accepts."""

    payload = {
        "schema": INSTALL_RECEIPT_SCHEMA,
        "asset": asset,
        "tree": tree.as_record(),
        "facts": wasi_sdk_receipt_facts(asset, tree),
    }
    return dumps_exact(payload, indent=None)


def load_wasi_sdk_install_receipt(prefix: Path) -> dict[str, Any]:
    """Decode and validate the exact provision receipt schema."""

    path = prefix / INSTALL_RECEIPT_FILENAME
    try:
        payload = read_exact(
            path, max_bytes=64 * 1024, label="WASI SDK provision receipt"
        )
    except (OSError, ValueError) as exc:
        raise WasiSdkIdentityError(
            f"WASI SDK provision receipt is invalid: {path}: {exc}"
        ) from exc
    return validate_wasi_sdk_install_receipt(payload)


def validate_wasi_sdk_selection(selection: object) -> Mapping[str, Any]:
    """Bind a finite selected generation to its exact provision receipt bytes."""
    if not isinstance(selection, Mapping) or set(selection) != {
        "sdk",
        "receipt",
        "generation",
    }:
        raise WasiSdkIdentityError("WASI SDK selection is malformed or incomplete")
    sdk_raw = selection["sdk"]
    if not isinstance(sdk_raw, str):
        raise WasiSdkIdentityError("WASI SDK selection is malformed")
    sdk = Path(sdk_raw)
    if (
        not sdk.is_absolute()
        or sdk.name != SDK_DIRNAME
        or ".." in sdk.parts
        or str(sdk) != sdk_raw
    ):
        raise WasiSdkIdentityError("WASI SDK root is invalid")
    generation = validate_wasi_sdk_install_receipt(selection["generation"])
    receipt = selection["receipt"]
    encoded = dumps_exact(generation, indent=None).encode("utf-8")
    if (
        not isinstance(receipt, Mapping)
        or set(receipt) != {"path", "size_bytes", "sha256"}
        or receipt["path"] != str(sdk.parent / INSTALL_RECEIPT_FILENAME)
        or receipt["size_bytes"] != len(encoded)
        or receipt["sha256"] != hashlib.sha256(encoded).hexdigest()
    ):
        raise WasiSdkIdentityError(
            "WASI SDK receipt does not bind its finite generation"
        )
    return selection


def capture_wasi_sdk_tool_files(
    selection: Mapping[str, object],
) -> list[dict[str, object]]:
    """Capture build helper bytes once per content path, retaining lexical roles.

    Developer fingerprints and guarded execution share this finite capture.
    Ordinary compilation continues to project the managed generation receipt.
    """
    selected = validate_wasi_sdk_selection(selection)
    sdk = Path(selected["sdk"])
    tools = selected["generation"]["facts"]["tools"]
    captured: dict[Path, dict[str, object]] = {}
    result = []
    for role in SDK_BUILD_TOOL_NAMES:
        fact = tools[role]
        path, content = sdk / fact["path"], sdk / fact["content_path"]
        if path.resolve(strict=True) != content:
            raise WasiSdkIdentityError(
                "WASI SDK helper alias differs from its provisioned generation"
            )
        if content not in captured:
            actual = stable_regular_file_identity(
                content, label="WASI SDK build helper"
            )
            captured[content] = {"sha256": actual.sha256, "size_bytes": actual.size}
        identity = captured[content]
        if (
            identity["sha256"] != fact["sha256"]
            or identity["size_bytes"] != fact["size"]
        ):
            raise WasiSdkIdentityError(
                "WASI SDK helper differs from its provisioned generation"
            )
        result.append({**identity, "role": role, "path": str(path)})
    return result


def validate_wasi_sdk_install_receipt(payload: object) -> dict[str, Any]:
    """Validate the finite receipt without reading the managed installation."""
    if not isinstance(payload, dict) or set(payload) != {
        "schema",
        "asset",
        "tree",
        "facts",
    }:
        raise WasiSdkIdentityError(
            "WASI SDK receipt requires v2; run tools/provision_wasi_sdk.py to verify/upgrade it"
        )
    asset = payload["asset"]
    tree = payload["tree"]
    if (
        payload["schema"] != INSTALL_RECEIPT_SCHEMA
        or not isinstance(asset, dict)
        or set(asset) != ASSET_RECORD_KEYS
        or not is_wasi_sdk_host_id(asset["id"])
        or not is_wasi_sdk_version(asset["sdk_version"])
        or not is_wasi_sdk_llvm_version(asset["llvm_version"])
        or not isinstance(asset["url"], str)
        or not asset["url"].startswith("https://")
        or type(asset["size"]) is not int
        or asset["size"] <= 0
        or not _is_sha256(asset["sha256"])
        or not isinstance(asset["archive_root"], str)
        or not asset["archive_root"]
        or not isinstance(asset["provenance_url"], str)
        or not asset["provenance_url"].startswith("https://")
        or not _is_sha256(asset["record_sha256"])
        or not isinstance(tree, dict)
        or set(tree) != {"schema", "entries", "total_bytes", "sha256"}
        or tree["schema"] != TREE_IDENTITY_SCHEMA
        or type(tree["entries"]) is not int
        or not 0 < tree["entries"] <= MAX_TREE_ENTRIES
        or type(tree["total_bytes"]) is not int
        or not 0 < tree["total_bytes"] <= MAX_TREE_BYTES
        or not _is_sha256(tree["sha256"])
    ):
        raise WasiSdkIdentityError("WASI SDK provision receipt identity is invalid")
    facts = payload["facts"]
    if not isinstance(facts, dict) or set(facts) != {"tools", "members", "resources"}:
        raise WasiSdkIdentityError("WASI SDK finite receipt facts are incomplete")
    sdk = Path(SDK_DIRNAME)
    expected = {
        "tools": {
            role: "bin/" + executable_filename(role, asset["id"])
            for role in (*SDK_TOOL_NAMES, *SDK_OPTIONAL_TOOL_NAMES)
        },
        "members": {
            role: path.relative_to(sdk).as_posix()
            for role, path in zip(
                WASI_C_ABI_ROLES,
                wasi_c_abi_member_paths(
                    sdk, sdk / "share/wasi-sysroot", asset["llvm_version"]
                ),
                strict=True,
            )
        },
    }
    for group, roles in expected.items():
        if not isinstance(facts[group], dict) or set(facts[group]) != set(roles):
            raise WasiSdkIdentityError(f"WASI SDK {group} roles are incomplete")
        for role, relative in roles.items():
            row = facts[group][role]
            if group == "tools" and role in SDK_OPTIONAL_TOOL_NAMES and row is None:
                continue
            if (
                not isinstance(row, dict)
                or set(row) != {"path", "content_path", "size", "sha256"}
                or row["path"] != relative
                or type(row["size"]) is not int
                or not 0 <= row["size"] <= MAX_TREE_BYTES
                or not _is_sha256(row["sha256"])
            ):
                raise WasiSdkIdentityError(f"WASI SDK {role} fact is invalid")
            if not isinstance(row["content_path"], str):
                raise WasiSdkIdentityError(f"WASI SDK {role} content path is invalid")
            portable_relative_path(row["content_path"])
    resources = facts["resources"]
    if not isinstance(resources, dict) or set(resources) != set(SDK_RESOURCE_ROOTS):
        raise WasiSdkIdentityError("WASI SDK resource roots are incomplete")
    for row in resources.values():
        if (
            not isinstance(row, dict)
            or set(row) != {"schema", "entries", "total_bytes", "sha256"}
            or row["schema"] != TREE_IDENTITY_SCHEMA
            or type(row["entries"]) is not int
            or not 0 < row["entries"] <= MAX_TREE_ENTRIES
            or type(row["total_bytes"]) is not int
            or not 0 < row["total_bytes"] <= MAX_TREE_BYTES
            or not _is_sha256(row["sha256"])
        ):
            raise WasiSdkIdentityError("WASI SDK resource identity is invalid")
    return payload
