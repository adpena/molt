"""One wheel-compatibility authority for shipped native binaries.

A platform wheel's tag is a claim about the binaries it carries, so it is
derived from those binaries by the maintained audit tools, never from the host:

* Linux: ``auditwheel.wheel_abi.analyze_wheel_abi`` (exact-pinned in the release
  dependency group) evaluates every ELF member against the manylinux policies:
  external library closure without grafting, GLIBC/GLIBCXX/CXXABI symbol
  versions, the ISA/machine level and the blacklist. Its overall policy is the
  tag; the unconstrained ``linux`` policy is a failure, not a generic tag.
* macOS: delocate (exact-pinned) requires a system-only dylib closure and
  computes the minimum deployment target and architecture from each Mach-O
  load command, as ``delocate-wheel`` does.
* Windows: release targets map exactly to ``win_amd64``/``win_arm64``.

The producer derives the tag from the assembled bundle's native members before
writing the wheel and re-audits the written wheel. The release consumer audits
the same wheel and every program it links against a shipped runtime cell;
static runtime archives are only observable through such linked programs.
Nothing is repaired, grafted or re-tagged: an unsatisfied policy raises
``WheelCompatibilityError`` with the tool's evidence and leaves artifacts in
place for repair.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
import importlib.metadata
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile
import zipfile

from packaging.utils import parse_wheel_filename

_WINDOWS_TAGS = {"x86_64": "win_amd64", "arm64": "win_arm64"}
_TAG_PREFIX = {"linux": "manylinux", "macos": "macosx"}
# Header identification only; interpretation belongs to the audit tools.
_NATIVE_MAGIC = frozenset(
    {
        b"\x7fELF",
        b"\xfe\xed\xfa\xce",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe",
    }
)
_AUDIT_DISTRIBUTION = "molt_binary_audit"


class WheelCompatibilityError(ValueError):
    """Shipped binaries do not satisfy any admissible wheel policy."""

    def __init__(self, message: str, evidence: Mapping[str, object]) -> None:
        super().__init__(message)
        self.evidence = dict(evidence)


@dataclass(frozen=True)
class WheelCompatibility:
    platform: str
    arch: str
    tag: str
    evidence: Mapping[str, object] = field(default_factory=dict)


def wheel_platform_tag_matches(platform: str, arch: str, tag: str) -> bool:
    """Whether one wheel platform tag names exactly this release coordinate."""
    if platform == "windows":
        return _WINDOWS_TAGS.get(arch) == tag
    prefix = _TAG_PREFIX.get(platform)
    return (
        prefix is not None
        and re.fullmatch(rf"{prefix}_(\d+)_(\d+)_{re.escape(arch)}", tag) is not None
    )


def tag_floor(platform: str, arch: str, tag: str) -> tuple[int, int]:
    """The (major, minor) C-library or OS floor a matching tag claims."""
    if not wheel_platform_tag_matches(platform, arch, tag) or platform == "windows":
        raise ValueError(f"{tag} has no {platform}/{arch} version floor")
    major, minor = tag[len(_TAG_PREFIX[platform]) + 1 :].split("_")[:2]
    return int(major), int(minor)


def _provisional_tag(platform: str, arch: str) -> str:
    # The audit tools read versions from the binaries, not from this name; it
    # only names the architecture the wheel claims to carry.
    if platform == "linux":
        return f"linux_{arch}"
    return f"macosx_{'11_0' if arch == 'arm64' else '10_9'}_{arch}"


def _tool_version(name: str) -> str:
    return importlib.metadata.version(name)


def native_members(root: Path) -> dict[str, Path]:
    """Every regular file under ``root`` whose header is ELF or Mach-O."""
    members: dict[str, Path] = {}
    for path in sorted(
        p for p in root.rglob("*") if p.is_file() and not p.is_symlink()
    ):
        with path.open("rb") as handle:
            if handle.read(4) in _NATIVE_MAGIC:
                members[path.relative_to(root).as_posix()] = path
    return members


def _write_audit_wheel(members: Mapping[str, Path], directory: Path, tag: str) -> Path:
    """Transient wheel carrying only native members for the tools' wheel APIs."""
    wheel = directory / f"{_AUDIT_DISTRIBUTION}-0-py3-none-{tag}.whl"
    data = PurePosixPath(f"{_AUDIT_DISTRIBUTION}-0.data", "data")
    dist_info = f"{_AUDIT_DISTRIBUTION}-0.dist-info"
    with zipfile.ZipFile(wheel, "w") as archive:
        for relative, path in sorted(members.items()):
            info = zipfile.ZipInfo((data / relative).as_posix())
            info.external_attr = (
                0o100755 if os.access(path, os.X_OK) else 0o100644
            ) << 16
            archive.writestr(info, path.read_bytes())
        archive.writestr(
            f"{dist_info}/METADATA",
            f"Metadata-Version: 2.1\nName: {_AUDIT_DISTRIBUTION}\nVersion: 0\n",
        )
        archive.writestr(
            f"{dist_info}/WHEEL",
            f"Wheel-Version: 1.0\nRoot-Is-Purelib: false\nTag: py3-none-{tag}\n",
        )
        archive.writestr(f"{dist_info}/RECORD", "")
    return wheel


def _audit_linux(wheel: Path, arch: str) -> tuple[str, dict[str, object]]:
    from auditwheel.architecture import Architecture
    from auditwheel.libc import Libc
    from auditwheel.wheel_abi import NonPlatformWheelError, analyze_wheel_abi

    evidence: dict[str, object] = {
        "tool": "auditwheel",
        "version": _tool_version("auditwheel"),
    }
    try:
        info = analyze_wheel_abi(
            Libc.GLIBC,
            Architecture(arch),
            wheel,
            frozenset(),
            disable_isa_ext_check=False,
            allow_graft=False,
        )
    except NonPlatformWheelError as exc:
        raise WheelCompatibilityError(
            f"no {arch} ELF members to audit: {exc}", evidence
        ) from exc
    evidence.update(
        overall_policy=info.overall_policy.name,
        symbol_policy=info.sym_policy.name,
        external_reference_policy=info.ref_policy.name,
        machine_policy=info.machine_policy.name,
        blacklist_policy=info.blacklist_policy.name,
        versioned_symbols={
            library: sorted(symbols)
            for library, symbols in sorted(info.versioned_symbols.items())
        },
        external_libraries={
            policy: sorted(reference.libs)
            for policy, reference in sorted(info.external_refs.items())
            if reference.libs
        },
        blacklisted={
            policy: {
                name: sorted(values) for name, values in reference.blacklist.items()
            }
            for policy, reference in sorted(info.external_refs.items())
            if reference.blacklist
        },
    )
    if info.overall_policy.priority <= info.policies.linux.priority:
        raise WheelCompatibilityError(
            "no manylinux policy admits the shipped ELF binaries", evidence
        )
    return info.overall_policy.name, evidence


def _audit_macos(root: Path, arch: str) -> tuple[str, dict[str, object]]:
    # delocate exposes the deployment-target computation that delocate-wheel
    # uses as a module function; it is consumed at the exact pinned version.
    from delocate.delocating import (
        _calculate_minimum_wheel_name,
        _get_macos_min_version,
        filter_system_libs,
    )
    from delocate.libsana import tree_libs_from_directory

    evidence: dict[str, object] = {
        "tool": "delocate",
        "version": _tool_version("delocate"),
        "minimum_versions": {
            relative: sorted(
                [cpu, str(version)] for cpu, version in _get_macos_min_version(path)
            )
            for relative, path in native_members(root).items()
        },
    }
    try:
        dependencies = tree_libs_from_directory(str(root), ignore_missing=False)
    except Exception as exc:  # delocate reports unresolved dependencies
        evidence["unresolved"] = str(exc)
        raise WheelCompatibilityError(
            "shipped Mach-O dependencies do not resolve", evidence
        ) from exc
    resolved_root = root.resolve()
    external = sorted(
        library
        for library in dependencies
        if filter_system_libs(library)
        and not Path(library).resolve().is_relative_to(resolved_root)
    )
    evidence["non_system_dependencies"] = external
    if external:
        raise WheelCompatibilityError(
            "shipped Mach-O binaries depend on non-system libraries", evidence
        )
    provisional = (
        f"{_AUDIT_DISTRIBUTION}-0-py3-none-{_provisional_tag('macos', arch)}.whl"
    )
    try:
        name, _unused = _calculate_minimum_wheel_name(provisional, root, None)
    except Exception as exc:  # e.g. no binary carries the claimed architecture
        evidence["error"] = str(exc)
        raise WheelCompatibilityError(
            "delocate cannot derive a macOS platform tag", evidence
        ) from exc
    tags = {tag.platform for tag in parse_wheel_filename(name)[3]}
    if len(tags) != 1:
        raise WheelCompatibilityError("delocate derived more than one tag", evidence)
    tag = tags.pop()
    evidence["derived_tag"] = tag
    return tag, evidence


def _audit(
    wheel: Path, *, platform: str, arch: str, root: Path | None = None
) -> WheelCompatibility:
    if platform == "windows":
        tag, evidence = _WINDOWS_TAGS[arch], {"policy": "exact-release-target"}
    elif platform == "linux":
        tag, evidence = _audit_linux(wheel, arch)
    elif platform == "macos":
        if root is not None:
            tag, evidence = _audit_macos(root, arch)
        else:
            with tempfile.TemporaryDirectory(prefix="molt-wheel-audit-") as temporary:
                extracted = Path(temporary)
                with zipfile.ZipFile(wheel) as archive:
                    archive.extractall(extracted)
                tag, evidence = _audit_macos(extracted, arch)
    else:
        raise ValueError(f"unsupported release platform: {platform}")
    if not wheel_platform_tag_matches(platform, arch, tag):
        raise WheelCompatibilityError(
            f"audited tag {tag} does not name {platform}/{arch}", evidence
        )
    return WheelCompatibility(platform, arch, tag, evidence)


def audit_wheel(wheel: Path, *, platform: str, arch: str) -> WheelCompatibility:
    """Audit a written wheel; its own tag must equal the derived tag."""
    compatibility = _audit(wheel, platform=platform, arch=arch)
    claimed = {tag.platform for tag in parse_wheel_filename(wheel.name)[3]}
    if claimed != {compatibility.tag}:
        raise WheelCompatibilityError(
            f"{wheel.name} claims {sorted(claimed)}, its binaries admit "
            f"{compatibility.tag}",
            compatibility.evidence,
        )
    return compatibility


def derive_bundle_wheel_compatibility(
    bundle_root: Path, *, platform: str, arch: str
) -> WheelCompatibility:
    """Derive the platform tag from an assembled bundle's native members."""
    if platform == "windows":
        return _audit(bundle_root, platform=platform, arch=arch)
    members = native_members(bundle_root)
    if not members:
        raise WheelCompatibilityError(
            "the bundle carries no native binaries to derive a tag from",
            {"bundle": str(bundle_root)},
        )
    with tempfile.TemporaryDirectory(prefix="molt-wheel-audit-") as temporary:
        audit = _write_audit_wheel(
            members, Path(temporary), _provisional_tag(platform, arch)
        )
        return _audit(
            audit,
            platform=platform,
            arch=arch,
            root=bundle_root if platform == "macos" else None,
        )


def audit_linked_executable(
    executable: Path, *, platform: str, arch: str, claimed_tag: str
) -> WheelCompatibility:
    """Require a program linked against shipped runtime cells to fit the claim.

    Static runtime archives carry no symbol versions or deployment targets of
    their own; the program the installed compiler links from them is the
    binary evidence of their floor.
    """
    if platform == "windows":
        return _audit(executable, platform=platform, arch=arch)
    with tempfile.TemporaryDirectory(prefix="molt-link-audit-") as temporary:
        root = Path(temporary) / "tree"
        root.mkdir()
        copy = root / executable.name
        shutil.copy2(executable, copy)
        audit = _write_audit_wheel(
            {executable.name: copy}, Path(temporary), _provisional_tag(platform, arch)
        )
        linked = _audit(
            audit,
            platform=platform,
            arch=arch,
            root=root if platform == "macos" else None,
        )
    if tag_floor(platform, arch, linked.tag) > tag_floor(platform, arch, claimed_tag):
        raise WheelCompatibilityError(
            f"{executable.name} requires {linked.tag}, newer than the shipped "
            f"wheel's {claimed_tag}",
            {"executable": str(executable), **linked.evidence},
        )
    return linked
