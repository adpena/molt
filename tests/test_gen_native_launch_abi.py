"""The native launch ABI has one source (HF-371 review finding 12).

`runtime/native_launch_abi.toml` declares the runtime entry points a native
program's launch calls and their order. The AOT main stub and the JIT runner
both consume the generated projections; these tests prove the projections are
current, that the stub calls the entries in the declared order, and that the
generated C declarations type-check against the stub's calls.
"""

from __future__ import annotations

import importlib
import re
import shutil
from pathlib import Path

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli.native_launch_abi_generated import (
    NATIVE_LAUNCH_FINISH_ENTRIES,
    NATIVE_LAUNCH_START_SEQUENCE,
)
from molt.cli.native_main_stub import _render_native_main_stub
from tests.process_guard_common import run_guarded_test_process


def _gen():
    return importlib.import_module("tools.gen_native_launch_abi")


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.render_all(gen.load_entries(), gen.load_runner()).items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path} is stale; run `python tools/gen_native_launch_abi.py --write`"
        )


def test_the_projections_name_every_declared_entry_in_order() -> None:
    gen = _gen()
    entries = gen.load_entries()
    assert NATIVE_LAUNCH_START_SEQUENCE == tuple(
        entry.name for entry in entries if entry.phase == "start"
    )
    assert NATIVE_LAUNCH_FINISH_ENTRIES == tuple(
        entry.name for entry in entries if entry.phase == "finish"
    )
    rust = gen.OUT_RUST.read_text(encoding="utf-8")
    for entry in entries:
        assert re.search(rf"pub\(crate\) {entry.name}:\s+unsafe extern", rust)


def _stub(*, runtime_module_roots: tuple[Path, ...] = ()) -> str:
    return _render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
        compiled_source_paths=("/src/app.py",),
        runtime_module_roots=runtime_module_roots,
    )


def _body(stub: str, signature: str) -> str:
    start = stub.index(signature)
    return stub[start : stub.index("\n}\n", start)]


def test_the_aot_stub_calls_the_launch_entries_in_the_declared_order() -> None:
    stub = _stub()
    for name in (*NATIVE_LAUNCH_START_SEQUENCE, *NATIVE_LAUNCH_FINISH_ENTRIES):
        assert len(re.findall(rf"\bextern [^;]*\b{name}\(", stub)) == 1, name
    main = _body(stub, "int main(int argc, char** argv) {")
    positions = [main.index(f"{name}(") for name in NATIVE_LAUNCH_START_SEQUENCE]
    assert positions == sorted(positions)
    # The entry runs after the last start entry; the finish follows it.
    assert positions[-1] < main.index("molt_main();") < main.index("molt_finish()")
    finish = _body(stub, "static int molt_finish() {")
    assert finish.index("molt_runtime_exit(") < finish.index("molt_main_exit_status()")


@pytest.mark.skipif(shutil.which("cc") is None, reason="needs a C compiler")
@pytest.mark.parametrize("with_runtime_module_roots", [False, True])
def test_the_generated_declarations_type_check_against_the_stub(
    tmp_path: Path, with_runtime_module_roots: bool
) -> None:
    source = tmp_path / "main.c"
    roots = (tmp_path / "module roots",) if with_runtime_module_roots else ()
    source.write_text(_stub(runtime_module_roots=roots), encoding="utf-8")
    # Compile the source as delivered: no external feature-test defines may
    # mask missing declarations in its capability or module-root setters.
    result = run_guarded_test_process(
        ["cc", "-fsyntax-only", "-std=c11", "-Werror", "-Wall", str(source)]
    )
    assert result.returncode == 0, result.stderr


def test_the_runner_command_and_image_format_come_from_the_declaration() -> None:
    # The CLI and the runner read the runner's flags, its start receipt and
    # the image marker from the launch ABI's one declaration (HF-371 review v4).
    import tomllib

    from molt.cli import jit_image, run_tier

    gen = _gen()
    with gen.SOURCE.open("rb") as handle:
        declared = tomllib.load(handle)["runner"]
    assert run_tier.jit_runner_argv("runner", "app.jit", ["x"], start_receipt="r") == [
        "runner",
        declared["start_receipt_flag"],
        "r",
        declared["image_flag"],
        "app.jit",
        "x",
    ]
    assert run_tier.JIT_START_RECEIPT == declared["start_receipt"]
    assert jit_image.JIT_IMAGE_MARKER == declared["image_marker"]
    rust = gen.OUT_RUST.read_text(encoding="utf-8")
    for key, constant in (
        ("image_flag", "IMAGE_FLAG"),
        ("probe_flag", "PROBE_FLAG"),
        ("start_receipt_flag", "START_RECEIPT_FLAG"),
        ("allocator_restart_flag", "ALLOCATOR_RESTART_FLAG"),
        ("image_marker", "IMAGE_MARKER"),
    ):
        assert f'pub(crate) const {constant}: &str = "{declared[key]}";' in rust
    # Only the macOS allocator restart consumes this internal runner flag.
    # The declaration remains shared; other runner protocol flags stay portable.
    assert (
        '#[cfg(target_os = "macos")]\n'
        "pub(crate) const ALLOCATOR_RESTART_FLAG: &str = "
        f'"{declared["allocator_restart_flag"]}";'
    ) in rust
