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
import sys
import token
import tokenize
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.file_publication import is_link_like
from molt.portable_paths import portable_path_identity, portable_relative_path


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
        if python_version is not None:
            if self.min_python is not None and python_version < self.min_python:
                return f"min_py {self.min_python[0]}.{self.min_python[1]}"
            if self.max_python is not None and python_version > self.max_python:
                return f"max_py {self.max_python[0]}.{self.max_python[1]}"
        return None


def _metadata_comments(file_path: Path) -> tuple[tuple[int, str], ...]:
    """Return actual Python comment tokens that declare ``MOLT_META``."""

    try:
        raw = file_path.read_bytes()
    except OSError as exc:
        raise ValueError(
            f"cannot read differential metadata source {file_path}: {exc}"
        ) from exc
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
    comments = _metadata_comments(path)
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


def collect_physical_test_files(
    suites: Sequence[tuple[str | Path, bool]],
    *,
    repo_root: Path = ROOT,
) -> tuple[Path, ...]:
    """Collect exact physical ``.py`` descendants for typed suite policies.

    Generated lane manifests are scheduling projections, not release-selection
    authorities. This collector therefore inventories the real directory tree,
    rejects every symlink/reparse traversal, and applies one portable identity
    across all suites.
    """

    root = repo_root.resolve(strict=True)
    files: dict[str, Path] = {}
    portable_identities: dict[str, str] = {}

    def add(candidate: Path, *, suite_root: Path) -> None:
        if is_link_like(candidate):
            raise ValueError(f"differential test must not be a link: {candidate}")
        absolute = candidate.absolute()
        resolved = candidate.resolve(strict=True)
        if absolute != resolved:
            raise ValueError(f"differential test must not traverse a link: {candidate}")
        if not resolved.is_relative_to(suite_root):
            raise ValueError(f"differential test escapes its suite: {candidate}")
        if not resolved.is_relative_to(root):
            raise ValueError(f"differential test escapes the repository: {candidate}")
        if not resolved.is_file() or resolved.suffix != ".py":
            raise ValueError(f"differential test is not a Python file: {candidate}")
        identity = portable_relative_path(
            resolved.relative_to(root).as_posix()
        ).as_posix()
        portable_identity = portable_path_identity(identity)
        prior = portable_identities.get(portable_identity)
        if prior is not None:
            if prior == identity:
                raise ValueError(
                    f"differential test is selected by multiple suites: {identity}"
                )
            raise ValueError(
                "differential tests collide on portable filesystems: "
                f"{prior!r}, {identity!r}"
            )
        portable_identities[portable_identity] = identity
        files[identity] = resolved

    for raw_suite, recursive in suites:
        if not isinstance(recursive, bool):
            raise ValueError("differential suite recursive policy must be boolean")
        candidate = Path(raw_suite)
        if not candidate.is_absolute():
            candidate = root / candidate
        if is_link_like(candidate):
            raise ValueError(f"differential suite must not be a link: {raw_suite}")
        absolute = candidate.absolute()
        suite_root = candidate.resolve(strict=True)
        if absolute != suite_root:
            raise ValueError(
                f"differential suite must not traverse a link: {raw_suite}"
            )
        if not suite_root.is_relative_to(root) or not suite_root.is_dir():
            raise ValueError(
                f"differential suite is not a repository directory: {raw_suite}"
            )

        pending = [suite_root]
        while pending:
            directory = pending.pop()
            if is_link_like(directory):
                raise ValueError(
                    f"differential suite contains a linked directory: {directory}"
                )
            with os.scandir(directory) as entries:
                ordered = sorted(entries, key=lambda entry: entry.name)
            for entry in ordered:
                path = Path(entry.path)
                if is_link_like(path):
                    raise ValueError(
                        f"differential suite contains a link or reparse point: {path}"
                    )
                if entry.is_dir(follow_symlinks=False):
                    if recursive:
                        pending.append(path)
                    continue
                if not entry.is_file(follow_symlinks=False):
                    raise ValueError(
                        f"differential suite contains a special entry: {path}"
                    )
                if path.suffix == ".py":
                    add(path, suite_root=suite_root)

    return tuple(files[name] for name in sorted(files))


@dataclass(frozen=True, slots=True)
class TestPolicySource:
    path: str
    source_sha256: str
    metadata: TestMetadata


def load_test_sources(
    files: Sequence[Path], *, repo_root: Path = ROOT
) -> tuple[TestPolicySource, ...]:
    sources: list[TestPolicySource] = []
    for path in files:
        metadata = parse_metadata(path)
        if (
            metadata.verification_scope != CPYTHON_EQUIVALENCE_SCOPE
            and not metadata.expect_molt_fail
        ):
            raise ValueError(
                f"verified-subset scope exclusion must remain an explicit "
                f"expected divergence: {normalize_repo_relative(path, repo_root=repo_root)}"
            )
        sources.append(
            TestPolicySource(
                path=normalize_repo_relative(path, repo_root=repo_root),
                source_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                metadata=metadata,
            )
        )
    sources.sort(key=lambda item: item.path)
    if len(sources) != len({item.path for item in sources}):
        raise ValueError("test-policy sources contain duplicate identities")
    return tuple(sources)


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
    sources = load_test_sources(
        collect_physical_test_files(suites, repo_root=repo_root), repo_root=repo_root
    )
    return frozenset(
        source.path for source in sources if source.metadata.verification_scope == scope
    )
