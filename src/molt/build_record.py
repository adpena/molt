"""Early cutoff for unchanged builds (docs/design/build_early_cutoff.md).

Each input authority reports what it reads through `molt.input_report` when it
reads it; an audit hook observes the reads no authority reports. This module
owns the ledger those facts reach, the request key and the record. A successful
build whose ledger holds no refusal and no racy input records those facts with
the token of the binary it published. The next build of the same request
re-checks metadata inputs by stat and consumed source bytes by checksum: when
every input matches and the binary is untouched, the build is complete. A
changed metadata token with a recorded digest falls back to a digest compare,
so a `touch` costs one hash. Anything unproven means a full build.
"""

from __future__ import annotations

import hashlib
import json
import os
import stat
import threading
import time
from collections.abc import Callable, Generator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any, assert_never

from molt import file_publication, input_report
from molt.durable_file_identity import (
    DurableFileIdentity,
    file_identity,
)
from molt.build_output_selection import BuildOutputSelection
from molt.file_hashing import DIGEST_RACY_WINDOW_NS
from molt.input_report import (
    FileEvidence,
    ProbeOutcome,
    ReadReceiptId,
    StatToken,
    resolved_probe,
    stat_token,
)
from enum import StrEnum

_SCHEMA = "molt-build-record-v11"
# Keys that name the terminal or login session. No build step reads them for
# its artifact (`TERM` only styles progress output); they change between
# shells and would defeat every hit. Everything else is keyed, `PWD` included:
# compilers may take the debug-info directory from it.
_SESSION_ENVIRONMENT_KEYS = frozenset(
    {
        "_",
        "COLORTERM",
        "COLUMNS",
        "ITERM_PROFILE",
        "ITERM_SESSION_ID",
        "LINES",
        "OLDPWD",
        # pytest's temporary root, which the memory guard rebinds to each
        # guarded child's scratch lease; only pytest reads it.
        "PYTEST_DEBUG_TEMPROOT",
        "SECURITYSESSIONID",
        "SHLVL",
        "SSH_AGENT_PID",
        "SSH_AUTH_SOCK",
        "TERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "TMUX",
        "TMUX_PANE",
        "__CFBundleIdentifier",
        "__CF_USER_TEXT_ENCODING",
    }
)
# Records unused this long are trimmed; a hit refreshes its record's mtime at
# most once per interval, and trimming runs at most once per interval.
_RECORD_UNUSED_LIMIT_NS = 7 * 24 * 3600 * 1_000_000_000
_RECORD_REFRESH_INTERVAL_NS = 3600 * 1_000_000_000
_TRIM_MARKER = ".trimmed"
# Each recorded build's own transient files, beside the records it writes.
_TRANSIENT_DIRNAME = ".transient"


def _file_sha256(path: Path | str) -> str | None:
    from molt.toolchain_identity import stable_file_sha256

    try:
        return stable_file_sha256(Path(path), label="build record input")
    except (OSError, ValueError):
        return None


class FileValidation(StrEnum):
    METADATA = "metadata"
    CHECKSUM = "checksum"


if TYPE_CHECKING:
    from molt.backend_protocol import BackendEmissionOrigin
    from molt.toolchain_identity import FileDigestCache, StableRegularFileIdentity


def _record_path(value: object) -> Path:
    if not isinstance(value, str) or not value or "\0" in value:
        raise ValueError("invalid recorded path")
    path = Path(value)
    if not path.is_absolute():
        raise ValueError("recorded path is not absolute")
    return path


def _record_digest(value: object) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(char not in "0123456789abcdef" for char in value)
    ):
        raise ValueError("invalid recorded content digest")
    return value


class BuildClaimKind(StrEnum):
    ARTIFACTS = "artifacts"
    CHECKOUT = "checkout"
    PRODUCT = "product"


@dataclass(frozen=True, slots=True)
class RecordedBuildClaim:
    kind: BuildClaimKind
    root: Path
    resolved_root: Path
    directory_identity: DurableFileIdentity
    cargo_target: Path | None = None
    product_identity: str | None = None

    @classmethod
    def capture(
        cls,
        kind: BuildClaimKind,
        root: Path,
        *,
        cargo_target: Path | None = None,
        product_identity: str | None = None,
    ) -> RecordedBuildClaim:
        root = root.absolute()
        resolved = root.resolve(strict=True)
        metadata = resolved.stat()
        if not stat.S_ISDIR(metadata.st_mode):
            raise ValueError(f"build claim is not a directory: {root}")
        return cls(
            kind,
            root,
            resolved,
            file_identity(resolved, expected=metadata),
            None if cargo_target is None else cargo_target.absolute(),
            product_identity,
        )

    def verify(self) -> None:
        if (
            RecordedBuildClaim.capture(
                self.kind,
                self.root,
                cargo_target=self.cargo_target,
                product_identity=self.product_identity,
            )
            != self
        ):
            raise ValueError(f"build claim root changed: {self.root}")

    def payload(self) -> dict[str, object]:
        return {
            "kind": self.kind.value,
            "root": str(self.root),
            "resolved_root": str(self.resolved_root),
            "directory_identity": self.directory_identity.payload(),
            "cargo_target": None
            if self.cargo_target is None
            else str(self.cargo_target),
            "product_identity": self.product_identity,
        }

    @classmethod
    def from_payload(cls, value: object) -> RecordedBuildClaim:
        if not isinstance(value, dict) or set(value) != {
            "kind",
            "root",
            "resolved_root",
            "directory_identity",
            "cargo_target",
            "product_identity",
        }:
            raise ValueError("invalid build claim")
        kind = BuildClaimKind(value["kind"])
        identity = DurableFileIdentity.from_payload(value["directory_identity"])
        cargo_target = (
            None
            if value["cargo_target"] is None
            else _record_path(value["cargo_target"])
        )
        product = value["product_identity"]
        if kind is BuildClaimKind.PRODUCT:
            product = _record_digest(product)
        elif product is not None:
            raise ValueError("non-product claim has a product identity")
        if kind is not BuildClaimKind.CHECKOUT and cargo_target is not None:
            raise ValueError("non-checkout claim has a Cargo target")
        return cls(
            kind,
            _record_path(value["root"]),
            _record_path(value["resolved_root"]),
            identity,
            cargo_target,
            product,
        )


@dataclass(frozen=True, slots=True)
class RecordedArtifact:
    path: Path
    resolved_path: Path
    token: StatToken
    sha256: str

    @classmethod
    def capture(cls, path: Path) -> RecordedArtifact:
        from molt.toolchain_identity import stable_regular_file_identity

        path = path.absolute()
        return cls.from_identity(
            path,
            stable_regular_file_identity(
                path.resolve(strict=True), label="build result artifact"
            ),
        )

    @classmethod
    def from_identity(
        cls,
        path: Path,
        identity: StableRegularFileIdentity,
    ) -> RecordedArtifact:
        path = path.absolute()
        resolved = path.resolve(strict=True)
        if resolved != identity.path:
            raise ValueError(
                f"build result artifact identity names another path: {path}"
            )
        result = cls(path, resolved, identity.stat_token(), identity.sha256)
        # The supplied capture already has its digest. This fence binds its
        # coordinate; detached reuse below independently revalidates content.
        result.verify_path()
        return result

    def verify_path(self) -> None:
        if (
            self.path.resolve(strict=True) != self.resolved_path
            or stat_token(self.path) != self.token
        ):
            raise ValueError(f"build result artifact changed: {self.path}")

    def verify(self, *, digests: FileDigestCache | None = None) -> None:
        from molt.toolchain_facts import host_file_digest_cache
        import sqlite3

        self.verify_path()
        owned_cache = digests is None
        cache = host_file_digest_cache() if digests is None else digests
        try:
            # Existing host policy hashes Windows and racy POSIX generations;
            # only settled, unchanged POSIX bytes can reuse a recorded digest.
            current = cache.identity(self.resolved_path, label="build result artifact")
            if current.size != self.token[3] or current.sha256 != self.sha256:
                raise ValueError(f"build result artifact content changed: {self.path}")
            self.verify_path()
            if owned_cache:
                cache.flush()
        except sqlite3.Error as exc:
            raise ValueError(f"build result digest cache unavailable: {exc}") from exc

    def payload(self) -> dict[str, object]:
        return {
            "path": str(self.path),
            "resolved_path": str(self.resolved_path),
            "token": list(self.token),
            "sha256": self.sha256,
        }

    @classmethod
    def from_payload(cls, value: object) -> RecordedArtifact:
        if not isinstance(value, dict) or set(value) != {
            "path",
            "resolved_path",
            "token",
            "sha256",
        }:
            raise ValueError("invalid build result artifact")
        token = value["token"]
        if (
            not isinstance(token, list)
            or len(token) != 6
            or any(type(item) is not int for item in token[2:])
        ):
            raise ValueError("invalid build result artifact token")
        DurableFileIdentity.from_stat_prefix(token[0], token[1])
        if not stat.S_ISREG(token[2]) or token[3] < 0:
            raise ValueError("build result artifact is not a regular file")
        return cls(
            _record_path(value["path"]),
            _record_path(value["resolved_path"]),
            (token[0], token[1], token[2], token[3], token[4], token[5]),
            _record_digest(value["sha256"]),
        )


@dataclass(frozen=True, slots=True)
class NativeBuildFacts:
    target_triple: str | None
    entry: str
    deterministic: bool
    trusted: bool
    capabilities: tuple[str, ...]
    capability_profiles: tuple[str, ...]
    capability_policy_digest: str
    capabilities_source: str | None
    profile: str
    native_arch_perf: bool
    cpu_baseline: str
    cranelift_flags: str

    def payload(self) -> dict[str, object]:
        from dataclasses import asdict

        result = asdict(self)
        result["capabilities"] = list(self.capabilities)
        result["capability_profiles"] = list(self.capability_profiles)
        return result

    @classmethod
    def from_payload(cls, value: object) -> NativeBuildFacts:
        from dataclasses import fields

        if not isinstance(value, dict) or set(value) != {
            item.name for item in fields(cls)
        }:
            raise ValueError("invalid native build facts")
        values = dict(value)
        for key in ("target_triple", "capabilities_source"):
            if values[key] is not None and not isinstance(values[key], str):
                raise ValueError(f"invalid native build fact: {key}")
        for key in (
            "entry",
            "capability_policy_digest",
            "profile",
            "cpu_baseline",
            "cranelift_flags",
        ):
            if not isinstance(values[key], str):
                raise ValueError(f"invalid native build fact: {key}")
        for key in ("deterministic", "trusted", "native_arch_perf"):
            if type(values[key]) is not bool:
                raise ValueError(f"invalid native build fact: {key}")
        for key in ("capabilities", "capability_profiles"):
            if not isinstance(values[key], list) or any(
                not isinstance(item, str) for item in values[key]
            ):
                raise ValueError(f"invalid native build fact: {key}")
            values[key] = tuple(values[key])
        algorithm, separator, digest = values["capability_policy_digest"].partition(":")
        if algorithm != "sha256" or separator != ":":
            raise ValueError("invalid native capability policy digest")
        _record_digest(digest)
        return cls(**values)


@dataclass(frozen=True, slots=True)
class NativeBuildResult:
    facts: NativeBuildFacts
    binary: RecordedArtifact
    archive: RecordedArtifact
    stub: RecordedArtifact
    runtime: RecordedArtifact
    compiler: RecordedArtifact
    selected_profiles: tuple[tuple[str, str], ...]
    emission_origin: BackendEmissionOrigin | None

    def verify_paths(self) -> None:
        for artifact in (
            self.binary,
            self.archive,
            self.stub,
            self.runtime,
            self.compiler,
        ):
            artifact.verify_path()

    def verify(self) -> None:
        from molt.toolchain_facts import host_file_digest_cache
        import sqlite3

        digests = host_file_digest_cache()
        for artifact in (
            self.binary,
            self.archive,
            self.stub,
            self.runtime,
            self.compiler,
        ):
            artifact.verify(digests=digests)
        try:
            digests.flush()
        except sqlite3.Error as exc:
            raise ValueError(f"build result digest cache unavailable: {exc}") from exc
        if self.emission_origin is not None and (
            self.emission_origin.output_size != self.archive.token[3]
            or self.emission_origin.output_sha256 != self.archive.sha256
        ):
            raise ValueError("build result archive differs from original emission")

    def verify_claims(self, claims: list[RecordedBuildClaim]) -> None:
        if not any(
            claim.kind is BuildClaimKind.ARTIFACTS
            and self.archive.resolved_path.is_relative_to(claim.resolved_root)
            and self.stub.resolved_path.is_relative_to(claim.resolved_root)
            for claim in claims
        ):
            raise ValueError("native JSON result lacks its producer artifacts claim")

    def payload(self) -> dict[str, object]:
        return {
            "facts": self.facts.payload(),
            **{
                name: getattr(self, name).payload()
                for name in ("binary", "archive", "stub", "runtime", "compiler")
            },
            "selected_profiles": dict(self.selected_profiles),
            "emission_origin": None
            if self.emission_origin is None
            else self.emission_origin.payload(),
        }

    @classmethod
    def from_payload(cls, value: object) -> NativeBuildResult:
        from molt.backend_protocol import BackendEmissionOrigin

        if not isinstance(value, dict) or set(value) != {
            "facts",
            "binary",
            "archive",
            "stub",
            "runtime",
            "compiler",
            "selected_profiles",
            "emission_origin",
        }:
            raise ValueError("invalid native build result")
        profiles = value["selected_profiles"]
        if not isinstance(profiles, dict) or any(
            not isinstance(key, str) or not isinstance(item, str)
            for key, item in profiles.items()
        ):
            raise ValueError("invalid native build profiles")
        return cls(
            facts=NativeBuildFacts.from_payload(value["facts"]),
            binary=RecordedArtifact.from_payload(value["binary"]),
            archive=RecordedArtifact.from_payload(value["archive"]),
            stub=RecordedArtifact.from_payload(value["stub"]),
            runtime=RecordedArtifact.from_payload(value["runtime"]),
            compiler=RecordedArtifact.from_payload(value["compiler"]),
            selected_profiles=tuple(sorted(profiles.items())),
            emission_origin=None
            if value["emission_origin"] is None
            else BackendEmissionOrigin.from_payload(value["emission_origin"]),
        )


@dataclass(frozen=True, slots=True)
class BuildProducts:
    """The files a successful build published beside its selected output.

    ``products`` maps each result artifact role to the absolute path the
    result promises; ``consumer`` names the product a user runs. A WASM build
    publishes a family (module, linked module, deployment manifest, optimizer
    attestation); its record is current only while every member keeps the
    token the build left. A native binary build publishes only its selected
    output and reports no family.
    """

    consumer: Path
    products: tuple[tuple[str, Path], ...]

    @classmethod
    def from_payload(cls, value: object) -> BuildProducts:
        if not isinstance(value, dict) or set(value) != {"consumer", "products"}:
            raise ValueError("invalid build products")
        raw = value["products"]
        if not isinstance(raw, dict) or not raw:
            raise ValueError("build products must name at least one file")
        products = []
        for role, path in sorted(raw.items()):
            if not isinstance(role, str) or not role:
                raise ValueError("invalid build product role")
            products.append((role, _record_path(path)))
        consumer = _record_path(value["consumer"])
        if consumer not in {path for _role, path in products}:
            raise ValueError("build consumer is not one of its products")
        return cls(consumer, tuple(products))

    def paths(self) -> tuple[Path, ...]:
        return tuple(path for _role, path in self.products)


@dataclass
class FileObservation:
    token: StatToken
    sha256: str | None
    evidence: FileEvidence


@dataclass
class InputLedger:
    """What one build read, reported by each authority when it read it."""

    # Weak inputs may not change at or after process birth minus the racy
    # window. Consumed source buffers instead retain checksum validation.
    started_ns: int
    files: dict[str, FileObservation] = field(default_factory=dict)
    # path -> token; adding, removing or renaming an entry changes it
    directories: dict[str, StatToken] = field(default_factory=dict)
    probes: dict[str, ProbeOutcome] = field(default_factory=dict)
    refusals: list[str] = field(default_factory=list)
    # ("file" | "directory", absolute path) seen by the audit
    # hook; resolved against the excluded roots when the record is written.
    observed: set[tuple[str, str]] = field(default_factory=set)
    output_selection: BuildOutputSelection | None = None
    products: BuildProducts | None = None
    result: NativeBuildResult | None = None
    claims: list[RecordedBuildClaim] = field(default_factory=list)
    _lock: threading.Lock = field(default_factory=threading.Lock, repr=False)
    _reads: dict[ReadReceiptId, str | None] = field(default_factory=dict, repr=False)

    def _file_locked(
        self,
        name: str,
        token: StatToken | None,
        sha256: str | None,
        evidence: FileEvidence,
    ) -> None:
        if token is None:
            self.refusals.append(f"unreportable file {name}")
            return
        prior = self.files.get(name)
        if ("file", name) in self.observed:
            evidence |= FileEvidence.UNBOUND
        if prior is None:
            self.files[name] = FileObservation(token, sha256, evidence)
            return
        if prior.token != token:
            self.refusals.append(f"file changed during the build: {name}")
        if prior.sha256 is not None and sha256 is not None and prior.sha256 != sha256:
            self.refusals.append(f"file bytes changed during the build: {name}")
        prior.evidence |= evidence
        if prior.sha256 is None:
            prior.sha256 = sha256

    def file(
        self, path: Path | str, token: StatToken | None, sha256: str | None
    ) -> None:
        name = os.fspath(path)
        with self._lock:
            self._file_locked(
                name,
                token,
                sha256,
                FileEvidence.CUSTODY if sha256 is not None else FileEvidence.UNBOUND,
            )

    def consumed_file(
        self, path: Path | str, token: StatToken | None, sha256: str
    ) -> None:
        with self._lock:
            self._file_locked(os.fspath(path), token, sha256, FileEvidence.CONSUMED)

    def read_started(self, receipt: ReadReceiptId, path: Path | str) -> None:
        path = os.fspath(path)
        with self._lock:
            if receipt in self._reads:
                self.refusals.append(f"duplicate captured read receipt: {path}")
            else:
                self._reads[receipt] = path

    def read_finished(
        self,
        receipt: ReadReceiptId,
        path: Path | str,
        token: StatToken | None,
        sha256: str | None,
    ) -> None:
        path = os.fspath(path)
        with self._lock:
            if self._reads.get(receipt) != path:
                self.refusals.append(f"unmatched captured read receipt: {path}")
                return
            self._reads[receipt] = None
            if token is None or sha256 is None:
                self.observed.add(("file", path))
                if path in self.files:
                    self.files[path].evidence |= FileEvidence.UNBOUND
                return
            self._file_locked(path, token, sha256, FileEvidence.CONSUMED)

    def pending_reads(self) -> bool:
        with self._lock:
            return any(path is not None for path in self._reads.values())

    def directory(self, path: Path | str, token: StatToken | None) -> None:
        name = os.fspath(path)
        with self._lock:
            if token is None:
                self.refusals.append(f"unreportable directory {name}")
                return
            prior = self.directories.get(name)
            if prior is not None and prior != token:
                self.refusals.append(f"directory changed during the build: {name}")
            self.directories[name] = token

    def probe(self, path: Path | str, outcome: ProbeOutcome) -> None:
        name = os.fspath(path)
        with self._lock:
            if name in self.probes and self.probes[name] != outcome:
                self.refusals.append(f"lookup changed during the build: {name}")
            self.probes[name] = outcome

    def refuse(self, reason: str) -> None:
        with self._lock:
            self.refusals.append(reason)

    def observe(self, entry: tuple[str, str]) -> None:
        with self._lock:
            self.observed.add(entry)
            if entry[0] == "file" and entry[1] in self.files:
                self.files[entry[1]].evidence |= FileEvidence.UNBOUND

    def build_output(self, selection: object) -> None:
        try:
            selected = BuildOutputSelection.from_dict(selection)
        except (TypeError, ValueError) as exc:
            self.refuse(f"unreportable build output: {exc}")
            return
        with self._lock:
            if self.output_selection is not None and self.output_selection != selected:
                self.refusals.append("build selected more than one native output")
            self.output_selection = selected

    def build_products(self, value: object) -> None:
        try:
            products = BuildProducts.from_payload(value)
        except (TypeError, ValueError) as exc:
            self.refuse(f"unreportable build products: {exc}")
            return
        with self._lock:
            if self.products is not None and self.products != products:
                self.refusals.append("build published conflicting products")
            self.products = products

    def build_claim(self, value: object) -> None:
        try:
            claim = RecordedBuildClaim.from_payload(value)
        except (TypeError, ValueError) as exc:
            self.refuse(f"unreportable build claim: {exc}")
            return
        with self._lock:
            for prior in self.claims:
                if (prior.kind, prior.root, prior.product_identity) == (
                    claim.kind,
                    claim.root,
                    claim.product_identity,
                ):
                    if prior != claim:
                        self.refusals.append("build claim changed during build")
                    return
            self.claims.append(claim)

    def build_result(self, value: object) -> None:
        try:
            result = NativeBuildResult.from_payload(value)
            result.verify()
        except (OSError, TypeError, ValueError) as exc:
            self.refuse(f"unreportable build result: {exc}")
            return
        with self._lock:
            if self.result is not None and self.result != result:
                self.refusals.append("build reported conflicting results")
            self.result = result

    def racy_inputs(self) -> list[str]:
        """Inputs whose content changed within the racy window of the
        recording process's start: their token cannot prove what it read."""
        limit = self.started_ns - DIGEST_RACY_WINDOW_NS
        racy = [
            name
            for name, fact in self.files.items()
            if fact.token[5] >= limit
            and (fact.evidence != FileEvidence.CONSUMED or fact.sha256 is None)
        ]
        racy.extend(
            name for name, token in self.directories.items() if token[5] >= limit
        )
        return racy


_BUILDS_LOCK = threading.Lock()
_BUILDS_ENTERED = 0


def enter_build() -> bool:
    """Count one build in this process; True for the first.

    Only the first build of a process may record: process-global caches can
    serve a later build without a read to report, whether or not the earlier
    build was eligible for a record.
    """
    global _BUILDS_ENTERED
    with _BUILDS_LOCK:
        _BUILDS_ENTERED += 1
        return _BUILDS_ENTERED == 1


def transient_root(state_root: Path) -> Path:
    """Where recorded builds keep their transient files: a product root."""
    return state_root / "build_records" / "v11" / _TRANSIENT_DIRNAME


@contextmanager
def recording(
    transient_parent: Path, *, capture_result: bool = False
) -> Generator[InputLedger]:
    """Activate one ledger for the first build of this process.

    The racy bound is the process's start: modules, caches and worker state may
    hold bytes read before the build began.

    The build's temporary files and its workers' read logs live in one
    directory under ``transient_parent``, as Bazel gives each action a
    temporary directory in its output base. A file the build creates and
    removes in an input directory, such as the system temporary directory,
    changes that directory during the build and refuses the record. Normal
    scope exit removes only this allocation, fenced by its directory identity.
    An interrupted build can leave its allocation behind; without process-tree
    closure evidence, neither its age nor a missing parent authorizes deletion.
    """
    import tempfile

    from molt.process_birth import current_process_started_ns
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    started = current_process_started_ns()
    ledger = InputLedger(started if started is not None else time.time_ns())
    if started is None:
        ledger.refuse("process start time unavailable")
    transient_parent.mkdir(parents=True, exist_ok=True)
    with OwnedTemporaryDirectory(prefix="build-", dir=transient_parent) as transient:
        log_dir = os.path.join(transient, "reads")
        temporary = os.path.join(transient, "temporary")
        os.mkdir(log_dir)
        os.mkdir(temporary)
        previous_temporary = tempfile.tempdir
        tempfile.tempdir = temporary
        input_report.install_sink(ledger, log_dir, capture_result=capture_result)
        try:
            yield ledger
        finally:
            input_report.remove_sink()
            tempfile.tempdir = previous_temporary
            input_report.merge_worker_logs(ledger, log_dir)


def request_key(
    arguments: Mapping[str, Any],
    *,
    environment: Mapping[str, str],
    cwd: str,
    interpreter: str,
    interpreter_version: str,
    host: Mapping[str, str],
) -> str | None:
    """Identity of one build request, or None when it is not plain data.

    The environment is keyed whole except terminal-session names and the
    registry's output-neutral names: a guarded build carries a fresh guard
    token, marker and scratch lease, and memory budgets measured at launch.
    """
    from molt.environment_registry import load_registry

    # The registry loads only when a MOLT_* name is set, as at CLI start.
    registry = (
        load_registry() if any(key.startswith("MOLT_") for key in environment) else None
    )
    material = {
        "schema": "molt-build-record-v10",
        "arguments": dict(arguments),
        "cwd": cwd,
        "interpreter": interpreter,
        "interpreter_version": interpreter_version,
        "host": dict(host),
        "environment": {
            key: value
            for key, value in environment.items()
            if key not in _SESSION_ENVIRONMENT_KEYS
            and not (
                registry is not None
                and key.startswith("MOLT_")
                and registry.is_output_neutral(key)
            )
        },
    }
    try:
        encoded = json.dumps(
            material, sort_keys=True, separators=(",", ":"), allow_nan=False
        )
    except (TypeError, ValueError):
        return None
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def record_path(state_root: Path, key: str) -> Path:
    return state_root / "build_records" / "v11" / key[:2] / f"{key}.json"


def _publication_custody(paths: set[str], outputs: tuple[Path, ...]) -> set[str]:
    """The resolved observed paths the outputs' publications own: never inputs.

    An output's family (the output, its receipt, lock, journals and stages)
    is custody. A publication can also stage a stage, as the WASM backend
    output's copy does, or stage a whole directory, as the WASM deployment
    generation does beside its manifest. A path that is an owned stage of a
    custody path, or lies inside an owned stage directory, joins the custody
    until nothing changes. Only reserved staging names whose key names a
    custody path in the same directory qualify, by the staging-name
    authority; nothing is opened. Stages live beside what they stage, so only
    paths under an output's directory are examined, and each walk stops there.
    """
    from molt.artifact_publication import is_publication_family_member
    from molt.file_publication import (
        is_staged_file_path,
        staged_file_destination_key,
        staging_destination_key,
    )
    from molt.path_custody import resolved_host_path_key

    def owner_key(path: Path) -> tuple[str, str]:
        return resolved_host_path_key(path.parent), staging_destination_key(path)

    roots = {resolved_host_path_key(output.parent) for output in outputs}
    prefixes = tuple(root.rstrip(os.sep) + os.sep for root in roots)

    def nodes_below_root(path: str) -> list[Path]:
        nodes: list[Path] = []
        for node in (Path(path), *Path(path).parents):
            if resolved_host_path_key(node) in roots:
                return nodes
            nodes.append(node)
        return []

    candidates = {
        path: nodes
        for path in sorted(paths)
        if resolved_host_path_key(path).startswith(prefixes)
        and (nodes := nodes_below_root(path))
    }
    custody = {
        path
        for path in candidates
        if any(is_publication_family_member(Path(path), output) for output in outputs)
    }
    owners = {owner_key(path) for path in (*outputs, *map(Path, custody))}
    pending = [
        (path, nodes) for path, nodes in candidates.items() if path not in custody
    ]
    changed = True
    while changed:
        changed = False
        remaining = []
        for path, nodes in pending:
            if any(
                is_staged_file_path(node)
                and (
                    resolved_host_path_key(node.parent),
                    staged_file_destination_key(node),
                )
                in owners
                for node in nodes
            ):
                custody.add(path)
                owners.add(owner_key(Path(path)))
                changed = True
            else:
                remaining.append((path, nodes))
        pending = remaining
    return custody


def resolve_observed(
    ledger: InputLedger,
    excluded_roots: tuple[Path, ...],
    *,
    published_outputs: tuple[Path, ...] = (),
) -> None:
    """Turn the audit hook's observations into inputs.

    Only named product roots and the successfully published outputs (the
    selected output and the product family published beside it), each with
    the receipt, journals, lock and stages its publication keeps beside it,
    are products. The publication lists each output's directory for its own
    journals, so an observed listing of that directory is custody; a reader
    that consumes the listing, such as the import topology, reports it
    explicitly. Pre-syscall audit events cannot prove write or
    directory-creation ownership. Explicit input reports are never excluded,
    even for a published output.
    """
    from molt.path_custody import resolved_host_path_key

    outputs = tuple(Path(os.path.realpath(path)) for path in published_outputs)
    output_parents = {resolved_host_path_key(output.parent) for output in outputs}
    observed = sorted(ledger.observed)
    resolved_paths = {name: os.path.realpath(name) for _kind, name in observed}
    custody = _publication_custody(set(resolved_paths.values()), outputs)
    roots = tuple(
        {
            resolved_host_path_key(os.path.realpath(root)).rstrip(os.sep) + os.sep
            for root in excluded_roots
        }
    )
    # Only this resolution's completed chains prove that a failed lookup has
    # a generation boundary. Explicit probes may not have recorded one.
    completed_lookups: set[str] = set()

    def failed_lookup(name: str) -> None:
        chain: list[str] = []
        while name not in completed_lookups:
            outcome = resolved_probe(name)
            ledger.probe(name, outcome)
            chain.append(name)
            if outcome is not None and outcome[0]:
                # The path exists but its content-change token is unavailable.
                # A parent cannot substitute for that missing generation.
                ledger.refuse(f"unreportable lookup generation {name}")
                break
            parent = os.path.dirname(name)
            if parent == name:
                ledger.refuse(f"unreportable lookup boundary {name}")
                break
            if parent in completed_lookups:
                break
            token = stat_token(parent)
            if token is not None:
                if stat.S_ISDIR(token[2]):
                    if parent not in ledger.directories:
                        ledger.directory(parent, token)
                elif parent not in ledger.files:
                    # ENOTDIR can terminate at a file. Keep its typed token;
                    # it is not a directory listing dependency.
                    ledger.file(parent, token, None)
                chain.append(parent)
                break
            name = parent
        completed_lookups.update(chain)

    for kind, name in observed:
        resolved = resolved_paths[name]
        real = resolved_host_path_key(resolved)
        if resolved in custody or (kind == "directory" and real in output_parents):
            continue
        if (real + os.sep).startswith(roots):
            continue
        if real.startswith(("/dev/", "/proc/", "/sys/")):
            continue
        token = stat_token(name)
        if token is None:
            if name in ledger.files:
                # A reported file keeps its original generation; validation
                # already refuses its removal or an unreadable replacement.
                ledger.probe(name, resolved_probe(name))
            else:
                # Missing intermediate components are probes too. The nearest
                # stat-able ancestor supplies the racy-removal boundary.
                failed_lookup(name)
            continue
        if kind == "directory":
            if name not in ledger.directories:
                ledger.directory(name, token)
        elif stat.S_ISREG(token[2]):
            ledger.file(name, token, None)


def _first_change(
    files: list[list[Any]],
    directories: list[list[Any]],
    probes: list[list[Any]],
) -> str | None:
    """The first recorded input that no longer matches, or None."""
    for name, token in directories:
        if stat_token(name) != tuple(token):
            return f"directory {name}"
    for name, outcome in probes:
        expected = None if outcome is None else (outcome[0], outcome[1])
        if resolved_probe(name) != expected:
            return f"lookup {name}"
    for name, token, sha256, mode in files:
        validation = FileValidation(mode)
        current = stat_token(name)
        if validation is FileValidation.CHECKSUM:
            if (
                not isinstance(sha256, str)
                or len(sha256) != 64
                or any(c not in "0123456789abcdef" for c in sha256)
            ):
                raise ValueError(f"invalid checksum input {name}")
            if (
                current is None
                or current[3] != token[3]
                or _file_sha256(name) != sha256
            ):
                return f"file {name}"
            continue
        if validation is not FileValidation.METADATA:
            assert_never(validation)
        if current == tuple(token):
            continue
        # A rewritten file with equal bytes (a touch, a checkout) is unchanged
        # when its content digest was recorded.
        if (
            sha256 is None
            or current is None
            or current[3] != token[3]
            or _file_sha256(name) != sha256
        ):
            return f"file {name}"
    return None


def write_record(
    path: Path, key: str, ledger: InputLedger, *, output: BuildOutputSelection
) -> str | None:
    """Publish the record of a build that produced ``output``.

    Returns why no record was written: a ledger refusal, a racy input, an
    input that no longer matches what the build read, or a failed write.
    """
    if ledger.refusals:
        return ledger.refusals[0]
    if ledger.pending_reads():
        return "pending captured read receipt"
    racy = ledger.racy_inputs()
    if racy:
        return f"input changed after the recording process started: {racy[0]}"
    files = [
        [
            name,
            list(fact.token),
            fact.sha256,
            FileValidation.CHECKSUM
            if fact.evidence & FileEvidence.CONSUMED
            else FileValidation.METADATA,
        ]
        for name, fact in sorted(ledger.files.items())
    ]
    directories = [
        [name, list(token)] for name, token in sorted(ledger.directories.items())
    ]
    probes = [
        [name, None if outcome is None else list(outcome)]
        for name, outcome in sorted(ledger.probes.items())
    ]
    changed = _first_change(files, directories, probes)
    if changed is not None:
        return f"input changed during the build: {changed}"
    try:
        output.verify()
        if ledger.result is not None:
            ledger.result.verify()
            ledger.result.verify_claims(ledger.claims)
            if ledger.result.binary.path != output.path:
                return "build result differs from output selection"
            for claim in ledger.claims:
                claim.verify()
    except (OSError, ValueError) as exc:
        return str(exc)
    output_token = stat_token(output.path)
    if output_token is None or not stat.S_ISREG(output_token[2]):
        return f"output {output.path} is not a regular file"
    products: list[list[object]] = []
    consumer = output.path
    if ledger.products is not None:
        if output.path not in ledger.products.paths():
            return "selected output is not one of the published products"
        consumer = ledger.products.consumer
        for role, product in ledger.products.products:
            token = stat_token(product)
            if token is None or not stat.S_ISREG(token[2]):
                return f"product {product} is not a regular file"
            products.append([role, os.fspath(product), list(token)])
    payload = {
        "schema": _SCHEMA,
        "key": key,
        "output": {
            "selection": output.to_dict(),
            "token": list(output_token),
            "consumer": os.fspath(consumer),
            "products": products,
        },
        "files": files,
        "directories": directories,
        "probes": probes,
        "result": None if ledger.result is None else ledger.result.payload(),
        "claims": [claim.payload() for claim in ledger.claims]
        if ledger.result is not None
        else [],
    }
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        file_publication.atomic_write_bytes(
            path,
            json.dumps(payload, separators=(",", ":")).encode("utf-8"),
            durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
        )
    except OSError as error:
        return f"record write failed: {error}"
    trim_if_due(path.parent.parent)
    return None


@dataclass(frozen=True, slots=True)
class BuildRecordHit:
    output: Path
    result: NativeBuildResult | None = None
    # The product a user runs: the selected output, or a member of its family.
    consumer: Path | None = None


def check(
    path: Path,
    key: str,
    *,
    claim: Callable[[RecordedBuildClaim], None] | None = None,
    require_result: bool = False,
) -> BuildRecordHit | str:
    """The verified output when current, otherwise the first difference.

    Current means every recorded input matches and ``output`` still holds the
    binary that build published. Never raises.
    """
    try:
        metadata = os.stat(path)
        with open(path, "rb") as handle:
            payload = json.loads(handle.read())
        if not isinstance(payload, dict):
            return "unreadable record: expected a JSON object"
        if payload.get("schema") != _SCHEMA or payload.get("key") != key:
            return "no record for this request"
        result = (
            None
            if payload["result"] is None
            else NativeBuildResult.from_payload(payload["result"])
        )
        if require_result and result is None:
            return "no native JSON result for this request"
        claims = [RecordedBuildClaim.from_payload(value) for value in payload["claims"]]
        if result is not None:
            result.verify_claims(claims)
            if claim is None:
                return "native JSON result requires build custody"
            for selection_claim in claims:
                selection_claim.verify()
                claim(selection_claim)
                selection_claim.verify()
            result.verify_paths()
        recorded_output = payload["output"]
        selection = BuildOutputSelection.from_dict(recorded_output["selection"])
        selection.verify()
        if stat_token(selection.path) != tuple(recorded_output["token"]):
            return f"output {selection.path} changed"
        consumer = _record_path(recorded_output["consumer"])
        products = recorded_output["products"]
        if not isinstance(products, list):
            return "unreadable record: invalid products"
        product_paths = set()
        for role, product, token in products:
            if not isinstance(role, str) or not role:
                return "unreadable record: invalid product role"
            path = _record_path(product)
            product_paths.add(path)
            if stat_token(path) != tuple(token):
                return f"product {path} changed"
        if consumer != selection.path and consumer not in product_paths:
            return "unreadable record: consumer is not a product"
        changed = _first_change(
            payload["files"], payload["directories"], payload["probes"]
        )
        if changed is not None:
            return changed
        if time.time_ns() - metadata.st_mtime_ns > _RECORD_REFRESH_INTERVAL_NS:
            os.utime(path)
        if result is not None:
            if result.binary.path != selection.path:
                return "native JSON result differs from output selection"
            for selection_claim in claims:
                selection_claim.verify()
            result.verify()
        return BuildRecordHit(selection.path, result, consumer)
    except FileNotFoundError:
        return "no record for this request"
    except (
        OSError,
        ValueError,
        TypeError,
        KeyError,
        IndexError,
        OverflowError,
        RecursionError,
    ) as error:
        return f"unreadable record: {error}"


def trim_if_due(records_root: Path) -> None:
    """Delete records unused for a week, at most once an hour.

    Concurrent trims are harmless: each deletion is idempotent, and a reader
    that loses its record to a trim sees a miss.
    """
    marker = records_root / _TRIM_MARKER
    now = time.time_ns()
    try:
        if now - os.stat(marker).st_mtime_ns < _RECORD_REFRESH_INTERVAL_NS:
            return
    except FileNotFoundError:
        pass
    except OSError:
        return
    try:
        marker.touch()
        shards = list(os.scandir(records_root))
    except OSError:
        return
    for shard in shards:
        if shard.name == _TRANSIENT_DIRNAME:
            # Execution scratch is not a cache record. Age does not prove
            # that its writers have exited, including after parent death.
            continue
        if not shard.is_dir(follow_symlinks=False):
            continue
        try:
            entries = list(os.scandir(shard.path))
        except OSError:
            continue
        for entry in entries:
            try:
                if (
                    now - entry.stat(follow_symlinks=False).st_mtime_ns
                    > _RECORD_UNUSED_LIMIT_NS
                ):
                    os.unlink(entry.path)
            except OSError:
                continue
