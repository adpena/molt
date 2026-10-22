"""The runtime as a shared image for the development JIT tier.

rustc_codegen_cranelift's JIT mode needs every dependency as a dynamic library.
The JIT runner likewise loads the Molt runtime as a shared image and resolves
generated code's imports in it. The image is linked from the exact AOT runtime
archive of the program's runtime generation, so JIT and AOT runs use the same
runtime bytes. One link per runtime generation is the whole cost: the image is
content-addressed and every later run reuses it. The images of one runtime
directory form a generation store (``generation_store.py``): every build and
every cached launch that uses an image renews it, and a publication evicts the
images no build used for the idle window.
"""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import uuid
from pathlib import Path

from molt import file_publication
from molt.cli.command_runtime import _run_completed_command
from molt.cli.generation_store import (
    GENERATION_STORE_IDLE_SECONDS,
    GenerationStoreLayout,
    generation_store,
)
from molt.cli.native_link_command import (
    _build_native_runtime_image_link_plan,
    runtime_image_suffix,
)
from molt.cli.native_link_plan import native_link_execution_command
from molt.cli.native_link_tool_identity import native_link_cache_tool_facts
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.file_deletion import delete_path
from molt.native_target import NativeTargetSpec

RUNTIME_IMAGE_SCHEMA = "molt.jit-runtime-image.v1"
_IMAGE_ROOT = ".molt-jit-runtime-images"
_PLANNED_OUTPUT_TOKEN = "<runtime-image>"
_ENTRY_RE = re.compile(r"^[0-9a-f]{64}$")


def _remove_image(entry: Path) -> None:
    """Remove one idle image directory and the image it holds."""
    removed, error = delete_path(entry)
    if not removed:
        raise OSError(error)


_RUNTIME_IMAGE_STORE = GenerationStoreLayout(
    label="JIT runtime image",
    lock_name=".jit-runtime-image-store.lock",
    ledger_name=".jit-runtime-image-store.json",
    ledger_schema="molt.jit-runtime-image-store.v1",
    entry=_ENTRY_RE,
    remove=_remove_image,
)


def record_jit_runtime_image_use(image: Path) -> None:
    """Renew the retention window of the runtime image a cached launch runs."""
    entry = image.parent
    if entry.parent.name != _IMAGE_ROOT or _ENTRY_RE.match(entry.name) is None:
        raise ValueError(f"not a JIT runtime image: {image}")
    with generation_store(entry.parent, _RUNTIME_IMAGE_STORE) as store:
        store.use(entry.name)


def _image_name(binding: NativeRuntimeCodegenBinding, target: NativeTargetSpec) -> str:
    archive = binding.runtime_lib.name
    stem = archive.removesuffix(".a").removesuffix(".lib")
    return stem + runtime_image_suffix(target)


def ensure_jit_runtime_image(
    *,
    binding: NativeRuntimeCodegenBinding,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    link_timeout: float | None,
) -> Path:
    """Return the runtime image for ``binding``, linking it once if needed."""
    binding.verify()
    root = binding.runtime_lib.parent / _IMAGE_ROOT
    name = _image_name(binding, target)
    planned = root / _PLANNED_OUTPUT_TOKEN / name
    plan = _build_native_runtime_image_link_plan(
        runtime_lib=binding.runtime_lib,
        output_image=planned,
        target=target,
        sysroot_path=sysroot_path,
        profile=profile,
        runtime_codegen_binding=binding,
    )
    key = hashlib.sha256(
        json.dumps(
            {
                "schema": RUNTIME_IMAGE_SCHEMA,
                "archive_sha256": binding.archive.sha256,
                "command": [
                    argument.replace(str(planned), _PLANNED_OUTPUT_TOKEN)
                    for argument in plan.command
                ],
                "tools": native_link_cache_tool_facts(plan),
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    destination = root / key
    image = destination / name
    root.mkdir(parents=True, exist_ok=True)
    with generation_store(root, _RUNTIME_IMAGE_STORE) as store:
        if destination.exists():
            if not image.is_file():
                raise ValueError(
                    f"JIT runtime image directory lacks its image: {image}"
                )
            store.use(key)
            return image
    # Link outside the store lock: other builds keep reading the store while
    # this one links.
    staged = root / f".{key}.{uuid.uuid4().hex}.staged"
    staged.mkdir()
    try:
        with native_link_execution_command(
            plan, planned_output=planned, execution_output=staged / name
        ) as command:
            process = _run_completed_command(
                command,
                capture_output=True,
                env=None,
                cwd=None,
                timeout=link_timeout,
                memory_guard_prefix="MOLT_BUILD",
            )
        if process.returncode != 0 or not (staged / name).is_file():
            detail = "\n".join(
                text
                for text in (
                    _text(process.stderr).strip(),
                    _text(process.stdout).strip(),
                )
                if text
            )
            raise ValueError(
                f"JIT runtime image link failed (exit {process.returncode}): "
                f"{' '.join(command)}" + (f"\n{detail}" if detail else "")
            )
        with generation_store(root, _RUNTIME_IMAGE_STORE) as store:
            try:
                file_publication.durable_publish_directory_exclusive(
                    staged, destination
                )
            except FileExistsError:
                # Another build published the same content-addressed image.
                shutil.rmtree(staged, ignore_errors=True)
            if not image.is_file():
                raise ValueError(
                    f"JIT runtime image publication lost its image: {image}"
                )
            store.select(key)
            store.evict_idle(GENERATION_STORE_IDLE_SECONDS)
    except BaseException:
        shutil.rmtree(staged, ignore_errors=True)
        raise
    return image


def _text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value
