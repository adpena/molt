"""Canonical differential-test selection and coordinate policy.

This module is the sole authority for expanding differential suites, parsing
``MOLT_META`` headers, deciding coordinate applicability, and classifying
expected Molt failures.  The execution harness, coverage/honesty tooling, and
verified-subset release receipts all consume this same projection.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import platform as platform_module
import re
import stat
import sys
import token
import tokenize
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.file_hashing import content_change_time_ns
from molt.file_publication import (
    is_link_like,
    metadata_is_link_like,
    resolve_owned_path,
)
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)


ROOT = Path(__file__).resolve().parents[2]
CPYTHON_EQUIVALENCE_SCOPE = "cpython_equivalence"
VERIFICATION_SCOPES = (
    "capability_policy",
    CPYTHON_EQUIVALENCE_SCOPE,
    "dynamic_execution_policy",
)
ALL_BACKENDS = ("native", "llvm", "wasm", "luau")
PLATFORM_SELECTORS = ("freebsd", "linux", "macos", "posix", "windows")
ARCHITECTURE_SELECTORS = ("aarch64", "arm64", "x86_64")
STDOUT_MODES = ("exact", "pyperformance")
STDERR_MODES = ("ignore", "exact", "exception_signature")
STDLIB_PROFILES = ("full",)

_METADATA_PREFIX = "# MOLT_META:"
_METADATA_KEYS = frozenset(
    {
        "architectures",
        "backends",
        "expect_fail",
        "expect_fail_reason",
        "max_py",
        "min_py",
        "platforms",
        "stderr",
        "stdlib_profile",
        "stdout",
        "verified_subset_scope",
    }
)
_LIST_KEYS = frozenset({"architectures", "backends", "platforms"})
_PYTHON_MINOR_RE = re.compile(r"3\.(?:0|[1-9][0-9]*)\Z", re.ASCII)
_REASON_RE = re.compile(r"[a-z][a-z0-9_]*\Z", re.ASCII)
_RAW_METADATA_CANDIDATE_RE = re.compile(
    r"^[ \t]*#[ \t]*MOLT_META\b[^\r\n]*", re.ASCII | re.MULTILINE
)
_COMMENT_METADATA_CANDIDATE_RE = re.compile(r"^#[ \t]*MOLT_META\b", re.ASCII)


def normalize_repo_relative(path: str | Path, *, repo_root: Path = ROOT) -> str:
    """Return one resolved, repo-relative POSIX identity when possible."""

    root = repo_root.resolve()
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = root / candidate
    candidate = candidate.resolve()
    try:
        return candidate.relative_to(root).as_posix()
    except ValueError:
        return candidate.as_posix()


def parse_version(value: str) -> tuple[int, int]:
    """Parse one canonical supported-or-future CPython minor version."""

    if _PYTHON_MINOR_RE.fullmatch(value) is None:
        raise ValueError(
            "MOLT_META Python versions must be exact 3.<minor> values with minor >= 12"
        )
    major, minor = value.split(".")
    parsed = int(major), int(minor)
    if parsed < (3, 12):
        raise ValueError(
            "MOLT_META Python versions must be exact 3.<minor> values with minor >= 12"
        )
    return parsed


@dataclass(frozen=True, slots=True)
class TestMetadata:
    """Exact typed policy carried by one differential source file."""

    verification_scope: str = CPYTHON_EQUIVALENCE_SCOPE
    expect_molt_fail: bool = False
    expected_failure_reason: str | None = None
    min_python: tuple[int, int] | None = None
    max_python: tuple[int, int] | None = None
    platforms: frozenset[str] = frozenset()
    architectures: frozenset[str] = frozenset()
    backends: frozenset[str] = frozenset()
    stdout_mode: str = "exact"
    stderr_mode: str = "ignore"
    stdlib_profile: str | None = None

    def as_record(self) -> dict[str, object]:
        """Return the canonical JSON-ready representation of non-default fields."""

        record: dict[str, object] = {}
        if self.verification_scope != CPYTHON_EQUIVALENCE_SCOPE:
            record["verified_subset_scope"] = self.verification_scope
        if self.expect_molt_fail:
            record["expect_fail"] = "molt"
            record["expect_fail_reason"] = self.expected_failure_reason
        if self.min_python is not None:
            record["min_py"] = f"{self.min_python[0]}.{self.min_python[1]}"
        if self.max_python is not None:
            record["max_py"] = f"{self.max_python[0]}.{self.max_python[1]}"
        for key, values in (
            ("platforms", self.platforms),
            ("architectures", self.architectures),
            ("backends", self.backends),
        ):
            if values:
                record[key] = sorted(values)
        if self.stdout_mode != "exact":
            record["stdout"] = self.stdout_mode
        if self.stderr_mode != "ignore":
            record["stderr"] = self.stderr_mode
        if self.stdlib_profile is not None:
            record["stdlib_profile"] = self.stdlib_profile
        return record

    def exclusion_reason(
        self,
        *,
        python_version: tuple[int, int] | None,
        platform_tags: frozenset[str] | set[str],
        architecture: str | None = None,
        backend: str | None = None,
    ) -> str | None:
        if self.platforms and platform_tags.isdisjoint(self.platforms):
            return f"platform {sorted(self.platforms)}"
        if self.architectures and (
            architecture is None or architecture not in self.architectures
        ):
            return f"architecture {sorted(self.architectures)}"
        if self.backends and (backend is None or backend not in self.backends):
            return f"backend {sorted(self.backends)}"
        return self.python_exclusion_reason(python_version)

    def python_exclusion_reason(
        self, python_version: tuple[int, int] | None
    ) -> str | None:
        """Project version applicability without inventing a backend coordinate.

        Static corpus consumers use this before parsing. Execution consumers
        continue through exclusion_reason for the full coordinate policy.
        """
        if python_version is not None:
            if self.min_python is not None and python_version < self.min_python:
                return f"min_py {self.min_python[0]}.{self.min_python[1]}"
            if self.max_python is not None and python_version > self.max_python:
                return f"max_py {self.max_python[0]}.{self.max_python[1]}"
        return None


def _read_source(file_path: Path) -> tuple[bytes, StableRegularFileIdentity]:
    """Read one stable generation, shared by metadata and content identity."""
    try:
        identity, raw = capture_stable_regular_file(
            file_path, label="differential metadata source"
        )
    except (OSError, ValueError) as exc:
        raise ValueError(
            f"cannot read differential metadata source {file_path}: {exc}"
        ) from exc
    return raw, identity


def _metadata_comments(file_path: Path, raw: bytes) -> tuple[tuple[int, str], ...]:
    """Return actual Python comment tokens that declare ``MOLT_META``."""

    try:
        encoding, _ = tokenize.detect_encoding(io.BytesIO(raw).readline)
        text = raw.decode(encoding)
    except (LookupError, SyntaxError, UnicodeError) as exc:
        raise ValueError(
            f"cannot decode differential metadata source {file_path}: {exc}"
        ) from exc
    raw_candidate_count = len(_RAW_METADATA_CANDIDATE_RE.findall(text))
    if raw_candidate_count == 0:
        return ()

    comments: list[tuple[int, str]] = []
    try:
        tokens = tokenize.tokenize(io.BytesIO(raw).readline)
        for item in tokens:
            if (
                item.type != token.COMMENT
                or _COMMENT_METADATA_CANDIDATE_RE.match(item.string) is None
            ):
                continue
            if not item.string.startswith(_METADATA_PREFIX):
                raise ValueError(
                    f"malformed MOLT_META comment at {file_path}:{item.start[0]}"
                )
            comments.append((item.start[0], item.string))
    except (IndentationError, SyntaxError, UnicodeError, tokenize.TokenError) as exc:
        # Differential inputs may intentionally exercise syntax errors. A later
        # lexical failure is irrelevant only when tokenize already classified
        # every raw marker occurrence as an actual metadata comment.
        if len(comments) != raw_candidate_count:
            raise ValueError(
                f"cannot tokenize differential metadata source {file_path}: {exc}"
            ) from exc
    return tuple(comments)


def _parse_tokens(
    file_path: Path, line: int, payload: str
) -> dict[str, tuple[str, ...]]:
    raw: dict[str, tuple[str, ...]] = {}
    if not payload:
        raise ValueError(f"empty MOLT_META declaration at {file_path}:{line}")
    for item in payload.split():
        if item.count("=") != 1:
            raise ValueError(
                f"malformed MOLT_META token {item!r} at {file_path}:{line}"
            )
        key, encoded = item.split("=", 1)
        if key not in _METADATA_KEYS:
            raise ValueError(f"unknown MOLT_META key {key!r} at {file_path}:{line}")
        if key in raw:
            raise ValueError(f"duplicate MOLT_META key {key!r} at {file_path}:{line}")
        values = tuple(encoded.split(","))
        if not encoded or any(not value for value in values):
            raise ValueError(f"empty MOLT_META value for {key!r} at {file_path}:{line}")
        if len(values) != len(set(values)):
            raise ValueError(
                f"duplicate MOLT_META value for {key!r} at {file_path}:{line}"
            )
        if key not in _LIST_KEYS and len(values) != 1:
            raise ValueError(f"MOLT_META {key} must select exactly one value")
        if key in _LIST_KEYS and values != tuple(sorted(values)):
            raise ValueError(f"MOLT_META {key} values must be sorted and unique")
        raw[key] = values
    return raw


def _enum_values(
    raw: dict[str, tuple[str, ...]], key: str, allowed: Sequence[str]
) -> tuple[str, ...]:
    values = raw.get(key, ())
    unknown = set(values).difference(allowed)
    if unknown:
        raise ValueError(
            f"MOLT_META {key} contains unknown values: {', '.join(sorted(unknown))}"
        )
    return values


def parse_metadata(file_path: str | Path) -> TestMetadata:
    """Parse and validate one exact, typed ``MOLT_META`` declaration."""

    path = Path(file_path)
    raw, _identity = _read_source(path)
    return _parse_metadata_bytes(path, raw)


def _parse_metadata_bytes(path: Path, source_bytes: bytes) -> TestMetadata:
    comments = _metadata_comments(path, source_bytes)
    if not comments:
        return TestMetadata()
    if len(comments) != 1:
        locations = ", ".join(str(line) for line, _ in comments)
        raise ValueError(
            f"multiple MOLT_META declarations in {path}: lines {locations}"
        )
    line, comment = comments[0]
    raw = _parse_tokens(path, line, comment.removeprefix(_METADATA_PREFIX).strip())

    scope = raw.get("verified_subset_scope", (CPYTHON_EQUIVALENCE_SCOPE,))[0]
    if scope not in VERIFICATION_SCOPES:
        raise ValueError(
            "MOLT_META verified_subset_scope must be one of "
            + ", ".join(VERIFICATION_SCOPES)
        )

    expect_values = raw.get("expect_fail", ())
    reason_values = raw.get("expect_fail_reason", ())
    if bool(expect_values) != bool(reason_values):
        raise ValueError(
            "MOLT_META expect_fail=molt and expect_fail_reason must be declared together"
        )
    if expect_values and expect_values != ("molt",):
        raise ValueError("MOLT_META expect_fail must be exactly 'molt'")
    reason = reason_values[0] if reason_values else None
    if reason is not None and _REASON_RE.fullmatch(reason) is None:
        raise ValueError("MOLT_META expect_fail_reason must be a lowercase identifier")

    min_python = parse_version(raw["min_py"][0]) if "min_py" in raw else None
    max_python = parse_version(raw["max_py"][0]) if "max_py" in raw else None
    if min_python is not None and max_python is not None and min_python > max_python:
        raise ValueError("MOLT_META min_py must not exceed max_py")

    platforms = _enum_values(raw, "platforms", PLATFORM_SELECTORS)
    if "posix" in platforms and len(platforms) != 1:
        raise ValueError(
            "MOLT_META platforms=posix must not duplicate concrete POSIX values"
        )
    architectures = _enum_values(raw, "architectures", ARCHITECTURE_SELECTORS)
    backends = _enum_values(raw, "backends", ALL_BACKENDS)
    stdout_mode = _enum_values(raw, "stdout", STDOUT_MODES)
    stderr_mode = _enum_values(raw, "stderr", STDERR_MODES)
    profiles = _enum_values(raw, "stdlib_profile", STDLIB_PROFILES)

    return TestMetadata(
        verification_scope=scope,
        expect_molt_fail=bool(expect_values),
        expected_failure_reason=reason,
        min_python=min_python,
        max_python=max_python,
        platforms=frozenset(platforms),
        architectures=frozenset(architectures),
        backends=frozenset(backends),
        stdout_mode=stdout_mode[0] if stdout_mode else "exact",
        stderr_mode=stderr_mode[0] if stderr_mode else "ignore",
        stdlib_profile=profiles[0] if profiles else None,
    )


def resolve_expected_failure_status(
    *, expect_molt_fail: bool, raw_status: str, cpython_returncode: int
) -> tuple[str, str | None]:
    if not expect_molt_fail or cpython_returncode != 0:
        return raw_status, None
    if raw_status == "fail":
        return "pass", "xfail"
    if raw_status == "pass":
        return "fail", "xpass"
    return raw_status, None


def coordinate_platform_tags(*, platform: str) -> frozenset[str]:
    if platform not in PLATFORM_SELECTORS or platform == "posix":
        raise ValueError(f"unknown concrete platform {platform!r}")
    platform_name = platform
    tags = {platform_name}
    if platform_name in {"linux", "macos", "freebsd"}:
        tags.add("posix")
    return frozenset(tags)


def current_platform_name() -> str:
    if sys.platform.startswith("linux"):
        return "linux"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("freebsd"):
        return "freebsd"
    if os.name == "nt":
        return "windows"
    detected = platform_module.system().strip().lower()
    if detected not in PLATFORM_SELECTORS or detected == "posix":
        raise ValueError(f"unsupported host platform {detected!r}")
    return detected


def current_architecture() -> str:
    raw = platform_module.machine().strip().lower()
    normalized = {
        "amd64": "x86_64",
        "x86_64": "x86_64",
        "aarch64": "aarch64",
        "arm64": "arm64",
    }.get(raw)
    if normalized is None:
        raise ValueError(f"unsupported host architecture {raw!r}")
    return normalized


def current_platform_tags() -> frozenset[str]:
    return coordinate_platform_tags(platform=current_platform_name())


def exclusion_reason(
    metadata: TestMetadata,
    *,
    python_version: tuple[int, int] | None,
    platform_tags: frozenset[str] | set[str],
    architecture: str | None = None,
    backend: str | None = None,
) -> str | None:
    return metadata.exclusion_reason(
        python_version=python_version,
        platform_tags=platform_tags,
        architecture=architecture,
        backend=backend,
    )


def should_skip(
    metadata: TestMetadata,
    *,
    python_version: tuple[int, int] | None,
    host_tags: frozenset[str] | set[str],
    architecture: str | None = None,
    backend: str | None = None,
) -> tuple[bool, str | None]:
    reason = exclusion_reason(
        metadata,
        python_version=python_version,
        platform_tags=host_tags,
        architecture=architecture,
        backend=backend,
    )
    return reason is not None, reason


def collect_test_files(
    targets: Sequence[str | Path],
    *,
    pattern: str = "*.py",
    repo_root: Path = ROOT,
) -> tuple[Path, ...]:
    """Expand manifests/directories into one canonical, duplicate-free closure."""

    root = repo_root.resolve()
    files: dict[str, Path] = {}
    portable_identities: dict[str, str] = {}

    def add(candidate: Path) -> None:
        absolute = candidate.absolute()
        resolved = candidate.resolve(strict=True)
        if is_link_like(candidate) or absolute != resolved:
            raise ValueError(f"differential test must not be a link: {candidate}")
        if not resolved.is_relative_to(root):
            raise ValueError(f"differential test escapes the repository: {candidate}")
        if not resolved.is_file() or resolved.suffix != ".py":
            return
        identity = portable_relative_path(
            resolved.relative_to(root).as_posix()
        ).as_posix()
        portable_identity = portable_path_identity(identity)
        prior = portable_identities.get(portable_identity)
        if prior is not None and prior != identity:
            raise ValueError(
                "differential tests collide on portable filesystems: "
                f"{prior!r}, {identity!r}"
            )
        if identity in files:
            raise ValueError(
                f"differential test is selected more than once: {identity}"
            )
        portable_identities[portable_identity] = identity
        files[identity] = resolved

    def expand(target: Path) -> None:
        candidate = target if target.is_absolute() else root / target
        if is_link_like(candidate) or candidate.absolute() != candidate.resolve(
            strict=True
        ):
            raise ValueError(f"differential suite must not traverse a link: {target}")
        if candidate.is_dir():
            manifest = candidate / "TESTS.txt"
            if manifest.is_file():
                for raw in manifest.read_text(encoding="utf-8").splitlines():
                    entry = raw.strip()
                    if not entry or entry.startswith("#"):
                        continue
                    manifest_target = Path(entry)
                    if not manifest_target.is_absolute():
                        manifest_target = root / manifest_target
                    if manifest_target.is_dir():
                        for match in sorted(manifest_target.glob(pattern)):
                            add(match)
                    else:
                        add(manifest_target)
                return
            for match in sorted(candidate.glob(pattern)):
                add(match)
            return
        add(candidate)

    for target in targets:
        expand(Path(target))
    return tuple(files[name] for name in sorted(files))


@dataclass(frozen=True, slots=True)
class _DirectoryGeneration:
    path: Path
    object_identity: tuple[int, int, int]
    membership_generation: tuple[int, int] | None

    def verify(self) -> None:
        current = _directory_generation(
            self.path, membership=self.membership_generation is not None
        )
        if current != self:
            raise ValueError(f"differential inventory directory changed: {self.path}")


def _directory_generation(path: Path, *, membership: bool) -> _DirectoryGeneration:
    """Bind direct directory topology, and membership only where enumerated.

    Directory allocation size and access time are not membership identities.
    Ancestors outside selected suites retain object custody without invalidating
    a transaction merely because an unrelated sibling was created.
    """
    try:
        before = path.lstat()
        if not stat.S_ISDIR(before.st_mode) or metadata_is_link_like(before):
            raise ValueError(
                f"differential inventory directory is linked or invalid: {path}"
            )
        change_time = content_change_time_ns(path, before) if membership else None
        after = path.lstat()
    except OSError as exc:
        raise ValueError(
            f"differential inventory directory is unavailable: {path}"
        ) from exc
    object_identity = (before.st_dev, before.st_ino, before.st_mode)
    if object_identity != (after.st_dev, after.st_ino, after.st_mode) or (
        membership
        and (before.st_mtime_ns, before.st_ctime_ns)
        != (after.st_mtime_ns, after.st_ctime_ns)
    ):
        raise ValueError(f"differential inventory directory changed: {path}")
    if membership and change_time is None:
        raise ValueError(
            f"differential inventory directory change time is unavailable: {path}"
        )
    return _DirectoryGeneration(
        path,
        object_identity,
        (before.st_mtime_ns, change_time) if change_time is not None else None,
    )


@dataclass(frozen=True, slots=True)
class _PhysicalTestSelection:
    repo_root: Path
    files: tuple[Path, ...]
    relative_paths: tuple[str, ...]
    suite_members: tuple[tuple[str, ...], ...]
    directories: tuple[_DirectoryGeneration, ...]

    def verify(self) -> None:
        for directory in self.directories:
            directory.verify()


class _DirectoryAdmission:
    """Shared ancestry admission for physical suites and explicit source lists."""

    def __init__(self) -> None:
        self.generations: dict[Path, _DirectoryGeneration] = {}

    def admit(self, path: Path, *, membership: bool) -> None:
        prior = self.generations.get(path)
        if prior is not None:
            prior.verify()
            if prior.membership_generation is not None or not membership:
                return
        self.generations[path] = _directory_generation(path, membership=membership)

    def ancestry(self, path: Path) -> None:
        for parent in reversed(path.parents):
            if parent not in self.generations:
                self.admit(parent, membership=False)


def _admit_source_path(
    path: Path,
    root: Path,
    files: dict[str, Path],
    portable_identities: dict[str, str],
) -> str:
    """Own one portable source identity after no-follow path admission."""
    if not path.is_relative_to(root) or path.suffix != ".py":
        raise ValueError(f"differential test is not a repository Python file: {path}")
    identity = portable_relative_path(path.relative_to(root).as_posix()).as_posix()
    portable_identity = portable_path_identity(identity)
    prior = portable_identities.get(portable_identity)
    if prior is not None:
        if prior == identity:
            raise ValueError(
                f"differential test is selected by multiple suites or inputs: {identity}"
            )
        raise ValueError(
            "differential tests collide on portable filesystems: "
            f"{prior!r}, {identity!r}"
        )
    portable_identities[portable_identity] = identity
    files[identity] = path
    return identity


def _collect_physical_test_selection(
    suites: Sequence[tuple[str | Path, bool]],
    *,
    repo_root: Path = ROOT,
) -> _PhysicalTestSelection:
    """Collect exact physical ``.py`` descendants for typed suite policies.

    Generated lane manifests are scheduling projections, not release-selection
    authorities. This collector therefore inventories the real directory tree,
    rejects every symlink/reparse traversal, and applies one portable identity
    across all suites.
    """

    root = resolve_owned_path(repo_root)
    files: dict[str, Path] = {}
    portable_identities: dict[str, str] = {}
    members: list[list[str]] = [[] for _suite in suites]
    directories = _DirectoryAdmission()

    directories.ancestry(root)
    directories.admit(root, membership=False)
    for suite_index, (raw_suite, recursive) in enumerate(suites):
        if not isinstance(recursive, bool):
            raise ValueError("differential suite recursive policy must be boolean")
        candidate = Path(raw_suite)
        if not candidate.is_absolute():
            candidate = root / candidate
        directories.ancestry(candidate)
        if is_link_like(candidate):
            raise ValueError(f"differential suite must not be a link: {raw_suite}")
        absolute = candidate.absolute()
        suite_root = candidate.resolve(strict=True)
        if absolute != suite_root:
            raise ValueError(
                f"differential suite must not traverse a link: {raw_suite}"
            )
        if not suite_root.is_relative_to(root):
            raise ValueError(
                f"differential suite is not a repository directory: {raw_suite}"
            )

        pending = [suite_root]
        while pending:
            directory = pending.pop()
            directories.admit(directory, membership=True)
            with os.scandir(directory) as entries:
                ordered = sorted(entries, key=lambda entry: entry.name)
            for entry in ordered:
                path = Path(entry.path)
                metadata = entry.stat(follow_symlinks=False)
                if metadata_is_link_like(metadata):
                    raise ValueError(
                        f"differential suite contains a link or reparse point: {path}"
                    )
                if stat.S_ISDIR(metadata.st_mode):
                    if recursive:
                        pending.append(path)
                    else:
                        directories.admit(path, membership=False)
                    continue
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValueError(
                        f"differential suite contains a special entry: {path}"
                    )
                if path.suffix == ".py":
                    identity = _admit_source_path(
                        path, root, files, portable_identities
                    )
                    members[suite_index].append(identity)

    names = tuple(sorted(files))
    selection = _PhysicalTestSelection(
        root,
        tuple(files[name] for name in names),
        names,
        tuple(tuple(sorted(paths)) for paths in members),
        tuple(directories.generations.values()),
    )
    selection.verify()
    return selection


def collect_physical_test_files(
    suites: Sequence[tuple[str | Path, bool]],
    *,
    repo_root: Path = ROOT,
) -> tuple[Path, ...]:
    """Return the physical closure admitted by the shared inventory traversal."""
    return _collect_physical_test_selection(suites, repo_root=repo_root).files


@dataclass(frozen=True, slots=True)
class TestPolicySource:
    path: str
    source_sha256: str
    metadata: TestMetadata


@dataclass(frozen=True, slots=True)
class TestSourceInventory:
    """One explicit capture lifetime, not a cache or an execution snapshot."""

    repo_root: Path
    suites: tuple[tuple[str | Path, bool], ...]
    files: tuple[Path, ...]
    sources: tuple[TestPolicySource, ...]
    suite_members: tuple[tuple[str, ...], ...]
    _directories: tuple[_DirectoryGeneration, ...]
    _file_identities: tuple[StableRegularFileIdentity, ...]

    def verify_unchanged(self) -> None:
        # Fence ancestry before opening leaves and again afterward. Do not
        # retain handles or re-read source bytes to verify a captured generation.
        for directory in self._directories:
            directory.verify()
        for identity in self._file_identities:
            verify_stable_regular_file_identity(
                identity, label="differential inventory source"
            )
        for directory in self._directories:
            directory.verify()


def _load_source(
    path: Path, relative: str
) -> tuple[TestPolicySource, StableRegularFileIdentity]:
    raw, identity = _read_source(path)
    metadata = _parse_metadata_bytes(path, raw)
    if (
        metadata.verification_scope != CPYTHON_EQUIVALENCE_SCOPE
        and not metadata.expect_molt_fail
    ):
        raise ValueError(
            "verified-subset scope exclusion must remain an explicit "
            f"expected divergence: {relative}"
        )
    return TestPolicySource(relative, identity.sha256, metadata), identity


def load_test_inventory(
    suites: Sequence[tuple[str | Path, bool]], *, repo_root: Path = ROOT
) -> TestSourceInventory:
    """Capture each physical source once for all suite and coordinate consumers."""
    selected_suites = tuple(suites)
    selection = _collect_physical_test_selection(selected_suites, repo_root=repo_root)
    return _capture_test_inventory(selection, selected_suites)


def _capture_test_inventory(
    selection: _PhysicalTestSelection,
    suites: tuple[tuple[str | Path, bool], ...],
) -> TestSourceInventory:
    """One byte-capture and generation-fence authority for admitted source paths."""
    sources: list[TestPolicySource] = []
    identities: list[StableRegularFileIdentity] = []
    for path, relative in zip(selection.files, selection.relative_paths, strict=True):
        source, identity = _load_source(path, relative)
        sources.append(source)
        identities.append(identity)
    inventory = TestSourceInventory(
        selection.repo_root,
        suites,
        selection.files,
        tuple(sources),
        selection.suite_members,
        selection.directories,
        tuple(identities),
    )
    inventory.verify_unchanged()
    return inventory


def load_test_sources(
    files: Sequence[Path], *, repo_root: Path = ROOT
) -> tuple[TestPolicySource, ...]:
    """Capture an explicit selection through the shared admission and fences."""
    root = resolve_owned_path(repo_root)
    directories = _DirectoryAdmission()
    directories.ancestry(root)
    directories.admit(root, membership=False)
    selected: dict[str, Path] = {}
    portable_identities: dict[str, str] = {}
    for raw_path in files:
        path = Path(raw_path)
        if not path.is_absolute():
            path = root / path
        directories.ancestry(path)
        path = resolve_owned_path(path)
        _admit_source_path(path, root, selected, portable_identities)
    names = tuple(sorted(selected))
    selection = _PhysicalTestSelection(
        root,
        tuple(selected[name] for name in names),
        names,
        (),
        tuple(directories.generations.values()),
    )
    selection.verify()
    return _capture_test_inventory(selection, ()).sources


@dataclass(frozen=True, slots=True)
class ProjectedTest:
    path: str
    source_sha256: str
    applicable: bool
    exclusion_reason: str | None
    verification_scope: str
    expect_molt_fail: bool
    expected_failure_reason: str | None

    def as_record(self) -> dict[str, object]:
        return {
            "applicable": self.applicable,
            "exclusion_reason": self.exclusion_reason,
            "verification_scope": self.verification_scope,
            "expect_molt_fail": self.expect_molt_fail,
            "expected_failure_reason": self.expected_failure_reason,
            "path": self.path,
            "source_sha256": self.source_sha256,
        }


@dataclass(frozen=True, slots=True)
class CoordinateProjection:
    python: str
    platform: str
    arch: str
    backend: str
    tests: tuple[ProjectedTest, ...]

    @property
    def applicable(self) -> tuple[ProjectedTest, ...]:
        return tuple(test for test in self.tests if test.applicable)

    @property
    def excluded(self) -> tuple[ProjectedTest, ...]:
        return tuple(test for test in self.tests if not test.applicable)

    @property
    def expected_failures(self) -> tuple[ProjectedTest, ...]:
        return tuple(test for test in self.applicable if test.expect_molt_fail)

    def closure_record(self) -> dict[str, object]:
        digest = hashlib.sha256()
        for test in self.tests:
            payload = json.dumps(
                test.as_record(), separators=(",", ":"), sort_keys=True
            ).encode("utf-8")
            digest.update(len(payload).to_bytes(8, "big"))
            digest.update(payload)
        return {
            "applicable": len(self.applicable),
            "excluded": len(self.excluded),
            "expected_failures": len(self.expected_failures),
            "sha256": digest.hexdigest(),
            "source_tests": len(self.tests),
        }


def project_coordinate(
    files: Sequence[Path],
    *,
    python: str,
    platform: str,
    arch: str,
    backend: str,
    repo_root: Path = ROOT,
) -> CoordinateProjection:
    return project_prepared_coordinate(
        load_test_sources(files, repo_root=repo_root),
        python=python,
        platform=platform,
        arch=arch,
        backend=backend,
    )


def project_prepared_coordinate(
    sources: Sequence[TestPolicySource],
    *,
    python: str,
    platform: str,
    arch: str,
    backend: str,
    excluded_verification_scopes: frozenset[str] = frozenset(),
) -> CoordinateProjection:
    try:
        version = parse_version(python)
    except ValueError as exc:
        raise ValueError(f"coordinate Python version is invalid: {python!r}") from exc
    if platform not in PLATFORM_SELECTORS or platform == "posix":
        raise ValueError(f"coordinate platform is invalid: {platform!r}")
    if arch not in ARCHITECTURE_SELECTORS:
        raise ValueError(f"coordinate architecture is invalid: {arch!r}")
    if backend not in ALL_BACKENDS:
        raise ValueError(f"coordinate backend is invalid: {backend!r}")
    unknown_scopes = excluded_verification_scopes.difference(VERIFICATION_SCOPES)
    if unknown_scopes:
        raise ValueError(
            "unknown verified-subset exclusion scopes: "
            + ", ".join(sorted(unknown_scopes))
        )
    if CPYTHON_EQUIVALENCE_SCOPE in excluded_verification_scopes:
        raise ValueError("CPython-equivalence tests cannot be excluded by scope")
    tags = coordinate_platform_tags(platform=platform)
    projected: list[ProjectedTest] = []
    for source in sources:
        reason = exclusion_reason(
            source.metadata,
            python_version=version,
            platform_tags=tags,
            architecture=arch,
            backend=backend,
        )
        metadata = source.metadata
        if (
            reason is None
            and metadata.verification_scope in excluded_verification_scopes
        ):
            reason = f"verified-subset scope exclusion: {metadata.verification_scope}"
        projected.append(
            ProjectedTest(
                path=source.path,
                source_sha256=source.source_sha256,
                applicable=reason is None,
                exclusion_reason=reason,
                verification_scope=metadata.verification_scope,
                expect_molt_fail=metadata.expect_molt_fail,
                expected_failure_reason=metadata.expected_failure_reason,
            )
        )
    projected.sort(key=lambda item: item.path)
    if len(projected) != len({item.path for item in projected}):
        raise ValueError("coordinate projection contains duplicate test identities")
    return CoordinateProjection(
        python=python,
        platform=platform,
        arch=arch,
        backend=backend,
        tests=tuple(projected),
    )


def verification_scope_paths(
    suites: Sequence[tuple[str | Path, bool]],
    *,
    scope: str,
    repo_root: Path = ROOT,
) -> frozenset[str]:
    """Return source identities assigned to one validated verification scope."""

    if scope not in VERIFICATION_SCOPES:
        raise ValueError(f"unknown verified-subset scope: {scope}")
    sources = load_test_inventory(suites, repo_root=repo_root).sources
    return frozenset(
        source.path for source in sources if source.metadata.verification_scope == scope
    )
