from __future__ import annotations

import ast
import contextlib
import functools
import hashlib
import io
import json
import os
import sys
import tokenize
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.default_paths import _default_molt_cache
from molt.file_hashing import _sha256_file


@dataclass(frozen=True)
class PythonSourceSnapshot:
    """One captured byte generation supplies source text, identity and host AST."""

    path: Path
    content: bytes

    @classmethod
    def capture(cls, path: Path) -> PythonSourceSnapshot:
        return cls(path, path.read_bytes())

    @functools.cached_property
    def sha256(self) -> str:
        return hashlib.sha256(self.content).hexdigest()

    @functools.cached_property
    def text(self) -> str:
        encoding, _ = tokenize.detect_encoding(io.BytesIO(self.content).readline)
        source = self.content.decode(encoding)
        return source.replace("\r\n", "\n").replace("\r", "\n")

    @functools.cached_property
    def tree(self) -> ast.Module:
        try:
            # This is the host-source parser. Target consumers parse text through
            # their selected target authority, never through this host AST.
            return ast.parse(self.content, filename=str(self.path))
        except (SyntaxError, UnicodeError, ValueError) as exc:
            raise ValueError(
                f"cannot parse local Python source {self.path}: {exc}"
            ) from exc

    @functools.cached_property
    def ast_digest(self) -> str:
        from molt.compiler_analysis.python_binding_flow import python_ast_digest

        return python_ast_digest(self.tree)


@dataclass(frozen=True)
class _ModuleSourceLease:
    path: Path
    inline_source: str | None = None
    source_size: int | None = None
    mtime_ns: int | None = None

    @classmethod
    def path_backed(
        cls, path: Path, path_stat: os.stat_result | None = None
    ) -> "_ModuleSourceLease":
        if path_stat is None:
            with contextlib.suppress(OSError):
                path_stat = path.stat()
        return cls(
            path=path,
            inline_source=None,
            source_size=path_stat.st_size if path_stat is not None else None,
            mtime_ns=path_stat.st_mtime_ns if path_stat is not None else None,
        )

    @classmethod
    def inline(
        cls,
        path: Path,
        source: str,
        path_stat: os.stat_result | None = None,
    ) -> "_ModuleSourceLease":
        return cls(
            path=path,
            inline_source=source,
            source_size=len(source),
            mtime_ns=path_stat.st_mtime_ns if path_stat is not None else None,
        )

    @property
    def path_backed_source(self) -> bool:
        return self.inline_source is None

    def read(self, resolution_cache: Any | None = None) -> str:
        if self.inline_source is not None:
            return self.inline_source
        if self.source_size is not None or self.mtime_ns is not None:
            stat = self.path.stat()
            if self.source_size is not None and stat.st_size != self.source_size:
                raise OSError(
                    f"Source lease for {self.path} changed size during compile"
                )
            if self.mtime_ns is not None and stat.st_mtime_ns != self.mtime_ns:
                raise OSError(
                    f"Source lease for {self.path} changed mtime during compile"
                )
        if resolution_cache is not None:
            return resolution_cache.read_module_source(self.path, retain=False)
        return _read_module_source(self.path)

    def worker_payload(self) -> dict[str, Any]:
        if self.inline_source is not None:
            return {
                "kind": "inline",
                "path": str(self.path),
                "source": self.inline_source,
                "source_size": self.source_size,
                "mtime_ns": self.mtime_ns,
            }
        return {
            "kind": "path",
            "path": str(self.path),
            "source_size": self.source_size,
            "mtime_ns": self.mtime_ns,
        }


@dataclass(frozen=True)
class _ModuleSourceCatalog:
    leases: Mapping[str, _ModuleSourceLease]

    def lease_for(self, module_name: str, module_path: Path) -> _ModuleSourceLease:
        lease = self.leases.get(module_name)
        if lease is not None:
            return lease
        return _ModuleSourceLease.path_backed(module_path)

    def source_size(self, module_name: str, module_path: Path | None = None) -> int:
        lease = self.leases.get(module_name)
        if lease is not None and lease.source_size is not None:
            return lease.source_size
        if module_path is not None:
            with contextlib.suppress(OSError):
                return module_path.stat().st_size
        return 0

    def read_source(
        self,
        module_name: str,
        module_path: Path,
        resolution_cache: Any | None = None,
    ) -> str:
        return self.lease_for(module_name, module_path).read(resolution_cache)

    def worker_source_lease_payload(
        self, module_name: str, module_path: Path
    ) -> dict[str, Any]:
        return self.lease_for(module_name, module_path).worker_payload()


def _stat_device(stat: os.stat_result) -> int:
    return int(getattr(stat, "st_dev", 0) or 0)


_SOURCE_HASH_CACHE_SCHEMA_VERSION = 1


def _source_hash_stat_identity_is_strong(
    *,
    ctime_ns: int,
    inode: int,
    device: int,
) -> bool:
    if sys.platform.startswith("win"):
        return False
    return ctime_ns > 0 and inode > 0 and device >= 0


@functools.lru_cache(maxsize=16384)
def _source_hash_cache_path_cached(
    cache_root_str: str,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
) -> Path:
    identity = {
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "device": device,
    }
    encoded = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    digest = hashlib.sha256(encoded).hexdigest()
    return Path(cache_root_str) / "source_hash_cache" / digest[:2] / f"{digest}.json"


def _source_hash_cache_path(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
) -> Path:
    return _source_hash_cache_path_cached(
        os.fspath(cache_root),
        path_str,
        size,
        mtime_ns,
        ctime_ns,
        inode,
        device,
    )


def _read_source_hash_cache_payload(cache_path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(cache_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _write_source_hash_cache_payload(
    cache_path: Path,
    payload: dict[str, Any],
) -> None:
    try:
        _atomic_write_text(cache_path, json.dumps(payload, sort_keys=True) + "\n")
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
    device: int,
) -> str | None:
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
        return None
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
    )
    payload = _read_source_hash_cache_payload(cache_path)
    if (
        not isinstance(payload, dict)
        or payload.get("version") != _SOURCE_HASH_CACHE_SCHEMA_VERSION
        or payload.get("path") != path_str
        or payload.get("size") != size
        or payload.get("mtime_ns") != mtime_ns
        or payload.get("ctime_ns") != ctime_ns
        or payload.get("inode") != inode
        or payload.get("device") != device
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
    device: int,
    source_hash: str,
) -> None:
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
        return
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
    )
    payload = {
        "version": _SOURCE_HASH_CACHE_SCHEMA_VERSION,
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "device": device,
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
    device: int,
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
        device=device,
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
        device=device,
        source_hash=source_hash,
    )
    return source_hash


def _source_content_sha256(
    path: Path,
    path_stat: os.stat_result | None = None,
) -> str | None:
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
    inode = int(getattr(path_stat, "st_ino", 0) or 0)
    device = _stat_device(path_stat)
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
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
        device,
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
    path_stats: Mapping[str, os.stat_result | None] | None = None,
) -> _ModuleSourceCatalog:
    leases: dict[str, _ModuleSourceLease] = {}
    module_sources = module_sources or {}
    for module_name, module_path in module_graph.items():
        path_stat = path_stats.get(module_name) if path_stats is not None else None
        inline_source = module_sources.get(module_name)
        if inline_source is not None:
            leases[module_name] = _ModuleSourceLease.inline(
                module_path, inline_source, path_stat
            )
        else:
            leases[module_name] = _ModuleSourceLease.path_backed(module_path, path_stat)
    return _ModuleSourceCatalog(leases=leases)
