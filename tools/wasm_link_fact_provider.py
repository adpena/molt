"""Rust WASM-facts scan, decode, and publication authority."""

from __future__ import annotations

from collections.abc import Callable, Mapping
import contextlib
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
from typing import Any

from wasm_link_format import CallableTableLayout


_WASM_LINK_FACTS_SCHEMA_VERSION = 4


def _decode_wasm_facts_response(
    process: subprocess.CompletedProcess[str],
    *,
    operation: str,
) -> dict[str, object]:
    try:
        payload = json.loads(process.stdout)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{operation} returned invalid JSON: {exc}") from exc
    if (
        not isinstance(payload, dict)
        or payload.get("schema_version") != _WASM_LINK_FACTS_SCHEMA_VERSION
    ):
        raise ValueError(f"{operation} returned an unsupported response schema")
    if process.returncode != 0 or payload.get("ok") is not True:
        error = payload.get("error")
        detail = error if isinstance(error, str) and error else process.stderr.strip()
        raise ValueError(f"{operation} failed: {detail or 'unknown scanner error'}")
    facts = payload.get("facts")
    if (
        not isinstance(facts, dict)
        or facts.get("schema_version") != _WASM_LINK_FACTS_SCHEMA_VERSION
    ):
        raise ValueError(f"{operation} returned an unsupported facts schema")
    return facts


def make_rust_wasm_facts_provider(
    api: Mapping[str, Any],
    scanner: Path,
    scratch_root: Path,
    metrics: dict[str, float] | None = None,
) -> Callable[[bytes], dict[str, object]]:
    if not scanner.is_file():
        raise ValueError(f"WASM facts scanner is not a file: {scanner}")
    try:
        resolved_scanner = scanner.resolve(strict=True)
        scanner_stat_identity = api["_file_stat_identity"](resolved_scanner.stat())
    except OSError as exc:
        raise ValueError(f"WASM facts scanner is unreadable: {scanner}") from exc
    scanner_sha256 = api["_stable_file_sha256_cached"](
        os.fspath(resolved_scanner),
        scanner_stat_identity,
    )
    if scanner_sha256 is None:
        raise ValueError(f"WASM facts scanner changed during identity read: {scanner}")
    provider_authority = hashlib.sha256(
        (
            f"molt-wasm-link-facts-schema:{_WASM_LINK_FACTS_SCHEMA_VERSION}\0"
            f"{scanner_sha256}"
        ).encode("ascii")
    ).hexdigest()
    cache: dict[str, dict[str, object]] = {}
    if metrics is not None:
        metrics.update(
            {
                "wasm_facts_hash_ms": 0.0,
                "wasm_facts_scan_ms": 0.0,
                "wasm_facts_scan_calls": 0.0,
                "wasm_facts_cache_hits": 0.0,
                "wasm_facts_input_bytes": 0.0,
                "wasm_facts_response_chars": 0.0,
            }
        )

    def provide(data: bytes) -> dict[str, object]:
        hash_start = time.perf_counter()
        digest = hashlib.sha256(data).hexdigest()
        if metrics is not None:
            metrics["wasm_facts_hash_ms"] += max(
                0.0, (time.perf_counter() - hash_start) * 1000.0
            )
        cached = cache.get(digest)
        if cached is not None:
            if metrics is not None:
                metrics["wasm_facts_cache_hits"] += 1.0
            return cached
        try:
            if (
                api["_file_stat_identity"](resolved_scanner.stat())
                != scanner_stat_identity
            ):
                raise ValueError("WASM facts scanner identity changed before execution")
        except OSError as exc:
            raise ValueError("WASM facts scanner disappeared before execution") from exc
        artifact = scratch_root / f"wasm-facts-{digest}.wasm"
        artifact.write_bytes(data)
        scan_start = time.perf_counter()
        try:
            process = api["_COMMANDS"].run(
                [str(resolved_scanner), "--scan-wasm-link-facts", str(artifact)],
                text=True,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                check=False,
            )
            if metrics is not None:
                metrics["wasm_facts_scan_ms"] += max(
                    0.0, (time.perf_counter() - scan_start) * 1000.0
                )
                metrics["wasm_facts_scan_calls"] += 1.0
                metrics["wasm_facts_input_bytes"] += float(len(data))
                metrics["wasm_facts_response_chars"] += float(len(process.stdout))
            facts = _decode_wasm_facts_response(
                process,
                operation=f"Rust WASM facts scan for {artifact.name}",
            )
            try:
                if (
                    api["_file_stat_identity"](resolved_scanner.stat())
                    != scanner_stat_identity
                ):
                    raise ValueError("WASM facts scanner changed during execution")
            except OSError as exc:
                raise ValueError(
                    "WASM facts scanner disappeared during execution"
                ) from exc
            cache[digest] = facts
            return facts
        finally:
            with contextlib.suppress(OSError):
                artifact.unlink()

    setattr(provide, "_molt_wasm_facts_authority_digest", provider_authority)
    return provide


def publish_rust_wasm_link_facts(
    api: Mapping[str, Any],
    scanner: Path,
    artifact: Path,
    *,
    layout: CallableTableLayout | None = None,
    role: str = "monolithic",
) -> dict[str, object]:
    if not scanner.is_file():
        raise ValueError(f"WASM facts scanner is not a file: {scanner}")
    command = [
        str(scanner),
        "--publish-wasm-link-facts",
        str(artifact),
        "--output",
        str(artifact),
    ]
    if layout is not None:
        command.extend(
            [
                "--callable-table-layout",
                ",".join(
                    str(value)
                    for value in (
                        layout.fixed_prefix_base,
                        layout.fixed_prefix_len,
                        layout.finalized_app_base,
                        layout.app_entry_count,
                    )
                ),
            ]
        )
    if role not in {"monolithic", "app", "runtime"}:
        raise ValueError(f"unknown callable-table artifact role: {role}")
    if role != "monolithic" and layout is None:
        raise ValueError(f"callable-table {role} publication requires a layout")
    command.extend(["--callable-table-role", role])
    process = api["_COMMANDS"].run(
        command,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    facts = _decode_wasm_facts_response(
        process,
        operation=f"Rust WASM facts publication for {artifact}",
    )
    if facts.get("callable_table_attestation_present") is not True:
        raise ValueError("Rust WASM facts publication omitted final attestation")
    return facts
