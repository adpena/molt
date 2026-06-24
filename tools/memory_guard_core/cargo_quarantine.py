from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import contextlib
import json
import os
from pathlib import Path
import shlex
import shutil

from tools.memory_guard_core.common import utc_timestamp as _utc_timestamp


DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP = 5


@dataclass(frozen=True, slots=True)
class CargoIncrementalQuarantineMove:
    original_path: str
    quarantined_path: str


@dataclass(frozen=True, slots=True)
class CargoIncrementalQuarantine:
    reason: str
    recorded_at: str
    target_dir: str
    quarantine_dir: str | None
    command: tuple[str, ...]
    cwd: str
    moved_paths: tuple[CargoIncrementalQuarantineMove, ...] = ()
    pruned_quarantine_dirs: tuple[str, ...] = ()
    errors: tuple[str, ...] = ()
    receipt_path: str | None = None


_CARGO_BUILD_STATE_EXECUTABLES = frozenset({"cargo", "rustc", "rustdoc"})


def _command_tokens(fragment: str) -> list[str]:
    try:
        return shlex.split(fragment)
    except ValueError:
        return fragment.split()


def _token_executable_name(token: str) -> str:
    text = token.strip().strip("\"'")
    name = text.replace("\\", "/").rsplit("/", 1)[-1]
    suffix = Path(name).suffix.casefold()
    if suffix in {".exe", ".cmd", ".bat"}:
        name = name[: -len(suffix)]
    return name.casefold()


def _command_invokes_cargo_build_state(command: Sequence[str]) -> bool:
    for item in command:
        for token in (item, *_command_tokens(item)):
            if _token_executable_name(token) in _CARGO_BUILD_STATE_EXECUTABLES:
                return True
    return False


def _samples_include_cargo_build_state(
    samples: Mapping[int, object],
    watched: set[int],
) -> bool:
    for pid in watched:
        sample = samples.get(pid)
        if sample is None:
            continue
        if _command_invokes_cargo_build_state(_command_tokens(sample.command)):
            return True
    return False


def _effective_guard_cwd(
    cwd: str | Path | None,
    environ: Mapping[str, str],
) -> Path:
    if cwd is not None:
        cwd_path = Path(cwd).expanduser()
        if cwd_path.is_absolute():
            return cwd_path.resolve(strict=False)
        return (Path.cwd() / cwd_path).resolve(strict=False)
    pwd = environ.get("PWD", "")
    if pwd:
        pwd_path = Path(pwd).expanduser()
        if pwd_path.is_absolute():
            return pwd_path.resolve(strict=False)
    return Path.cwd().resolve(strict=False)


def _cargo_target_dir(
    environ: Mapping[str, str],
    cwd: str | Path | None,
) -> Path:
    base = _effective_guard_cwd(cwd, environ)
    raw_target = environ.get("CARGO_TARGET_DIR", "").strip()
    if raw_target:
        target = Path(raw_target).expanduser()
        if target.is_absolute():
            return target.resolve(strict=False)
        return (base / target).resolve(strict=False)
    return (base / "target").resolve(strict=False)


def _cargo_incremental_dirs(target_dir: Path) -> tuple[Path, ...]:
    if not target_dir.exists():
        return ()
    state_root = target_dir / ".molt_state"
    found: list[Path] = []
    try:
        candidates = tuple(target_dir.rglob("incremental"))
    except OSError:
        raise
    for candidate in candidates:
        if not candidate.is_dir():
            continue
        with contextlib.suppress(ValueError):
            if candidate.is_relative_to(state_root):
                continue
        found.append(candidate)
    return tuple(sorted(found, key=lambda p: p.relative_to(target_dir).parts))


def _cargo_quarantine_parent(target_dir: Path) -> Path:
    return target_dir / ".molt_state" / "quarantine" / "cargo_incremental"


def _cargo_quarantine_id(recorded_at: str, pid: int, reason: str) -> str:
    safe_time = (
        recorded_at.replace(":", "").replace("-", "").replace("T", "-").replace("Z", "")
    )
    safe_reason = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in reason)
    return f"{safe_time}-pid{pid}-{safe_reason}"


def _prune_cargo_incremental_quarantine(
    parent: Path,
    *,
    keep: int = DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    if keep <= 0 or not parent.exists():
        return (), ()

    def mtime_ns(path: Path) -> int:
        try:
            return path.stat().st_mtime_ns
        except OSError:
            return 0

    try:
        roots = sorted(
            (path for path in parent.iterdir() if path.is_dir()),
            key=mtime_ns,
            reverse=True,
        )
    except OSError as exc:
        return (), (f"{parent}: {exc}",)
    pruned: list[str] = []
    errors: list[str] = []
    for stale in roots[keep:]:
        try:
            shutil.rmtree(stale)
        except OSError as exc:
            errors.append(f"{stale}: {exc}")
            continue
        pruned.append(str(stale))
    return tuple(pruned), tuple(errors)


def _cargo_incremental_quarantine_payload(
    receipt: CargoIncrementalQuarantine | None,
) -> dict[str, object] | None:
    if receipt is None:
        return None
    return {
        "reason": receipt.reason,
        "recorded_at": receipt.recorded_at,
        "target_dir": receipt.target_dir,
        "quarantine_dir": receipt.quarantine_dir,
        "command": list(receipt.command),
        "cwd": receipt.cwd,
        "moved_paths": [
            {
                "original_path": move.original_path,
                "quarantined_path": move.quarantined_path,
            }
            for move in receipt.moved_paths
        ],
        "pruned_quarantine_dirs": list(receipt.pruned_quarantine_dirs),
        "errors": list(receipt.errors),
        "receipt_path": receipt.receipt_path,
    }


def _write_cargo_quarantine_receipt(
    *,
    receipt_path: Path,
    payload: Mapping[str, object],
) -> None:
    receipt_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _quarantine_cargo_incremental_state(
    *,
    reason: str,
    target_dir: Path,
    command: Sequence[str],
    cwd: str | Path,
    retention_keep: int = DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP,
) -> CargoIncrementalQuarantine:
    recorded_at = _utc_timestamp()
    errors: list[str] = []
    try:
        incremental_dirs = _cargo_incremental_dirs(target_dir)
    except OSError as exc:
        incremental_dirs = ()
        errors.append(f"{target_dir}: failed to scan Cargo incremental dirs: {exc}")
    quarantine_dir: Path | None = None
    moved: list[CargoIncrementalQuarantineMove] = []
    receipt_path: Path | None = None

    if incremental_dirs:
        parent = _cargo_quarantine_parent(target_dir)
        quarantine_dir = parent / _cargo_quarantine_id(
            recorded_at,
            os.getpid(),
            reason,
        )
        for source in incremental_dirs:
            try:
                destination = quarantine_dir / source.relative_to(target_dir)
                destination.parent.mkdir(parents=True, exist_ok=True)
                source.rename(destination)
            except OSError as exc:
                errors.append(f"{source}: {exc}")
                continue
            moved.append(
                CargoIncrementalQuarantineMove(
                    original_path=str(source),
                    quarantined_path=str(destination),
                )
            )
        if quarantine_dir.exists():
            receipt_path = quarantine_dir / "receipt.json"

    pruned, prune_errors = _prune_cargo_incremental_quarantine(
        _cargo_quarantine_parent(target_dir),
        keep=retention_keep,
    )
    errors.extend(prune_errors)
    receipt = CargoIncrementalQuarantine(
        reason=reason,
        recorded_at=recorded_at,
        target_dir=str(target_dir),
        quarantine_dir=None if quarantine_dir is None else str(quarantine_dir),
        command=tuple(command),
        cwd=str(cwd),
        moved_paths=tuple(moved),
        pruned_quarantine_dirs=pruned,
        errors=tuple(errors),
        receipt_path=None if receipt_path is None else str(receipt_path),
    )
    if receipt_path is not None:
        try:
            _write_cargo_quarantine_receipt(
                receipt_path=receipt_path,
                payload=_cargo_quarantine_payload_required(receipt),
            )
        except OSError as exc:
            receipt = CargoIncrementalQuarantine(
                reason=receipt.reason,
                recorded_at=receipt.recorded_at,
                target_dir=receipt.target_dir,
                quarantine_dir=receipt.quarantine_dir,
                command=receipt.command,
                cwd=receipt.cwd,
                moved_paths=receipt.moved_paths,
                pruned_quarantine_dirs=receipt.pruned_quarantine_dirs,
                errors=(*receipt.errors, f"{receipt_path}: {exc}"),
                receipt_path=receipt.receipt_path,
            )
    return receipt


def _cargo_quarantine_payload_required(
    receipt: CargoIncrementalQuarantine,
) -> dict[str, object]:
    payload = _cargo_incremental_quarantine_payload(receipt)
    assert payload is not None
    return payload


def _cargo_incremental_quarantine_message(
    receipt: CargoIncrementalQuarantine,
) -> str:
    moved_count = len(receipt.moved_paths)
    error_count = len(receipt.errors)
    if moved_count:
        base = (
            "memory_guard: quarantined Cargo incremental state after "
            f"{receipt.reason}: moved={moved_count} target_dir={receipt.target_dir} "
            f"quarantine_dir={receipt.quarantine_dir}"
        )
    else:
        base = (
            "memory_guard: checked Cargo incremental state after "
            f"{receipt.reason}: moved=0 target_dir={receipt.target_dir}"
        )
    if receipt.pruned_quarantine_dirs:
        base = f"{base} pruned={len(receipt.pruned_quarantine_dirs)}"
    if receipt.receipt_path:
        base = f"{base} receipt={receipt.receipt_path}"
    if error_count:
        base = f"{base} errors={error_count}"
    return base
