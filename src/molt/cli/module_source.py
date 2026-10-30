from __future__ import annotations

import ast
from molt.python_private_names import resolve_python_private_names
import functools
import hashlib
import io
import json
import os
import tokenize
from collections.abc import Callable, Collection, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, BinaryIO, Literal, TYPE_CHECKING, TypeVar

from molt import file_publication
from molt.cli.atomic_io import _atomic_write_text
from molt.default_paths import _default_molt_cache
from molt.file_hashing import _sha256_file as _unobserved_sha256_file


if TYPE_CHECKING:
    from molt.cli.python_import_topology import PythonImportTopology


class PythonSourceChangedError(ValueError):
    """An admitted source generation no longer matches the current file."""


_Read = TypeVar("_Read")


def _read_source_input(
    path: Path, consume: Callable[[BinaryIO, int], tuple[_Read, str]]
) -> _Read:
    """Read one bounded source generation; reporting owns no semantic bytes."""
    from molt import input_report

    selector = path.absolute()
    input_report.report_probe(selector)
    selected = selector.resolve(strict=True)
    with input_report.capture_read(selector) as receipt:
        # Preserve ordinary source-open sharing and permission behavior. Unknown
        # change-time/selection evidence refuses reuse, not source compilation.
        with open(selector, "rb") as handle:
            receipt.opened()
            before = os.fstat(handle.fileno())
            value, digest = consume(handle, before.st_size)
            after = os.fstat(handle.fileno())
            token = input_report.stat_token(selector, before)
            if (
                token is None
                or token != input_report.stat_token(selector, after)
                or token != input_report.stat_token(selector)
                or selector.resolve(strict=True) != selected
            ):
                input_report.refuse(f"source generation changed while read: {path}")
                token = None
        input_report.report_probe(selector)
        receipt.complete(token, digest)
    return value


def _sha256_file(path: Path) -> str:
    from molt import input_report

    if not input_report.active():
        return _unobserved_sha256_file(path)
    from molt.toolchain_identity import iter_captured_file_chunks

    def consume(handle: BinaryIO, size: int) -> tuple[str, str]:
        digest = hashlib.sha256()
        for chunk in iter_captured_file_chunks(handle, size=size):
            digest.update(chunk)
        value = digest.hexdigest()
        return value, value

    return _read_source_input(path, consume)


@dataclass(frozen=True)
class PythonSourceSnapshot:
    """One captured byte generation supplies source text, identity and host AST."""

    path: Path
    content: bytes
    _reported_session: object | None = field(
        default=None, init=False, repr=False, compare=False
    )

    @classmethod
    def capture(cls, path: Path) -> PythonSourceSnapshot:
        from molt import input_report

        if not input_report.active():
            return cls(path, path.read_bytes())
        from molt.toolchain_identity import iter_captured_file_chunks

        def consume(
            handle: BinaryIO,
            size: int,
        ) -> tuple[PythonSourceSnapshot, str]:
            content = b"".join(iter_captured_file_chunks(handle, size=size))
            snapshot = cls(path, content)
            return snapshot, snapshot._sha256

        snapshot = _read_source_input(path, consume)
        snapshot.report_input()
        return snapshot

    @functools.cached_property
    def _sha256(self) -> str:
        return hashlib.sha256(self.content).hexdigest()

    def report_input(self) -> None:
        from molt import input_report

        session = input_report.report_consumed_buffer(
            self.path, self.content, previous_session=self._reported_session
        )
        object.__setattr__(self, "_reported_session", session)

    @property
    def sha256(self) -> str:
        self.report_input()
        return self._sha256

    @functools.cached_property
    def _decoded_text(self) -> str:
        encoding, _ = tokenize.detect_encoding(io.BytesIO(self.content).readline)
        source = self.content.decode(encoding)
        return source.replace("\r\n", "\n").replace("\r", "\n")

    @property
    def text(self) -> str:
        return self.text_for_filename(str(self.path))

    def text_for_filename(self, filename: str) -> str:
        """Decode Python source, preserving native parser encoding diagnostics.

        Only a failed encoding decode enters the host parser. It consumes the
        same captured bytes and logical filename; successful reads do no extra
        parsing. Selected-version grammar admission belongs to target_python.
        """
        self.report_input()
        try:
            return self._decoded_text
        except (SyntaxError, UnicodeDecodeError) as decode_error:
            ast.parse(self.content, filename=filename)
            raise RuntimeError(
                "Python source decoder rejected bytes accepted by the parser"
            ) from decode_error

    @functools.cached_property
    def _tree(self) -> ast.Module:
        try:
            # This is the host-source parser. Target consumers parse text through
            # their selected target authority, never through this host AST.
            return resolve_python_private_names(
                ast.parse(self.content, filename=str(self.path))
            )
        except (SyntaxError, UnicodeError, ValueError) as exc:
            raise ValueError(
                f"cannot parse local Python source {self.path}: {exc}"
            ) from exc

    @property
    def tree(self) -> ast.Module:
        self.report_input()
        return self._tree

    @functools.cached_property
    def _ast_digest(self) -> str:
        from molt.compiler_analysis.python_source_keys import python_ast_digest

        return python_ast_digest(self.tree)

    @property
    def ast_digest(self) -> str:
        self.report_input()
        return self._ast_digest


@dataclass(frozen=True)
class _ModuleSourceLease:
    """Immutable source identity carried from analysis through publication.

    Path leases identify raw bytes. Inline leases identify explicitly supplied
    text; their distinct kind prevents encoding-cookie bytes aliasing that text.
    Neither file size nor timestamps can establish this input generation.
    """

    path: Path
    source_sha256: str
    source_size: int
    inline_source: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "path", self.path.resolve())
        if (
            type(self.source_sha256) is not str
            or len(self.source_sha256) != 64
            or any(c not in "0123456789abcdef" for c in self.source_sha256)
            or type(self.source_size) is not int
            or self.source_size < 0
        ):
            raise ValueError("invalid source lease generation")
        if self.inline_source is not None:
            content = self.inline_source.encode("utf-8", errors="surrogatepass")
            if (
                len(content) != self.source_size
                or hashlib.sha256(content).hexdigest() != self.source_sha256
            ):
                raise ValueError("inline source lease lost text generation")

    @classmethod
    def path_backed(
        cls,
        path: Path,
        *,
        snapshot: PythonSourceSnapshot | None = None,
    ) -> "_ModuleSourceLease":
        snapshot = snapshot or PythonSourceSnapshot.capture(path)
        if snapshot.path.resolve() != path.resolve():
            raise ValueError("source lease snapshot lost path custody")
        return cls(path, snapshot.sha256, len(snapshot.content))

    @classmethod
    def inline(
        cls,
        path: Path,
        source: str,
    ) -> "_ModuleSourceLease":
        content = source.encode("utf-8", errors="surrogatepass")
        return cls(path, hashlib.sha256(content).hexdigest(), len(content), source)

    @property
    def source_kind(self) -> Literal["path", "inline"]:
        return "path" if self.inline_source is None else "inline"

    @property
    def path_backed_source(self) -> bool:
        return self.inline_source is None

    def validate_path(self, path: Path) -> None:
        if path != self.path and path.resolve() != self.path:
            raise ValueError(f"source lease lost path custody: {path}")

    def validate(self) -> None:
        if self.inline_source is not None:
            return
        try:
            current = _sha256_file(self.path)
        except OSError as exc:
            raise PythonSourceChangedError(
                f"source became unavailable during compile: {self.path}"
            ) from exc
        if current != self.source_sha256:
            raise PythonSourceChangedError(
                f"source generation changed during compile: {self.path}"
            )

    def read(self) -> str:
        if self.inline_source is not None:
            return self.inline_source
        try:
            snapshot = PythonSourceSnapshot.capture(self.path)
        except OSError as exc:
            raise PythonSourceChangedError(
                f"source became unavailable during compile: {self.path}"
            ) from exc
        if snapshot.sha256 != self.source_sha256:
            raise PythonSourceChangedError(
                f"source generation changed during compile: {self.path}"
            )
        return snapshot.text

    def worker_payload(self) -> dict[str, Any]:
        payload = {
            "kind": self.source_kind,
            "path": str(self.path),
            "source_sha256": self.source_sha256,
            "source_size": self.source_size,
        }
        if self.inline_source is not None:
            payload["source"] = self.inline_source
        return payload

    @classmethod
    def from_worker_payload(cls, payload: object) -> "_ModuleSourceLease":
        if not isinstance(payload, Mapping):
            raise ValueError("missing source lease")
        kind = payload.get("kind")
        expected = {"kind", "path", "source_sha256", "source_size"}
        if kind == "inline":
            expected.add("source")
        elif kind != "path":
            raise ValueError(f"unsupported source lease kind: {kind!r}")
        if set(payload) != expected:
            raise ValueError("source lease payload lost generation custody")
        path, sha256, size = (
            payload["path"],
            payload["source_sha256"],
            payload["source_size"],
        )
        source = payload.get("source")
        if (
            type(path) is not str
            or not path
            or type(sha256) is not str
            or type(size) is not int
        ):
            raise ValueError("invalid source lease generation")
        if kind == "inline" and type(source) is not str:
            raise ValueError("inline source lease is missing source text")
        return cls(Path(path), sha256, size, source)


@dataclass(frozen=True)
class _ModuleSourceCatalog:
    leases: Mapping[str, _ModuleSourceLease]
    import_topology: "PythonImportTopology | None" = None

    def lease_for(self, module_name: str, module_path: Path) -> _ModuleSourceLease:
        lease = self.leases.get(module_name)
        if lease is None:
            raise ValueError(f"module lacks captured source lease: {module_name!r}")
        lease.validate_path(module_path)
        return lease

    def source_size(self, module_name: str, module_path: Path | None = None) -> int:
        lease = self.leases[module_name]
        if module_path is not None:
            lease.validate_path(module_path)
        return lease.source_size

    def read_source(self, module_name: str, module_path: Path) -> str:
        return self.lease_for(module_name, module_path).read()

    def source_paths(self, reported_paths: Collection[str]) -> tuple[str, ...]:
        """The compiled files that a traceback frame can name, sorted.

        A frame names its module's logical source path. A toolchain-owned or
        generated module reports a fixed virtual path, never its file
        (``molt.logical_source_paths``), so only files that some module reports
        as its own path are listed.
        """
        reported = frozenset(reported_paths)
        return tuple(
            sorted(
                {
                    path
                    for lease in self.leases.values()
                    if lease.inline_source is None
                    and (path := os.fspath(lease.path)) in reported
                }
            )
        )

    def validate(self) -> None:
        if self.import_topology is not None:
            self.import_topology.validate()
        for lease in self.leases.values():
            lease.validate()

    def worker_source_lease_payload(
        self, module_name: str, module_path: Path
    ) -> dict[str, Any]:
        return self.lease_for(module_name, module_path).worker_payload()


_SOURCE_HASH_CACHE_SCHEMA_VERSION = 3


def _source_hash_stat_identity_is_strong(
    *,
    ctime_ns: int,
    inode: int,
) -> bool:
    return os.name != "nt" and ctime_ns > 0 and type(inode) is int and inode > 0


@functools.lru_cache(maxsize=16384)
def _source_hash_cache_path_cached(
    cache_root_str: str,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    mode: int,
) -> Path:
    coordinate = {
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "mode": mode,
    }
    encoded = json.dumps(coordinate, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    digest = hashlib.sha256(encoded).hexdigest()
    return (
        Path(cache_root_str)
        / "source_hash_cache"
        / "v3"
        / digest[:2]
        / f"{digest}.json"
    )


def _source_hash_cache_path(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    mode: int,
) -> Path:
    return _source_hash_cache_path_cached(
        os.fspath(cache_root),
        path_str,
        size,
        mtime_ns,
        ctime_ns,
        inode,
        mode,
    )


def _read_source_hash_cache_payload(cache_path: Path) -> dict[str, Any] | None:
    try:
        source = cache_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return None
    try:
        data = json.loads(source)
    except (ValueError, RecursionError):
        return None
    return data if isinstance(data, dict) else None


def _write_source_hash_cache_payload(
    cache_path: Path,
    payload: dict[str, Any],
) -> None:
    try:
        _atomic_write_text(
            cache_path,
            json.dumps(payload, sort_keys=True) + "\n",
            durability=file_publication.PublicationDurability.RECONSTRUCTIBLE,
        )
    except OSError:
        return


def _read_persistent_source_hash(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    mode: int,
) -> str | None:
    if not _source_hash_stat_identity_is_strong(ctime_ns=ctime_ns, inode=inode):
        return None
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        mode=mode,
    )
    payload = _read_source_hash_cache_payload(cache_path)
    if (
        not isinstance(payload, dict)
        or set(payload)
        != {
            "version",
            "path",
            "size",
            "mtime_ns",
            "ctime_ns",
            "inode",
            "mode",
            "source_sha256",
        }
        or payload.get("version") != _SOURCE_HASH_CACHE_SCHEMA_VERSION
        or payload.get("path") != path_str
        or payload.get("size") != size
        or payload.get("mtime_ns") != mtime_ns
        or payload.get("ctime_ns") != ctime_ns
        or type(payload.get("inode")) is not int
        or payload.get("inode") != inode
        or type(payload.get("mode")) is not int
        or payload.get("mode") != mode
    ):
        return None
    source_hash = payload.get("source_sha256")
    return source_hash if isinstance(source_hash, str) and source_hash else None


def _write_persistent_source_hash(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    mode: int,
    source_hash: str,
) -> None:
    if not _source_hash_stat_identity_is_strong(ctime_ns=ctime_ns, inode=inode):
        return
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        mode=mode,
    )
    payload = {
        "version": _SOURCE_HASH_CACHE_SCHEMA_VERSION,
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "mode": mode,
        "source_sha256": source_hash,
    }
    _write_source_hash_cache_payload(cache_path, payload)


@functools.lru_cache(maxsize=16384)
def _source_content_sha256_cached(
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    mode: int,
    cache_root_str: str,
) -> str | None:
    cache_root = Path(cache_root_str)
    cached_hash = _read_persistent_source_hash(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        mode=mode,
    )
    if cached_hash is not None:
        return cached_hash
    try:
        source_hash = _sha256_file(Path(path_str))
    except OSError:
        return None
    _write_persistent_source_hash(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        mode=mode,
        source_hash=source_hash,
    )
    return source_hash


def _source_content_sha256(
    path: Path,
    path_stat: os.stat_result | None = None,
) -> str | None:
    from molt import input_report

    if input_report.active():
        try:
            return _sha256_file(path)
        except OSError:
            return None
    if path_stat is None:
        try:
            path_stat = path.stat()
        except OSError:
            return None
    try:
        path_str = os.fspath(path.resolve())
    except OSError:
        path_str = os.fspath(path)
    ctime_ns = path_stat.st_ctime_ns
    inode = path_stat.st_ino
    if not _source_hash_stat_identity_is_strong(ctime_ns=ctime_ns, inode=inode):
        try:
            return _sha256_file(Path(path_str))
        except OSError:
            return None
    return _source_content_sha256_cached(
        path_str,
        path_stat.st_size,
        path_stat.st_mtime_ns,
        ctime_ns,
        inode,
        path_stat.st_mode,
        os.fspath(_default_molt_cache()),
    )


def _payload_source_matches(
    payload: Mapping[str, Any],
    path: Path,
    path_stat: os.stat_result,
) -> bool:
    expected_hash = payload.get("source_sha256")
    if not isinstance(expected_hash, str) or not expected_hash:
        return False
    # ``size`` is content-correlated (identical bytes => identical size), so a
    # size mismatch is a sound fast reject that avoids hashing. ``mtime_ns`` is
    # deliberately NOT gated: it is content-invariant metadata that a fresh git
    # checkout / file copy / pip reinstall / OneDrive sync resets even when the
    # bytes are unchanged, so gating on it forced a cold cross-session/worktree
    # re-lower of byte-identical source (build-dedup doctrine 74 law 1:
    # content-addressed, not metadata-addressed). The source-content sha256 below
    # is the sole correctness authority and already defends against the
    # coarse-mtime same-size collision the metadata gate cannot catch anyway.
    if payload.get("size") != path_stat.st_size:
        return False
    return _source_content_sha256(path, path_stat) == expected_hash


def _read_module_source(path: Path) -> str:
    return PythonSourceSnapshot.capture(path).text


def _build_module_source_catalog(
    module_graph: Mapping[str, Path],
    *,
    module_sources: Mapping[str, str] | None = None,
) -> _ModuleSourceCatalog:
    leases: dict[str, _ModuleSourceLease] = {}
    module_sources = module_sources or {}
    for module_name, module_path in module_graph.items():
        inline_source = module_sources.get(module_name)
        if inline_source is not None:
            leases[module_name] = _ModuleSourceLease.inline(module_path, inline_source)
        else:
            leases[module_name] = _ModuleSourceLease.path_backed(module_path)
    return _ModuleSourceCatalog(leases=leases)
