"""Compact one-capture toolchain custody and frozen-manifest verification."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import tempfile
import time
from typing import Mapping, Sequence

from tools.command_execution import CommandExecutor
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg.process_image_capture import (
    capture_image,
    revalidate_images,
)


CAPTURE_SCHEMA = "molt.proof-toolchain-capture.v1"
VERIFICATION_SCHEMA = "molt.proof-toolchain-verification.v1"
_COMMANDS = CommandExecutor.for_file(__file__)


@dataclass(frozen=True)
class FrozenFile:
    path: str
    sha256: str
    size: int | None

    def as_dict(self) -> dict[str, object]:
        return {"path": self.path, "sha256": self.sha256, "size": self.size}


def _resolve_executable(value: str, env: Mapping[str, str]) -> Path:
    candidate = Path(value)
    if candidate.is_absolute():
        return candidate.resolve(strict=True)
    resolved = shutil.which(value, path=env.get("PATH"))
    if resolved is None:
        raise ValueError(f"selected process image is unavailable: {value!r}")
    return Path(resolved).resolve(strict=True)


def _command_tokens(line: str) -> list[str]:
    """Decode rustc's quoted command-debug output without platform guessing."""
    tokens: list[str] = []
    decoder = json.JSONDecoder()
    index = 0
    while index < len(line):
        while index < len(line) and line[index].isspace():
            index += 1
        if index >= len(line):
            break
        if line[index] != '"':
            return shlex.split(line, posix=os.name != "nt")
        value, end = decoder.raw_decode(line, index)
        if not isinstance(value, str):
            raise ValueError("rust linker command contains a non-string argument")
        tokens.append(value)
        index = end
    return tokens


def _selected_command_lines(output: str) -> list[list[str]]:
    commands: list[list[str]] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped.startswith('"'):
            continue
        try:
            tokens = _command_tokens(stripped)
        except (ValueError, json.JSONDecodeError):
            continue
        if tokens:
            commands.append(tokens)
    return commands


def capture_rust_link_process_images(
    *,
    rustc: Path,
    cargo: Path | None,
    cwd: Path,
    env: Mapping[str, str],
    target: str | None,
    command_argv: Sequence[str] = (),
    linker_process_helpers: Mapping[str, Sequence[str]] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Ask the selected Rust toolchain to link a zero-dependency synthetic crate.

    The probe runs outside the repository source/target and uses the exact captured
    environment and Cargo configuration. It executes no repository build script.
    `--print link-args` supplies the actual selected driver argv; compiler-driver
    dry-run output then exposes internally selected linker helpers such as mold.
    """
    started = time.perf_counter()
    probe_env = dict(env)
    with tempfile.TemporaryDirectory(prefix="molt-rust-link-capture-") as raw_root:
        root = Path(raw_root).resolve()
        source = root / "main.rs"
        source.write_text("fn main() {}\n#[test]\nfn proof_link_test() {}\n", encoding="utf-8")
        output = root / ("probe.exe" if os.name == "nt" else "probe")
        if cargo is not None:
            manifest = root / "Cargo.toml"
            feature_names: set[str] = set()
            custom_profiles: set[str] = set()
            cargo_profile_args: list[str] = []
            rustc_link_args: list[str] = []
            command = [str(value) for value in command_argv]
            try:
                separator = command.index("--")
            except ValueError:
                separator = len(command)
            cargo_args = command[1:separator]
            forwarded = command[separator + 1 :] if separator < len(command) else []
            index = 0
            while index < len(cargo_args):
                value = cargo_args[index]
                if value in {"--release", "--all-features", "--no-default-features"}:
                    cargo_profile_args.append(value)
                    index += 1
                    continue
                if value in {"--profile", "--features", "-F"}:
                    if index + 1 >= len(cargo_args):
                        raise ValueError(f"Cargo {value} requires a value")
                    argument = cargo_args[index + 1]
                    if value == "--profile":
                        cargo_profile_args.extend((value, argument))
                        if argument not in {"dev", "release", "test", "bench"}:
                            if not argument or not all(
                                character.isalnum() or character in "_-"
                                for character in argument
                            ):
                                raise ValueError("Cargo profile name is not canonical")
                            custom_profiles.add(argument)
                    else:
                        selected_features: list[str] = []
                        for feature in argument.replace(",", " ").split():
                            local = feature.rsplit("/", 1)[-1]
                            if local and all(
                                character.isalnum() or character in "_-"
                                for character in local
                            ):
                                feature_names.add(local)
                                selected_features.append(local)
                        cargo_profile_args.extend(
                            (value, ",".join(selected_features))
                        )
                    index += 2
                    continue
                if value.startswith("--profile="):
                    cargo_profile_args.append(value)
                    profile = value.split("=", 1)[1]
                    if profile not in {"dev", "release", "test", "bench"}:
                        if not profile or not all(
                            character.isalnum() or character in "_-"
                            for character in profile
                        ):
                            raise ValueError("Cargo profile name is not canonical")
                        custom_profiles.add(profile)
                elif value.startswith("--features="):
                    raw_features = value.split("=", 1)[1]
                    selected_features = []
                    for feature in raw_features.replace(",", " ").split():
                        local = feature.rsplit("/", 1)[-1]
                        if local and all(
                            character.isalnum() or character in "_-"
                            for character in local
                        ):
                            feature_names.add(local)
                            selected_features.append(local)
                    cargo_profile_args.append(
                        "--features=" + ",".join(selected_features)
                    )
                index += 1
            index = 0
            while index < len(forwarded):
                value = forwarded[index]
                if value == "-C":
                    if index + 1 >= len(forwarded):
                        raise ValueError("rustc -C requires a value")
                    rustc_link_args.extend((value, forwarded[index + 1]))
                    index += 2
                    continue
                if value == "--crate-type":
                    if index + 1 >= len(forwarded):
                        raise ValueError("rustc --crate-type requires a value")
                    rustc_link_args.extend((value, forwarded[index + 1]))
                    index += 2
                    continue
                if value.startswith("-C") or value.startswith("--crate-type="):
                    rustc_link_args.append(value)
                index += 1
            feature_table = "".join(
                f'{json.dumps(name)}=[]\n' for name in sorted(feature_names)
            )
            profile_table = "".join(
                f'\n[profile.{name}]\ninherits="release"\n'
                for name in sorted(custom_profiles)
            )
            manifest.write_text(
                '[package]\nname="molt_link_capture"\nversion="0.0.0"\n'
                'edition="2024"\npublish=false\n\n[[bin]]\nname="molt_link_capture"\n'
                'path="main.rs"\n\n[features]\n' + feature_table + profile_table,
                encoding="utf-8",
            )
            probe_env["CARGO_TARGET_DIR"] = str(root / "target")
            probe_env["CARGO_INCREMENTAL"] = "0"
            command = [
                str(cargo),
                "rustc",
                "--quiet",
                "--offline",
                "--manifest-path",
                str(manifest),
            ]
            if target:
                command.extend(("--target", target))
            command.extend(cargo_profile_args)
            command.extend(("--", *rustc_link_args, "--print", "link-args"))
        else:
            command = [
                str(rustc),
                str(source),
                "--crate-name",
                "molt_link_capture",
                "--print",
                "link-args",
                "-o",
                str(output),
            ]
            if target:
                command.extend(("--target", target))
            direct = [str(value) for value in command_argv[1:]]
            index = 0
            while index < len(direct):
                value = direct[index]
                if value == "-C" and index + 1 < len(direct):
                    command.extend((value, direct[index + 1]))
                    index += 2
                    continue
                if value == "--crate-type":
                    if index + 1 >= len(direct):
                        raise ValueError("rustc --crate-type requires a value")
                    command.extend((value, direct[index + 1]))
                    index += 2
                    continue
                if value.startswith("-C") or value.startswith("--crate-type="):
                    command.append(value)
                index += 1
        completed = _COMMANDS.run(
            command,
            cwd=cwd,
            env=probe_env,
            check=False,
            capture_output=True,
            text=True,
            timeout=120.0,
        )
        if completed.returncode != 0:
            raise ValueError(
                "synthetic Rust linker selection failed: "
                + (completed.stderr.strip() or completed.stdout.strip())
            )
        commands = _selected_command_lines(completed.stdout)
        if len(commands) != 1:
            raise ValueError(
                f"synthetic Rust linker selection returned {len(commands)} commands"
            )
        selected = commands[0]
        primary = _resolve_executable(selected[0], probe_env)
        selected_paths = [primary]
        driver_name = primary.name.casefold()
        if any(token in driver_name for token in ("clang", "gcc", "cc", "c++")):
            dry_run = _COMMANDS.run(
                [str(primary), "-###", *selected[1:]],
                cwd=cwd,
                env=probe_env,
                check=False,
                capture_output=True,
                text=True,
                timeout=30.0,
            )
            nested_commands = _selected_command_lines(
                dry_run.stdout + "\n" + dry_run.stderr
            )
            if dry_run.returncode != 0 or not nested_commands:
                raise ValueError(
                    "selected compiler driver did not expose its exact helper commands"
                )
            for nested in nested_commands:
                selected_paths.append(_resolve_executable(nested[0], probe_env))
        helper_policy = {
            str(linker).casefold(): tuple(str(helper) for helper in helpers)
            for linker, helpers in (linker_process_helpers or {}).items()
        }
        declared_helpers = helper_policy.get(driver_name, ())
        selected_helpers: list[Path] = []
        for helper_name in declared_helpers:
            if Path(helper_name).name != helper_name:
                raise ValueError("Rust linker helper policy requires basenames")
            helper = primary.with_name(helper_name)
            if helper.is_file():
                selected_helpers.append(helper.resolve(strict=True))
        selected_paths.extend(selected_helpers)
        unique_paths = list(dict.fromkeys(selected_paths))
        images = []
        auxiliary_keys = {os.path.normcase(str(path)) for path in selected_helpers}
        for index, path in enumerate(unique_paths):
            image = capture_image(
                "rust-linker" if index == 0 else "rust-link-helper",
                path,
                root_exit_disposition=(
                    "terminate"
                    if os.path.normcase(str(path)) in auxiliary_keys
                    else "require-exit"
                ),
            )
            images.append(image)
        telemetry = {
            "schema": "molt.proof-rust-link-selection-telemetry.v1",
            "target": target,
            "probe": "cargo-rustc" if cargo is not None else "rustc",
            "selected_process_count": len(images),
            "selection_probe_count": 1,
            "declared_helper_count": len(declared_helpers),
            "selected_helper_count": len(selected_helpers),
            "helper_policy_sha256": hashlib.sha256(
                json.dumps(
                    {
                        linker: list(helpers)
                        for linker, helpers in sorted(helper_policy.items())
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest(),
            "command_semantics_sha256": hashlib.sha256(
                json.dumps(
                    [str(value) for value in command_argv], separators=(",", ":")
                ).encode()
            ).hexdigest(),
            "link_argv_sha256": hashlib.sha256(
                json.dumps(selected, separators=(",", ":")).encode()
            ).hexdigest(),
            "capture_s": time.perf_counter() - started,
        }
        return images, telemetry


def revalidate_rust_link_process_images(
    selected_identity: Mapping[str, object],
    *,
    target: str | None,
    command_argv: Sequence[str] = (),
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Rehash the one pre-arm linker selection without selecting through PATH again."""
    raw_telemetry = selected_identity.get("link_selection")
    if not isinstance(raw_telemetry, Mapping):
        raise ValueError("pre-arm Rust linker selection telemetry is unavailable")
    telemetry = dict(raw_telemetry)
    if telemetry.get("schema") != "molt.proof-rust-link-selection-telemetry.v1":
        raise ValueError("pre-arm Rust linker selection telemetry schema mismatch")
    if telemetry.get("selection_probe_count") != 1:
        raise ValueError("Rust linker selection must execute exactly once pre-arm")
    if telemetry.get("target") != target:
        raise ValueError("Rust linker target changed while live custody armed")
    command_semantics_sha256 = hashlib.sha256(
        json.dumps(
            [str(value) for value in command_argv], separators=(",", ":")
        ).encode()
    ).hexdigest()
    if telemetry.get("command_semantics_sha256") != command_semantics_sha256:
        raise ValueError("Rust linker command semantics changed while live custody armed")

    raw_images = selected_identity.get("process_images")
    if not isinstance(raw_images, list):
        raise ValueError("pre-arm Rust process-image selection is unavailable")
    selected_rows: list[Mapping[str, object]] = []
    for raw in raw_images:
        if not isinstance(raw, Mapping):
            raise ValueError("pre-arm Rust process-image row is malformed")
        role = raw.get("role")
        if role not in {"rust-linker", "rust-link-helper"}:
            continue
        selected_rows.append(raw)
    try:
        selected = revalidate_images(selected_rows)
    except ValueError as exc:
        raise ValueError(
            f"Rust linker process image changed while live custody armed: {exc}"
        ) from exc
    if not selected:
        raise ValueError("pre-arm Rust linker selection captured no process image")
    if telemetry.get("selected_process_count") != len(selected):
        raise ValueError("pre-arm Rust linker process count is inconsistent")
    return selected, telemetry


def _digest(value: object) -> str | None:
    return value if isinstance(value, str) and len(value) == 64 else None


def frozen_files(payload: object) -> list[FrozenFile]:
    """Project every captured file row into one deduplicated content manifest."""
    files: dict[str, FrozenFile] = {}

    def add(raw_path: object, raw_digest: object, raw_size: object = None) -> None:
        digest = _digest(raw_digest)
        if not isinstance(raw_path, str) or digest is None:
            return
        path = Path(raw_path)
        if not path.is_absolute():
            return
        normalized = os.path.normcase(os.path.abspath(path))
        size = raw_size if isinstance(raw_size, int) and raw_size >= 0 else None
        row = FrozenFile(str(path), digest, size)
        prior = files.get(normalized)
        if prior is not None and prior.sha256 != digest:
            raise ValueError(f"captured file has conflicting identities: {path}")
        files[normalized] = row if prior is None or prior.size is None else prior

    def visit(value: object) -> None:
        if isinstance(value, Mapping):
            path = value.get("resolved_path") or value.get("path")
            add(path, value.get("sha256"), value.get("size", value.get("size_bytes")))
            add(value.get("executable"), value.get("executable_sha256"))
            add(value.get("path"), value.get("launcher_sha256"))
            add(value.get("content_path"), value.get("executable_sha256"))
            for nested in value.values():
                visit(nested)
        elif isinstance(value, (list, tuple)):
            for nested in value:
                visit(nested)

    visit(payload)
    return [files[key] for key in sorted(files)]


def _compact_python(identity: Mapping[str, object]) -> dict[str, object]:
    compact = {
        key: value
        for key, value in identity.items()
        if key not in {"runtime", "distributions", "inventory_profile"}
        and not isinstance(value, (dict, list))
    }
    runtime = identity.get("runtime")
    if isinstance(runtime, Mapping):
        compact["runtime"] = {
            key: value
            for key, value in runtime.items()
            if not isinstance(value, (dict, list))
        }
    distributions = identity.get("distributions")
    compact_distributions: list[dict[str, object]] = []
    if isinstance(distributions, list):
        for distribution in distributions:
            if not isinstance(distribution, Mapping):
                continue
            row = {
                key: distribution.get(key)
                for key in (
                    "name",
                    "version",
                    "file_manifest_sha256",
                    "direct_url_sha256",
                    "record_sha256",
                )
                if distribution.get(key) is not None
            }
            editable = distribution.get("editable_source")
            if isinstance(editable, Mapping):
                row["editable_source"] = {
                    key: value
                    for key, value in editable.items()
                    if key != "files" and not isinstance(value, (dict, list))
                }
                compact_distributions.append(row)
    compact["distributions"] = compact_distributions
    profile = identity.get("inventory_profile")
    compact["inventory_profile"] = dict(profile) if isinstance(profile, Mapping) else {}
    return compact


def compact_toolchains(toolchains: Mapping[str, object]) -> dict[str, object]:
    summaries: dict[str, object] = {}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError(f"toolchain {name!r} has malformed identity")
        if name == "python":
            summaries[name] = _compact_python(raw)
        else:
            summary = {
                key: value
                for key, value in raw.items()
                if not isinstance(value, (dict, list))
            }
            process_images = raw.get("process_images")
            if isinstance(process_images, list):
                summary["process_images"] = process_images
            link_selection = raw.get("link_selection")
            if isinstance(link_selection, Mapping):
                summary["link_selection"] = dict(link_selection)
            summaries[name] = summary
    return summaries


def publish_capture(
    cas_root: Path, toolchains: Mapping[str, object]
) -> tuple[dict[str, object], dict[str, object], dict[str, object]]:
    started = time.perf_counter()
    files = frozen_files(toolchains)
    artifact_payload: dict[str, object] = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": CAPTURE_SCHEMA,
        "toolchains": dict(toolchains),
        "files": [row.as_dict() for row in files],
    }
    reference = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    summaries = compact_toolchains(toolchains)
    telemetry = {
        "schema": "molt.proof-toolchain-capture-telemetry.v1",
        "full_capture_count": 1,
        "frozen_file_count": len(files),
        "artifact_compressed_bytes": reference["compressed_bytes"],
        "artifact_uncompressed_bytes": reference["uncompressed_bytes"],
        "publish_s": time.perf_counter() - started,
    }
    return summaries, reference, telemetry


def load_capture(
    reference: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    payload = custody_cas.read_ref(reference, expected_root=cas_root)
    if payload.get("kind") != CAPTURE_SCHEMA:
        raise ValueError("proof toolchain capture artifact kind mismatch")
    toolchains = payload.get("toolchains")
    files = payload.get("files")
    if not isinstance(toolchains, dict) or not isinstance(files, list):
        raise ValueError("proof toolchain capture artifact is incomplete")
    return payload


def _rehash(row: Mapping[str, object]) -> tuple[str, str | None, int | None, str | None]:
    raw_path = row.get("path")
    expected = row.get("sha256")
    if not isinstance(raw_path, str) or not isinstance(expected, str):
        return str(raw_path), None, None, "malformed-row"
    path = Path(raw_path)
    try:
        stat = path.stat()
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError as exc:
        return raw_path, None, None, type(exc).__name__
    return raw_path, actual, stat.st_size, None


def verify_capture(
    reference: Mapping[str, object], *, workers: int, cas_root: Path
) -> dict[str, object]:
    started = time.perf_counter()
    payload = load_capture(reference, cas_root=cas_root)
    rows = payload["files"]
    assert isinstance(rows, list)
    if workers < 1:
        raise ValueError("toolchain verification workers must be positive")
    with ThreadPoolExecutor(max_workers=workers) as executor:
        actual = list(executor.map(_rehash, rows))
    mismatches: list[dict[str, object]] = []
    bytes_hashed = 0
    for row, (path, digest, size, error) in zip(rows, actual, strict=True):
        assert isinstance(row, Mapping)
        if isinstance(size, int):
            bytes_hashed += size
        expected_size = row.get("size")
        if (
            error is not None
            or digest != row.get("sha256")
            or (isinstance(expected_size, int) and size != expected_size)
        ):
            mismatches.append(
                {
                    "path": path,
                    "expected_sha256": row.get("sha256"),
                    "actual_sha256": digest,
                    "expected_size": expected_size,
                    "actual_size": size,
                    "error": error,
                }
            )
    material = {
        "capture_semantic_sha256": reference.get("semantic_sha256"),
        "verified_file_count": len(rows),
        "bytes_hashed": bytes_hashed,
        "mismatches": mismatches,
    }
    return {
        "schema": VERIFICATION_SCHEMA,
        **material,
        "stable": not mismatches,
        "verification_s": time.perf_counter() - started,
        "identity_sha256": hashlib.sha256(
            json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
    }
