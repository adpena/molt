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
import re
import shutil
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

from molt import file_publication
from molt.cli.command_runtime import _run_completed_command
from molt.cli.native_link_command import (
    _build_native_runtime_image_link_plan,
    _build_native_program_image_link_plan,
    runtime_image_suffix,
)
from molt.cli.native_link_plan import (
    NativeLinkPlan,
    NativeLinkSidecar,
    image_generation_witness,
    SharedStdlibImage,
    native_link_execution_command,
)
from molt.cli.native_symbol_inspection import (
    _native_archive_global_symbol_facts,
    _native_object_global_symbol_facts,
)
from molt.cli.native_link_tool_identity import native_link_cache_tool_facts
from molt.cli.product_store import ProductStore
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.native_target import NativeArtifactKind, NativeTargetSpec

RUNTIME_IMAGE_SCHEMA = "molt.jit-runtime-image.v2"
STDLIB_IMAGE_SCHEMA = "molt.stdlib-image.v1"
_PLANNED_GENERATION = "0" * 64
_NATIVE_IDENTIFIER = re.compile(r"[a-zA-Z_][a-zA-Z0-9_]*", re.ASCII)
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
    return _ensure_image(
        store,
        stem=Path(name).stem,
        suffix=Path(name).suffix,
        identity={
            "schema": RUNTIME_IMAGE_SCHEMA,
            "archive_sha256": binding.archive.sha256,
            "target": target.triple,
            "profile": profile,
        },
        archive=binding.runtime_lib,
        link_timeout=link_timeout,
        plan_for=lambda output: _build_native_runtime_image_link_plan(
            runtime_lib=binding.runtime_lib,
            output_image=output,
            target=target,
            sysroot_path=sysroot_path,
            profile=profile,
            runtime_codegen_binding=binding,
        ),
    )


def ensure_shared_stdlib_image(
    *,
    stdlib_archive: Path,
    program_artifact: Path,
    program_kind: NativeArtifactKind,
    runtime_image: Path,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    link_timeout: float | None,
) -> SharedStdlibImage:
    # Link the existing stdlib archive against its runtime, once.
    stdlib = _native_archive_global_symbol_facts(
        stdlib_archive, target_triple=target.triple
    )
    program_symbols = (
        _native_archive_global_symbol_facts
        if program_kind is NativeArtifactKind.ARCHIVE
        else _native_object_global_symbol_facts
    )
    program = program_symbols(program_artifact, target_triple=target.triple)
    # A reverse link edge is allowed only for program-owned storage. The
    # backend has already validated closure over compiled stdlib functions.
    imports = (stdlib.undefined - stdlib.defined) & program.defined
    invalid = sorted(
        name
        for name in imports
        if name in program.defined_functions or not _NATIVE_IDENTIFIER.fullmatch(name)
    )
    if invalid:
        raise ValueError(
            f"Invalid application data import for the stdlib image: {invalid[0]}"
        )
    if stdlib.artifact_digest is None:
        raise ValueError(
            f"Shared stdlib archive has no admitted digest: {stdlib_archive}"
        )
    program_data = tuple(sorted(imports))
    store = ProductStore(runtime_image.parent.parent)
    path = _ensure_image(
        store,
        stem="libmolt_stdlib",
        suffix=runtime_image_suffix(target),
        identity={
            "schema": STDLIB_IMAGE_SCHEMA,
            "archive_sha256": stdlib.artifact_digest,
            "runtime_image": str(runtime_image),
            "target": target.triple,
            "profile": profile,
        },
        archive=stdlib_archive,
        link_timeout=link_timeout,
        plan_for=lambda output: _build_native_program_image_link_plan(
            input_artifact=stdlib_archive,
            input_kind=NativeArtifactKind.ARCHIVE,
            runtime_image=runtime_image,
            image=SharedStdlibImage(output, program_data),
            target=target,
            sysroot_path=sysroot_path,
            profile=profile,
        ),
    )
    return SharedStdlibImage(path, program_data)


def ensure_jit_program_data_image(
    *,
    store: ProductStore,
    program_storage: Path,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    link_timeout: float | None,
) -> Path:
    storage = _native_object_global_symbol_facts(
        program_storage, target_triple=target.triple
    )
    if (
        storage.artifact_digest is None
        or storage.defined_functions
        or storage.undefined
    ):
        raise ValueError(f"Invalid JIT application storage object: {program_storage}")
    return _ensure_image(
        store,
        stem="libmolt_program",
        suffix=runtime_image_suffix(target),
        identity={
            "schema": "molt.jit-program-data-image.v1",
            "program_storage": storage.artifact_digest,
            "target": target.triple,
            "profile": profile,
        },
        archive=program_storage,
        link_timeout=link_timeout,
        plan_for=lambda output: _build_native_program_image_link_plan(
            input_artifact=program_storage,
            input_kind=NativeArtifactKind.OBJECT,
            runtime_image=None,
            image=SharedStdlibImage(output, ()),
            target=target,
            sysroot_path=sysroot_path,
            profile=profile,
        ),
    )


def _image_plan_with_witness(plan: NativeLinkPlan, output: Path) -> NativeLinkPlan:
    source = output.with_name("image_generation.c")
    sidecar = NativeLinkSidecar(
        role="image-generation-witness",
        planned_path=source,
        content=f"const unsigned char {image_generation_witness(output)} = 0;\n".encode(),
        command_index=len(plan.command),
        operand_prefix="",
    )
    return replace(
        plan,
        command=(*plan.command, str(source)),
        sidecars=(*plan.sidecars, sidecar),
    )


def _ensure_image(
    store: ProductStore,
    *,
    stem: str,
    suffix: str,
    identity: dict[str, str],
    archive: Path,
    link_timeout: float | None,
    plan_for: Callable[[Path], NativeLinkPlan],
) -> Path:
    def image_path(key: str) -> Path:
        return store.entry(key) / f"{stem}_{key}{suffix}"

    planned = image_path(_PLANNED_GENERATION)
    plan = _image_plan_with_witness(plan_for(planned), planned)
    key = hashlib.sha256(
        json.dumps(
            {
                **identity,
                "command": [
                    argument.replace(str(archive), "<archive>")
                    for argument in plan.command
                ],
                "sidecars": plan.sidecar_facts(),
                "tools": native_link_cache_tool_facts(plan),
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    image = image_path(key)

    def build() -> Path:
        _link_image(
            _image_plan_with_witness(plan_for(image), image), image, image, link_timeout
        )
        store.retain(key)
        return image

    return store.ensure(key, lambda: image if image.is_file() else None, build)


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
