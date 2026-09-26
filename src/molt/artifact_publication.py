from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
import re
import shutil
import stat
import time
import uuid
import warnings
from collections.abc import Callable, Iterable, Iterator, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import Any, TypedDict

from molt.file_publication import (
    durable_namespace_replace as _durable_namespace_replace,
    durable_replace as _durable_replace,
    canonical_file_leaf,
    fsync_directory as _fsync_directory,
    _flush_staged_file,
    is_link_like as _is_link_like,
    is_owned_staged_file_path,
    is_staged_file_path,
    staged_file_path,
)
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.file_hashing import _sha256_file
from molt.file_deletion import unlink_file
from molt.exact_json import canonical_json_bytes, encode_exact, loads_exact
from molt.toolchain_identity import (
    stable_regular_file_version,
    verify_stable_regular_file_identity,
)


_BACKUP_UNLINK_ATTEMPTS = 8
_PUBLICATION_JOURNAL_SCHEMA = "molt.artifact-publication.v2"
_PUBLICATION_JOURNAL_KEYS = frozenset(
    {"schema", "transaction_id", "state", "journals", "replacements", "removals"}
)
_PUBLICATION_JOURNAL_PREFIX = ".molt-artifact-publication-"
_PUBLICATION_JOURNAL_SUFFIX = ".json"
_PUBLICATION_LOCK_NAME = ".molt-artifact-publication.lock"
_PUBLICATION_RECEIPT_NAME_RE = re.compile(
    r"^\.molt-artifact-receipt-[0-9a-f]{64}\.json$"
)
_PUBLICATION_LOCK_TIMEOUT_SECONDS = 300.0
_PUBLICATION_JOURNAL_NAME_RE = re.compile(
    rf"^{re.escape(_PUBLICATION_JOURNAL_PREFIX)}"
    rf"(?P<transaction_id>[0-9a-f]{{32}})"
    rf"{re.escape(_PUBLICATION_JOURNAL_SUFFIX)}$"
)
_PUBLICATION_JOURNAL_STAGE_NAME_RE = re.compile(
    rf"^{re.escape(_PUBLICATION_JOURNAL_PREFIX)}"
    rf"(?P<transaction_id>[0-9a-f]{{32}})"
    rf"{re.escape(_PUBLICATION_JOURNAL_SUFFIX)}"
    r"\.stage-(?P<nonce>[0-9a-f]{32})\.tmp$"
)


@dataclass(frozen=True)
class _PublicationDirectoryResidue:
    journals: tuple[Path, ...]
    journal_stages: tuple[Path, ...]


@dataclass(frozen=True)
class _TransactionRecovery:
    committed: bool
    retained: tuple[Path, ...]


class _RetirementEntry(TypedDict):
    final: str
    backup: str
    had_final: bool
    prior_identity: list[int] | None


class _ReplacementEntry(_RetirementEntry):
    staged: str
    installed_identity: list[int]


def _unlink_backup(path: Path) -> None:
    """Remove one retired artifact through a bounded sharing-contention policy."""

    delay = 0.01
    for attempt in range(_BACKUP_UNLINK_ATTEMPTS):
        try:
            _require_regular_or_absent(path, role="retired artifact")
            unlink_file(path)
            return
        except FileNotFoundError:
            return
        except OSError:
            if attempt + 1 == _BACKUP_UNLINK_ATTEMPTS:
                raise
            time.sleep(delay)
            delay *= 2


def _warn_after_commit(message: str) -> None:
    """Report cleanup residue without allowing warning policy to undo success."""

    try:
        with warnings.catch_warnings():
            warnings.simplefilter("always", RuntimeWarning)
            warnings.warn(message, RuntimeWarning, stacklevel=3)
    except Exception:
        # Publication has crossed its durable commit point. Diagnostic delivery
        # is secondary and must never turn committed bytes into a reported abort.
        pass


def discard_staged_output(path: Path) -> None:
    """Reclaim an owned private leaf, reporting residue without masking results."""
    try:
        _unlink_backup(path)
    except OSError as exc:
        _warn_after_commit(f"artifact publication retained private stage {path}: {exc}")


def _path_sort_key(path: Path) -> str:
    return os.path.normcase(os.fspath(path))


def _canonical_leaf(path: Path, *, role: str, create_parent: bool) -> Path:
    """Delegate leaf custody to the single-file publication authority."""

    return canonical_file_leaf(path, create_parent=create_parent, role=role)


def _reject_authority_destination(path: Path, *, role: str) -> None:
    name = path.name.casefold()
    if (
        name == _PUBLICATION_LOCK_NAME
        or _PUBLICATION_JOURNAL_NAME_RE.fullmatch(name)
        or _PUBLICATION_JOURNAL_STAGE_NAME_RE.fullmatch(name)
    ):
        raise ValueError(f"{role} is reserved for publication custody: {path}")


def _require_owned_stage(
    staged: Path,
    final: Path,
    *,
    role: str,
    purpose: str | None = None,
    suffix: str | None = None,
) -> None:
    if not is_owned_staged_file_path(staged, final, purpose=purpose, suffix=suffix):
        raise ValueError(
            f"{role} does not use the final artifact's owned staging namespace: "
            f"{staged}, {final}"
        )


def _require_regular_or_absent(path: Path, *, role: str) -> None:
    if _is_link_like(path):
        raise ValueError(f"{role} must not be a link or junction: {path}")
    if path.exists() and not path.is_file():
        raise ValueError(f"{role} is not a regular file: {path}")


def _file_node_identity(path: Path) -> list[int]:
    _require_regular_or_absent(path, role="publication generation member")
    metadata = path.stat()
    if not metadata.st_ino:
        raise ValueError(f"publication requires stable file identity: {path}")
    return [metadata.st_dev, metadata.st_ino]


def staged_output_path(
    final: Path,
    *,
    purpose: str = "stage",
    suffix: str = ".tmp",
) -> Path:
    """Return a bounded same-directory staging path for a final artifact.

    Final artifact names may already approach the Windows component/path limits.
    Embedding that name again in a temporary component recursively amplifies the
    path and makes otherwise-valid publications fail before their external tool
    can start.  One digest-keyed authority keeps every staging and backup
    component bounded while preserving same-filesystem atomic replacement.
    """
    final = _canonical_leaf(Path(final), role="final artifact", create_parent=True)
    return staged_file_path(final, purpose=purpose, suffix=suffix)


def atomic_copy_file(src: Path, dst: Path) -> None:
    with staged_copy_file(src, dst) as tmp_path:
        publish_validated_outputs([(tmp_path, dst)])


@contextmanager
def staged_copy_file(
    src: Path,
    dst: Path,
    *,
    prepare: Callable[[Path], None] | None = None,
    expected_sha256: str | None = None,
) -> Iterator[Path]:
    """Own a verified private copy until its caller publishes or abandons it."""

    tmp_path = staged_output_path(dst, purpose="copy")
    try:
        shutil.copyfile(src, tmp_path)
        if prepare is not None:
            prepare(tmp_path)
        if expected_sha256 is not None and _sha256_file(tmp_path) != expected_sha256:
            raise ValueError(f"source changed while staging verified copy: {src}")
        shutil.copymode(src, tmp_path)
        yield tmp_path
    finally:
        discard_staged_output(tmp_path)


def fsync_file(path: Path) -> None:
    """Flush and verify a private staged artifact before consumer validation."""

    _flush_staged_file(Path(path))


def atomic_write_bytes(path: Path, data: bytes) -> None:
    tmp_path = staged_output_path(path)
    try:
        with tmp_path.open("wb") as handle:
            handle.write(data)
        publish_validated_outputs([(tmp_path, path)])
    finally:
        discard_staged_output(tmp_path)


def atomic_write_text(path: Path, text: str, *, encoding: str = "utf-8") -> None:
    atomic_write_bytes(path, text.encode(encoding))


def atomic_write_json(
    path: Path,
    payload: Any,
    *,
    indent: int | None = 2,
    sort_keys: bool = False,
    default: Callable[[Any], Any] | None = None,
) -> None:
    """Publish JSON through the shared exact atomic codec."""

    atomic_write_bytes(
        path,
        encode_exact(
            payload,
            indent=indent,
            sort_keys=sort_keys,
            default=default,
        ),
    )


def _journal_path(parent: Path, transaction_id: str) -> Path:
    return parent / (
        f"{_PUBLICATION_JOURNAL_PREFIX}{transaction_id}{_PUBLICATION_JOURNAL_SUFFIX}"
    )


def _journal_stage_path(path: Path) -> Path:
    match = _PUBLICATION_JOURNAL_NAME_RE.fullmatch(path.name)
    if match is None:
        raise ValueError(f"invalid artifact publication journal path: {path}")
    return path.with_name(f"{path.name}.stage-{uuid.uuid4().hex}.tmp")


def _write_journal(path: Path, payload: Mapping[str, Any]) -> None:
    encoded = encode_exact(payload, indent=None)
    staged = _journal_stage_path(path)
    try:
        staged.write_bytes(encoded)
        _durable_replace(staged, path)
    finally:
        discard_staged_output(staged)


def _write_journal_copies(payload: Mapping[str, Any]) -> None:
    for raw_path in payload["journals"]:
        _write_journal(Path(raw_path), payload)


def _publication_directory_residue(parent: Path) -> _PublicationDirectoryResidue:
    journals: list[Path] = []
    journal_stages: list[Path] = []
    try:
        with os.scandir(parent) as children:
            for child in children:
                target = None
                if _PUBLICATION_JOURNAL_NAME_RE.fullmatch(child.name):
                    target = journals
                elif _PUBLICATION_JOURNAL_STAGE_NAME_RE.fullmatch(child.name):
                    target = journal_stages
                if target is None:
                    continue
                if not child.is_file(follow_symlinks=False):
                    raise OSError(
                        "artifact publication authority path is not a regular file: "
                        f"{child.path}"
                    )
                target.append(Path(child.path))
    except FileNotFoundError:
        return _PublicationDirectoryResidue(journals=(), journal_stages=())
    return _PublicationDirectoryResidue(
        journals=tuple(sorted(journals, key=_path_sort_key)),
        journal_stages=tuple(sorted(journal_stages, key=_path_sort_key)),
    )


def _load_journal(path: Path) -> dict[str, Any]:
    path = _canonical_leaf(
        path, role="artifact publication journal", create_parent=False
    )
    if not path.is_file():
        raise OSError(f"artifact publication journal is not a regular file: {path}")
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise OSError(f"invalid artifact publication journal {path}: {exc}") from exc
    if not isinstance(payload, dict) or set(payload) != _PUBLICATION_JOURNAL_KEYS:
        raise OSError(f"invalid artifact publication journal payload: {path}")
    transaction_id = payload.get("transaction_id")
    expected_name = (
        f"{_PUBLICATION_JOURNAL_PREFIX}{transaction_id}{_PUBLICATION_JOURNAL_SUFFIX}"
    )
    if (
        payload.get("schema") != _PUBLICATION_JOURNAL_SCHEMA
        or payload.get("state") not in {"prepared", "committed", "aborted"}
        or not isinstance(transaction_id, str)
        or len(transaction_id) != 32
        or any(character not in "0123456789abcdef" for character in transaction_id)
        or path.name != expected_name
    ):
        raise OSError(f"invalid artifact publication journal identity: {path}")
    journals = payload.get("journals")
    replacements = payload.get("replacements")
    removals = payload.get("removals")
    if (
        not isinstance(journals, list)
        or not journals
        or not isinstance(replacements, list)
        or not isinstance(removals, list)
    ):
        raise OSError(f"invalid artifact publication journal structure: {path}")
    journal_paths = tuple(Path(value) for value in journals if isinstance(value, str))
    if len(journal_paths) != len(journals) or any(
        not journal.is_absolute() or journal.name != expected_name
        for journal in journal_paths
    ):
        raise OSError(f"invalid artifact publication journal copies: {path}")
    canonical_journals = tuple(
        _canonical_leaf(
            journal,
            role="artifact publication journal copy",
            create_parent=False,
        )
        for journal in journal_paths
    )
    if (
        journal_paths != canonical_journals
        or len(set(journal_paths)) != len(journal_paths)
        or tuple(sorted(journal_paths, key=_path_sort_key)) != journal_paths
        or path not in set(journal_paths)
    ):
        raise OSError(f"artifact publication journal omits its own copy: {path}")
    journal_parents = {journal.parent for journal in journal_paths}
    seen_finals: set[Path] = set()
    for group, require_staged in ((replacements, True), (removals, False)):
        for entry in group:
            if not isinstance(entry, dict):
                raise OSError(f"invalid artifact publication journal entry: {path}")
            keys = {"final", "backup", "had_final", "prior_identity"}
            if require_staged:
                keys.update(("staged", "installed_identity"))
            if set(entry) != keys or not isinstance(entry["had_final"], bool):
                raise OSError(f"invalid artifact publication journal entry: {path}")
            for key in (
                "prior_identity",
                *(("installed_identity",) if require_staged else ()),
            ):
                identity = entry[key]
                if key == "prior_identity" and not entry["had_final"]:
                    if identity is not None:
                        raise OSError(f"unexpected prior publication identity: {path}")
                elif (
                    not isinstance(identity, list)
                    or len(identity) != 2
                    or any(type(value) is not int or value < 0 for value in identity)
                    or not identity[1]
                ):
                    raise OSError(f"invalid publication file identity: {path}")
            for key in keys - {"had_final", "prior_identity", "installed_identity"}:
                value = entry[key]
                if not isinstance(value, str) or not Path(value).is_absolute():
                    raise OSError(
                        f"invalid artifact publication journal path {key}: {path}"
                    )
            final = _canonical_leaf(
                Path(entry["final"]),
                role="artifact publication final",
                create_parent=False,
            )
            backup = _canonical_leaf(
                Path(entry["backup"]),
                role="artifact publication backup",
                create_parent=False,
            )
            if (
                Path(entry["final"]) != final
                or Path(entry["backup"]) != backup
                or final.parent not in journal_parents
                or final in seen_finals
            ):
                raise OSError(f"invalid artifact publication final path: {path}")
            try:
                _reject_authority_destination(final, role="artifact publication final")
            except ValueError as exc:
                raise OSError(
                    f"invalid artifact publication final path: {path}"
                ) from exc
            seen_finals.add(final)
            try:
                _require_owned_stage(
                    backup,
                    final,
                    role="artifact publication backup",
                    purpose="backup",
                    suffix=".old",
                )
            except ValueError as exc:
                raise OSError(
                    f"invalid artifact publication backup path: {path}"
                ) from exc
            if require_staged:
                staged = _canonical_leaf(
                    Path(entry["staged"]),
                    role="artifact publication stage",
                    create_parent=False,
                )
                if Path(entry["staged"]) != staged:
                    raise OSError(f"invalid artifact publication staged path: {path}")
                try:
                    _require_owned_stage(
                        staged,
                        final,
                        role="artifact publication stage",
                    )
                except ValueError as exc:
                    raise OSError(
                        f"invalid artifact publication staged path: {path}"
                    ) from exc
    return payload


def _journal_parent_closure(
    parents: set[Path],
) -> tuple[set[Path], dict[Path, _PublicationDirectoryResidue]]:
    expanded = set(parents)
    residue_by_parent: dict[Path, _PublicationDirectoryResidue] = {}
    for parent in tuple(parents):
        residue = _publication_directory_residue(parent)
        residue_by_parent[parent] = residue
        for journal in residue.journals:
            payload = _load_journal(journal)
            expanded.update(
                Path(value).parent.resolve() for value in payload["journals"]
            )
    return expanded, residue_by_parent


def _publication_lock_path(parent: Path) -> Path:
    """One stable lock for a public directory, independent of cache settings."""

    return parent / _PUBLICATION_LOCK_NAME


def _verify_publication_lock(path: Path, *, opened: int | None = None) -> None:
    """Reject an indirect lock leaf both before and after opening it."""

    if _is_link_like(path):
        raise ValueError(f"artifact publication lock must not be indirect: {path}")
    try:
        named = path.lstat()
    except FileNotFoundError:
        if opened is None:
            return
        raise ValueError(f"artifact publication lock disappeared: {path}") from None
    if not stat.S_ISREG(named.st_mode):
        raise ValueError(f"artifact publication lock is not a regular file: {path}")
    if named.st_size != 0:
        raise ValueError(f"artifact publication lock must be empty: {path}")
    if opened is not None:
        held = os.fstat(opened)
        if not stat.S_ISREG(held.st_mode) or (
            held.st_dev,
            held.st_ino,
        ) != (named.st_dev, named.st_ino):
            raise ValueError(f"artifact publication lock changed while opening: {path}")


def is_publication_lock_file(path: Path) -> bool:
    """Recognize verified persistent lock metadata, never artifact payload.

    Directory packagers may omit this leaf, but must never unlink it: waiters
    could still hold its inode. Malformed reserved leaves are errors, not data
    to silently omit or package.
    """
    if path.name.casefold() != _PUBLICATION_LOCK_NAME:
        return False
    _verify_publication_lock(path)
    return path.is_file()


def publication_receipt_path(anchor: Path) -> Path:
    """One persistent family record per destination, independent of build policy."""
    anchor = _canonical_leaf(
        anchor, role="publication family anchor", create_parent=True
    )
    digest = hashlib.sha256(os.fsencode(os.path.normcase(anchor.name))).hexdigest()
    return anchor.parent / f".molt-artifact-receipt-{digest}.json"


@contextmanager
def _publication_locks(
    initial_parents: set[Path],
) -> Iterator[tuple[set[Path], Mapping[Path, _PublicationDirectoryResidue]]]:
    parents = {parent.resolve() for parent in initial_parents}
    while True:
        handles = []
        try:
            lock_paths = {_publication_lock_path(parent) for parent in parents}
            for lock_path in sorted(lock_paths, key=_path_sort_key):
                _verify_publication_lock(lock_path)
                handle = _acquire_file_lock(
                    lock_path,
                    timeout_s=_PUBLICATION_LOCK_TIMEOUT_SECONDS,
                    timeout_message=(
                        f"timed out waiting for artifact publication lock {lock_path}"
                    ),
                )
                handles.append(handle)
                _verify_publication_lock(lock_path, opened=handle.file.fileno())
            expanded, residue_by_parent = _journal_parent_closure(parents)
            if expanded == parents:
                yield parents, residue_by_parent
                return
        finally:
            for handle in reversed(handles):
                _release_file_lock(handle)
        parents = expanded


def _recover_locked_publications(
    residue_by_parent: Mapping[Path, _PublicationDirectoryResidue],
) -> None:
    stale_residue = _reap_orphan_journal_stages(residue_by_parent)
    if stale_residue:
        raise OSError(
            "artifact publication recovery retained orphan journal stages: "
            + ", ".join(str(path) for path in stale_residue)
        )
    stale_residue = _recover_publication_journals(residue_by_parent)
    if stale_residue:
        raise OSError(
            "artifact publication recovery retained cleanup residue: "
            + ", ".join(str(path) for path in stale_residue)
        )


@contextmanager
def publication_locks(paths: Iterable[Path]) -> Iterator[None]:
    """Recover and lock every destination directory for a consistent family read."""

    parents = {
        _canonical_leaf(Path(path), role="publication read", create_parent=False).parent
        for path in paths
    }
    with _publication_locks(parents) as (_locked_parents, residue_by_parent):
        _recover_locked_publications(residue_by_parent)
        yield


def _payload_inventory(
    roots: tuple[Path, ...],
    include: Callable[[Path, Path], bool] | None = None,
) -> tuple[dict[Path, tuple[Path, ...]], set[Path]]:
    payloads: dict[Path, tuple[Path, ...]] = {}
    publication_parents: set[Path] = set()
    for root in roots:
        files: list[Path] = []
        pending = [root]
        while pending:
            directory = pending.pop()
            if _is_link_like(directory) or not directory.is_dir():
                raise ValueError(
                    f"package source is not a direct directory: {directory}"
                )
            with os.scandir(directory) as entries:
                for entry in entries:
                    path = Path(entry.path)
                    if _is_link_like(path):
                        raise ValueError(
                            f"package source must not contain links: {path}"
                        )
                    if is_staged_file_path(path):
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        pending.append(path)
                        continue
                    if not entry.is_file(follow_symlinks=False):
                        raise ValueError(
                            f"package source is not a regular file: {path}"
                        )
                    if is_publication_lock_file(path):
                        publication_parents.add(directory)
                    elif _PUBLICATION_JOURNAL_NAME_RE.fullmatch(
                        path.name.casefold()
                    ) or _PUBLICATION_JOURNAL_STAGE_NAME_RE.fullmatch(
                        path.name.casefold()
                    ):
                        publication_parents.add(directory)
                    elif not _PUBLICATION_RECEIPT_NAME_RE.fullmatch(
                        path.name.casefold()
                    ) and (include is None or include(root, path)):
                        files.append(path)
        payloads[root] = tuple(sorted(files, key=_path_sort_key))
    return payloads, publication_parents


@contextmanager
def publication_payload_snapshot(
    roots: Iterable[Path],
    *,
    include: Callable[[Path, Path], bool] | None = None,
) -> Iterator[dict[Path, tuple[Path, ...]]]:
    """Read coherent payload trees without packaging private publication state.

    Lock existing publication namespaces, recover their journals, and reject
    membership or file mutation during the consumer's read. Ordinary read-only
    package trees gain no lock files. A concurrently introduced namespace is a
    changed snapshot, never permission to package an unguarded generation.
    Consumers must finish this context before publishing their private archive.
    """
    canonical_roots = tuple(dict.fromkeys(Path(root).absolute() for root in roots))
    for root in canonical_roots:
        if _is_link_like(root):
            raise ValueError(f"package source must not be indirect: {root}")
    canonical_roots = tuple(root.resolve(strict=True) for root in canonical_roots)
    _payloads, parents = _payload_inventory(canonical_roots, include)
    with _publication_locks(parents) as (locked, residue):
        _recover_locked_publications(residue)
        payloads, observed = _payload_inventory(canonical_roots, include)
        if not observed.issubset(locked):
            raise ValueError("package publication namespaces changed during snapshot")
        identities = tuple(
            stable_regular_file_version(path, label="package payload")
            for path in dict.fromkeys(
                path for paths in payloads.values() for path in paths
            )
        )
        yield payloads
        after, after_parents = _payload_inventory(canonical_roots, include)
        if after != payloads or after_parents != observed:
            raise ValueError("package source membership changed during snapshot")
        for identity in identities:
            verify_stable_regular_file_identity(identity, label="package payload")


def _journal_core(payload: Mapping[str, Any]) -> str:
    return canonical_json_bytes(
        {key: value for key, value in payload.items() if key != "state"}
    ).decode("utf-8")


def _move_new_final_back_to_stage(entry: Mapping[str, Any]) -> None:
    final = Path(entry["final"])
    staged = Path(entry["staged"])
    backup = Path(entry["backup"])
    had_final = bool(entry["had_final"])
    if backup.exists() or backup.is_symlink():
        if _file_node_identity(backup) != entry["prior_identity"]:
            raise OSError(f"publication backup identity changed: {backup}")
        if final.exists() or final.is_symlink():
            if _file_node_identity(final) != entry["installed_identity"]:
                raise OSError(
                    f"publication final belongs to another generation: {final}"
                )
            if staged.exists() or staged.is_symlink():
                raise OSError(
                    "cannot recover artifact publication with both staged and final "
                    f"outputs present: {staged}, {final}"
                )
            _durable_namespace_replace(final, staged)
        _durable_namespace_replace(backup, final)
        return
    # A missing backup can also mean an earlier rollback already restored the
    # prior final and crashed during cleanup. Keep recovery idempotent there.
    if had_final and (
        not final.is_file() or _file_node_identity(final) != entry["prior_identity"]
    ):
        raise OSError(
            f"cannot recover prior artifact without matching final or backup: {final}"
        )
    if not had_final and (final.exists() or final.is_symlink()):
        if _file_node_identity(final) != entry["installed_identity"]:
            raise OSError(f"publication final belongs to another generation: {final}")
        if staged.exists() or staged.is_symlink():
            raise OSError(
                "cannot recover new artifact with both staged and final outputs "
                f"present: {staged}, {final}"
            )
        _durable_namespace_replace(final, staged)


def _cleanup_paths(paths: Iterator[Path]) -> tuple[Path, ...]:
    retained: list[Path] = []
    for path in paths:
        try:
            _unlink_backup(path)
            _fsync_directory(path.parent)
        except OSError:
            retained.append(path)
    return tuple(retained)


def _recover_transaction(journals: tuple[Path, ...]) -> _TransactionRecovery:
    existing_copies = tuple(
        (path, _load_journal(path)) for path in journals if path.is_file()
    )
    if not existing_copies:
        return _TransactionRecovery(committed=False, retained=())
    copies = tuple(copy for _path, copy in existing_copies)
    cores = {_journal_core(payload) for payload in copies}
    if len(cores) != 1:
        raise OSError(
            "artifact publication journal copies disagree: "
            + ", ".join(str(path) for path in journals)
        )
    payload = copies[0]
    committed = any(copy["state"] == "committed" for copy in copies)
    aborted = any(copy["state"] == "aborted" for copy in copies)
    if committed and aborted:
        raise OSError("artifact publication has conflicting terminal journal states")
    if committed:
        # A single committed copy is the durable commit point. Before deleting
        # any backup, stage, or journal, propagate it to every surviving copy:
        # otherwise a crash while deleting the only committed copy would leave
        # a prepared copy that could roll back the committed generation.
        committed_payload = {**payload, "state": "committed"}
        for journal, copy in existing_copies:
            if copy["state"] == "committed":
                continue
            try:
                _write_journal(journal, committed_payload)
            except OSError:
                return _TransactionRecovery(
                    committed=True,
                    retained=tuple(path for path in journals if path.is_file()),
                )
    replacements = tuple(payload["replacements"])
    removals = tuple(payload["removals"])
    if not committed and not aborted:
        for entry in reversed(replacements):
            _move_new_final_back_to_stage(entry)
        for entry in reversed(removals):
            final = Path(entry["final"])
            backup = Path(entry["backup"])
            if backup.exists() or backup.is_symlink():
                if _file_node_identity(backup) != entry["prior_identity"]:
                    raise OSError(f"publication backup identity changed: {backup}")
                if final.exists() or final.is_symlink():
                    raise OSError(
                        "cannot restore retired artifact over an occupied final: "
                        f"{final}"
                    )
                _durable_namespace_replace(backup, final)
            elif entry["had_final"] and (
                not final.is_file()
                or _file_node_identity(final) != entry["prior_identity"]
            ):
                raise OSError(
                    f"cannot recover retired artifact without matching final or backup: {final}"
                )
    if not committed:
        # Rollback has completed, or a prior recovery already recorded it.
        # Preserve that terminal fact in every surviving copy before deleting
        # even one: a prepared orphan must never adopt a later producer's file.
        aborted_payload = {**payload, "state": "aborted"}
        for journal, copy in existing_copies:
            if copy["state"] == "aborted":
                continue
            try:
                _write_journal(journal, aborted_payload)
            except OSError:
                return _TransactionRecovery(
                    committed=False,
                    retained=tuple(path for path in journals if path.is_file()),
                )
    cleanup = [Path(entry["backup"]) for entry in (*removals, *replacements)]
    cleanup.extend(Path(entry["staged"]) for entry in replacements)
    retained = _cleanup_paths(
        path for path in cleanup if path.exists() or path.is_symlink()
    )
    if retained:
        return _TransactionRecovery(committed=committed, retained=retained)
    retained_journals = _cleanup_paths(
        path
        for path in (Path(value) for value in payload["journals"])
        if path.exists() or path.is_symlink()
    )
    return _TransactionRecovery(committed=committed, retained=retained_journals)


def _recover_publication_journals(
    residue_by_parent: Mapping[Path, _PublicationDirectoryResidue],
) -> tuple[Path, ...]:
    by_transaction: dict[str, list[Path]] = {}
    for residue in residue_by_parent.values():
        for journal in residue.journals:
            payload = _load_journal(journal)
            by_transaction.setdefault(payload["transaction_id"], []).append(journal)
    retained: list[Path] = []
    for transaction_id in sorted(by_transaction):
        payload = _load_journal(by_transaction[transaction_id][0])
        journal_paths = tuple(Path(value) for value in payload["journals"])
        if not set(by_transaction[transaction_id]).issubset(journal_paths):
            raise OSError(
                "artifact publication journal copies disagree: "
                + ", ".join(str(path) for path in by_transaction[transaction_id])
            )
        retained.extend(_recover_transaction(journal_paths).retained)
    return tuple(dict.fromkeys(retained))


def _reap_orphan_journal_stages(
    residue_by_parent: Mapping[Path, _PublicationDirectoryResidue],
) -> tuple[Path, ...]:
    return _cleanup_paths(
        stage
        for residue in residue_by_parent.values()
        for stage in residue.journal_stages
    )


@contextmanager
def _publication_write_scope(
    parents: set[Path],
    select_removals: Callable[[frozenset[Path]], Iterable[Path]] | None,
) -> Iterator[tuple[Path, ...]]:
    while True:
        with _publication_locks(parents) as (locked, residue):
            _recover_locked_publications(residue)
            selected = tuple(
                dict.fromkeys(
                    _canonical_leaf(
                        Path(path), role="retired artifact", create_parent=True
                    )
                    for path in (
                        select_removals(frozenset(locked)) if select_removals else ()
                    )
                )
            )
            expanded = locked | {path.parent for path in selected}
            if expanded == locked:
                yield selected
                return
        # A receipt can introduce a previously unknown output parent. Release
        # all locks and acquire the expanded set in canonical order, then read
        # the receipt again: no lock upgrade or stale retirement decision.
        parents = expanded


def publish_validated_outputs(
    pairs: list[tuple[Path, Path]],
    *,
    removals: tuple[Path, ...] = (),
    select_removals: Callable[[frozenset[Path]], Iterable[Path]] | None = None,
) -> tuple[Path, ...]:
    """Durably replace and retire one crash-recoverable artifact set.

    Callers must validate every staged source before calling this function.
    Each staged source must use :func:`staged_output_path`, which binds it to the
    final artifact's canonical parent and identity. A durable journal is copied
    into every destination directory before namespace changes begin.
    Recovery restores the complete prior generation unless at least one journal
    copy records the commit point; committed generations retain journal custody
    until every backup is reaped.

    An optional pure removal selector reads family metadata while locked. It
    may run again when its output expands the destination lock set; it must
    not publish or take publication locks itself.
    """
    normalized = []
    for raw_staged, raw_final in pairs:
        final = _canonical_leaf(
            Path(raw_final), role="final artifact", create_parent=True
        )
        staged = _canonical_leaf(
            Path(raw_staged), role="staged artifact", create_parent=False
        )
        _reject_authority_destination(final, role="final artifact")
        _require_owned_stage(staged, final, role="staged artifact")
        normalized.append((staged, final))
    normalized_removals = tuple(
        _canonical_leaf(Path(path), role="retired artifact", create_parent=True)
        for path in removals
    )
    for final in normalized_removals:
        _reject_authority_destination(final, role="retired artifact")
    seen_finals: set[Path] = set()
    seen_stages: set[Path] = set()
    for staged, final in normalized:
        if final in seen_finals:
            raise ValueError(f"duplicate final artifact in publication set: {final}")
        seen_finals.add(final)
        if staged in seen_stages:
            raise ValueError(f"duplicate staged artifact in publication set: {staged}")
        seen_stages.add(staged)
        if staged == final:
            raise ValueError(f"staged and final artifact paths match: {final}")
        _require_regular_or_absent(final, role="final artifact")
    for final in normalized_removals:
        if final in seen_finals:
            raise ValueError(f"duplicate final artifact in publication set: {final}")
        seen_finals.add(final)
        _require_regular_or_absent(final, role="retired artifact")

    overlapping = seen_stages & seen_finals
    if overlapping:
        raise ValueError(
            "staged artifact overlaps a publication destination: "
            + ", ".join(str(path) for path in sorted(overlapping, key=_path_sort_key))
        )

    if not seen_finals:
        return ()
    parents = {final.parent for final in seen_finals}
    with _publication_write_scope(parents, select_removals) as selected_removals:
        normalized_removals = tuple(
            dict.fromkeys((*normalized_removals, *selected_removals))
        )
        for final in selected_removals:
            _reject_authority_destination(final, role="retired artifact")
            if final in seen_stages or any(
                final == destination for _, destination in normalized
            ):
                raise ValueError(f"retired artifact overlaps publication: {final}")
        parents.update(path.parent for path in normalized_removals)
        for staged, final in normalized:
            _require_regular_or_absent(final, role="final artifact")
            if _is_link_like(staged):
                raise ValueError(
                    f"staged artifact must not be a link or junction: {staged}"
                )
            if not staged.is_file():
                raise FileNotFoundError(f"staged artifact missing: {staged}")
            _require_owned_stage(staged, final, role="staged artifact")
        for final in normalized_removals:
            _require_regular_or_absent(final, role="retired artifact")
        if len(normalized) == 1 and not normalized_removals:
            staged, final = normalized[0]
            _durable_replace(staged, final)
            return ()
        transaction_id = uuid.uuid4().hex
        journal_paths = tuple(
            _journal_path(parent, transaction_id)
            for parent in sorted(parents, key=_path_sort_key)
        )
        replacement_entries: list[_ReplacementEntry] = [
            {
                "staged": str(staged),
                "final": str(final),
                "backup": str(
                    staged_output_path(final, purpose="backup", suffix=".old")
                ),
                "had_final": final.is_file(),
                "prior_identity": _file_node_identity(final)
                if final.is_file()
                else None,
                "installed_identity": _file_node_identity(staged),
            }
            for staged, final in normalized
        ]
        removal_entries: list[_RetirementEntry] = [
            {
                "final": str(final),
                "backup": str(
                    staged_output_path(final, purpose="backup", suffix=".old")
                ),
                "had_final": final.is_file(),
                "prior_identity": _file_node_identity(final)
                if final.is_file()
                else None,
            }
            for final in normalized_removals
        ]
        journal: dict[str, Any] = {
            "schema": _PUBLICATION_JOURNAL_SCHEMA,
            "transaction_id": transaction_id,
            "state": "prepared",
            "journals": [str(path) for path in journal_paths],
            "replacements": replacement_entries,
            "removals": removal_entries,
        }
        try:
            _write_journal_copies(journal)
            for entry in (*removal_entries, *replacement_entries):
                if not entry["had_final"]:
                    continue
                _durable_namespace_replace(Path(entry["final"]), Path(entry["backup"]))
            for entry in replacement_entries:
                _durable_replace(Path(entry["staged"]), Path(entry["final"]))
            journal["state"] = "committed"
            _write_journal_copies(journal)
        except BaseException as primary:
            try:
                recovery = _recover_transaction(journal_paths)
            except BaseException as recovery_error:
                primary.add_note(
                    "artifact publication rollback recovery failed: "
                    f"{type(recovery_error).__name__}: {recovery_error}"
                )
                raise primary
            if not recovery.committed or not isinstance(primary, Exception):
                raise
            retained = recovery.retained
        else:
            try:
                retained = _recover_transaction(journal_paths).retained
            except BaseException as cleanup_error:
                if not isinstance(cleanup_error, Exception):
                    raise
                retained = tuple(
                    path for path in journal_paths if path.exists() or path.is_symlink()
                )
                _warn_after_commit(
                    "artifact publication committed but cleanup recovery failed: "
                    f"{type(cleanup_error).__name__}: {cleanup_error}"
                )
    if retained:
        _warn_after_commit(
            "artifact publication committed with journal-owned cleanup residue: "
            + ", ".join(str(path) for path in retained)
        )
    return retained
