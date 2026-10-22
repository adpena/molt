"""The runtime as a shared image for the development JIT tier.

rustc_codegen_cranelift's JIT mode needs every dependency as a dynamic library.
The JIT runner likewise loads the Molt runtime as a shared image and resolves
generated code's imports in it. The image is linked from the exact AOT runtime
archive of the program's runtime generation, so JIT and AOT runs use the same
runtime bytes. One link per runtime generation is the whole cost: the image is
content-addressed and every later run reuses it. Images are products of the
host's product store (``product_store.py``, kind ``jit-runtime-image``), one
entry per image key, with that store's retention: every build and every cached
launch that uses an image renews its entry, and a publication evicts the
entries no build used for the idle window. An image is never written into the
runtime entry it is linked from, because published entries are immutable.
"""

from __future__ import annotations

import hashlib
import json
import shutil
from pathlib import Path

from molt import file_publication
from molt.cli.command_runtime import _run_completed_command
from molt.cli.native_link_command import (
    _build_native_runtime_image_link_plan,
    runtime_image_suffix,
)
from molt.cli.native_link_plan import NativeLinkPlan, native_link_execution_command
from molt.cli.native_link_tool_identity import native_link_cache_tool_facts
from molt.cli.product_store import ProductStore
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.native_target import NativeTargetSpec

RUNTIME_IMAGE_SCHEMA = "molt.jit-runtime-image.v1"
_PLANNED_OUTPUT_TOKEN = "<runtime-image>"
# A builder links here inside the entry, under the entry's build lock, so a
# directory a dead builder left is that lock holder's to clear.
_STAGING = ".staged"


def jit_runtime_image_store(project_root: Path) -> ProductStore:
    """The product store a JIT build of ``project_root`` keeps its images in."""
    return ProductStore.of("jit-runtime-image", project_root)


def _image_name(binding: NativeRuntimeCodegenBinding, target: NativeTargetSpec) -> str:
    archive = binding.runtime_lib.name
    stem = archive.removesuffix(".a").removesuffix(".lib")
    return stem + runtime_image_suffix(target)


def ensure_jit_runtime_image(
    *,
    store: ProductStore,
    binding: NativeRuntimeCodegenBinding,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    link_timeout: float | None,
) -> Path:
    """Return the runtime image for ``binding``, linking it once if needed."""
    binding.verify()
    name = _image_name(binding, target)
    planned = store.root / _PLANNED_OUTPUT_TOKEN / name
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
    image = store.entry(key) / name
    # Renewed before the read, so no eviction races it (design 56, 2.8).
    if store.record_use(key) and image.is_file():
        return image
    with store.single_flight(key):
        # A rival builder may have published while this one waited.
        if not image.is_file():
            _link_image(plan, planned, image, link_timeout)
    store.retain(key)
    return image


def _link_image(
    plan: NativeLinkPlan, planned: Path, image: Path, link_timeout: float | None
) -> None:
    """Link ``image`` and publish it whole; the caller holds its build lock."""
    staged = image.parent / _STAGING
    shutil.rmtree(staged, ignore_errors=True)
    staged.mkdir()
    try:
        with native_link_execution_command(
            plan, planned_output=planned, execution_output=staged / image.name
        ) as command:
            process = _run_completed_command(
                command,
                capture_output=True,
                env=None,
                cwd=None,
                timeout=link_timeout,
                memory_guard_prefix="MOLT_BUILD",
            )
        if process.returncode != 0 or not (staged / image.name).is_file():
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
        # A reader sees the image whole or not at all.
        file_publication.durable_publish_exclusive(staged / image.name, image)
    finally:
        shutil.rmtree(staged, ignore_errors=True)


def _text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value
