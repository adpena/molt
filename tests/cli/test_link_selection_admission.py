from __future__ import annotations

import hashlib
from pathlib import Path

import pytest
from tools.command_execution import CommandExecutor

from molt.cli.link_selection_admission import LinkSelectionAdmission
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkProvider,
    SourceExtensionLinkProviderKind,
    SourceExtensionLinkCyclicGroup,
    SourceExtensionLinkRequirements,
    source_extension_link_file,
)


_COMMANDS = CommandExecutor.for_file(__file__)


@pytest.mark.parametrize(
    "kind",
    [
        SourceExtensionLinkProviderKind.LIBRARY,
        SourceExtensionLinkProviderKind.ARCHIVE,
        SourceExtensionLinkProviderKind.FRAMEWORK,
    ],
)
def test_unbound_providers_cannot_bypass_selected_input_admission(kind):
    target = "x86_64-apple-darwin"
    name = (
        "libpython.a" if kind is SourceExtensionLinkProviderKind.ARCHIVE else "Python"
    )
    requirements = SourceExtensionLinkRequirements(
        target, (SourceExtensionLinkProvider(kind, name),)
    )
    with pytest.raises(ValueError, match="checksummed static inputs"):
        LinkSelectionAdmission.capture(requirements)


def test_grouped_provider_cannot_hide_from_admission():
    requirements = SourceExtensionLinkRequirements(
        "x86_64-unknown-linux-gnu",
        (
            SourceExtensionLinkCyclicGroup(
                (
                    SourceExtensionLinkProvider(
                        SourceExtensionLinkProviderKind.LIBRARY, "python3.12"
                    ),
                )
            ),
        ),
    )
    with pytest.raises(ValueError, match="python3.12"):
        LinkSelectionAdmission.capture(requirements)


def test_admission_policy_snapshot_and_subprocess_role_binding(tmp_path, monkeypatch):
    from molt.cli import link_selection_admission as admission
    from molt.cli.extension_scan_surface import _ExtensionScanSurface

    surface = _ExtensionScanSurface(
        frozenset({"PyLong_FromLong"}), frozenset(), frozenset(), tmp_path / "Python.h"
    )
    captured = admission.LinkSelectionAdmission.capture(
        SourceExtensionLinkRequirements("wasm32-wasip1"),
        surface=surface,
    )
    monkeypatch.setattr(
        admission,
        "_support_surface",
        lambda: pytest.fail("support must be captured once"),
    )
    proof = captured.admit(dialect="wasm", stdout="", stderr="")
    path = tmp_path / "selection.json"
    admission.write_link_selection(path, {"linked": proof, "app": proof})
    policy = admission.link_selection_policy(surface)
    admission.validate_link_selection_policy(path, policy, roles={"linked", "app"})
    with pytest.raises(ValueError, match="policy or role"):
        admission.validate_link_selection_policy(path, policy, roles={"linked"})
    with pytest.raises(ValueError, match="policy or role"):
        admission.validate_link_selection_policy(
            path, {"selection_policy": "changed"}, roles={"linked", "app"}
        )


@pytest.mark.slow
@pytest.mark.parametrize(
    ("target", "dialect", "role"),
    [
        ("x86_64-unknown-linux-gnu", "elf-gnu", "ld.lld"),
        ("x86_64-pc-windows-msvc", "coff-msvc", "lld-link"),
        ("x86_64-apple-darwin", "macho", "ld64.lld"),
        ("wasm32-wasip1", "wasm", "wasm-ld"),
    ],
)
def test_real_selection_dormant_api_is_ignored_but_selected_api_is_rejected(
    tmp_path,
    target,
    dialect,
    role,
):
    """A successful link is not a C-API conformance oracle.

    The deliberately unsupported function is supplied by an independent runtime
    object, so both links succeed. Only the second extracts the offending member.
    """
    from molt.cli.llvm_wasi_tools import llvm_tool_candidates, llvm_linker_candidates

    cc_candidates = llvm_tool_candidates("cc")
    ar_candidates = llvm_tool_candidates("ar")
    linkers = llvm_linker_candidates(role)
    if not cc_candidates or not ar_candidates or not linkers:
        pytest.skip("required canonical LLVM target tools are unavailable")
    cc, ar, linker = map(str, (cc_candidates[0], ar_candidates[0], linkers[0]))
    if dialect == "wasm":
        from molt.llvm_toolchain import verify_wasm_llvm_nm
        from molt.source_root import compiler_source_root

        # Use the already-provisioned, verified SDK family, never install in tests.
        sdk = verify_wasm_llvm_nm(compiler_source_root()).path.parent
        suffix = Path(cc).suffix
        cc, ar, linker = (
            str(sdk / (name + suffix)) for name in ("clang", "llvm-ar", "wasm-ld")
        )

    def run(command):
        result = _COMMANDS.run(command, capture_output=True, text=True, timeout=30)
        assert result.returncode == 0, result.stderr
        return result

    def compile(name, source):
        path = tmp_path / (name + ".c")
        path.write_text(source, encoding="utf-8")
        obj = path.with_suffix(".o")
        run(
            [
                cc,
                f"--target={target}",
                "-ffreestanding",
                "-fno-stack-protector",
                "-c",
                str(path),
                "-o",
                str(obj),
            ]
        )
        return obj

    needed = compile(
        "needed",
        "int PyInit_probe(void) { return 42; } int needed(void) { return PyInit_probe(); }",
    )
    dormant = compile(
        "dormant",
        "extern int PyMoltUnsupportedProbe(void); int dormant(void) { return PyMoltUnsupportedProbe(); }",
    )
    runtime = compile("runtime", "int PyMoltUnsupportedProbe(void) { return 0; }")
    override = compile(
        "override",
        "__attribute__((weak)) int PyErr_Occurred(void) { return 0; } "
        "int override(void) { return PyErr_Occurred(); }",
    )
    dynamic = compile(
        "dynamic",
        "void *__imp_PyErr_Occurred; int dynamic(void) { return __imp_PyErr_Occurred != 0; }",
    )
    archive = tmp_path / "dependency (space).a"
    run(
        [
            ar,
            "rcsD",
            str(archive),
            str(needed),
            str(dormant),
            str(override),
            str(dynamic),
        ]
    )
    requirements = SourceExtensionLinkRequirements(
        target, (source_extension_link_file(archive),)
    )
    admission = LinkSelectionAdmission.capture(requirements)
    direct = LinkSelectionAdmission.capture(
        SourceExtensionLinkRequirements(target, (source_extension_link_file(needed),))
    )
    assert not direct.lazy_archives
    assert (
        direct.admit(dialect=dialect, stdout="", stderr="")["inputs"][0]["members"]
        is None
    )
    for reference in ("needed", "dormant", "override", "dynamic"):
        entry = compile(
            "entry",
            f"extern int {reference}(void); int entry(void) {{ return {reference}(); }}",
        )
        output = tmp_path / (reference + ".out")
        why = tmp_path / (reference + ".why")
        if dialect == "coff-msvc":
            args = [
                "/entry:entry",
                "/subsystem:console",
                "/nodefaultlib",
                "/verbose",
                f"/out:{output}",
            ]
        elif dialect == "macho":
            args = [
                "-arch",
                "x86_64",
                "-platform_version",
                "macos",
                "11.0",
                "11.0",
                "-e",
                "_entry",
                "-t",
                "-o",
                str(output),
            ]
        else:
            args = [
                "--entry=entry",
                "--trace",
                f"--why-extract={why}",
                "-o",
                str(output),
            ]
            if dialect == "wasm":
                args += ["--export=entry"]
        result = run([linker, *args, str(entry), str(archive), str(runtime)])
        kwargs = dict(
            dialect=dialect,
            stdout=result.stdout,
            stderr=result.stderr,
            why_extract=why.read_text(encoding="utf-8") if why.exists() else None,
        )
        if reference != "needed":
            rejected = (
                "PyMoltUnsupportedProbe" if reference == "dormant" else "PyErr_Occurred"
            )
            with pytest.raises(ValueError, match=rejected):
                admission.admit(**kwargs)
        else:
            receipt = admission.admit(**kwargs)
            assert receipt["inputs"] == [
                {
                    "input_index": 0,
                    "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                    "members": [
                        {
                            "ordinal": 0,
                            "name": needed.name,
                            "sha256": hashlib.sha256(needed.read_bytes()).hexdigest(),
                        }
                    ],
                }
            ]

    # Eager archives use every member, including runtime impersonators; their
    # ownership cannot disappear through definition/requirement subtraction.
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkLoadingPolicy,
    )

    eager = LinkSelectionAdmission.capture(
        SourceExtensionLinkRequirements(
            target,
            (
                source_extension_link_file(
                    archive, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                ),
            ),
        )
    )
    with pytest.raises(ValueError, match="canonical runtime"):
        eager.admit(dialect=dialect, stdout="", stderr="")

    # Replacing the file after capture must not lend earlier selection to new bytes.
    archive.write_bytes(archive.read_bytes() + b"changed")
    with pytest.raises((OSError, ValueError), match="changed"):
        admission.admit(**kwargs)


@pytest.mark.parametrize("archive", [False, True])
def test_native_selection_captures_member_facts_under_owned_admission(
    tmp_path, monkeypatch, archive
):
    from contextlib import contextmanager
    from collections import OrderedDict
    import subprocess
    import sys

    from molt.cli import native_symbol_inspection as symbols
    from molt.cli.extension_scan_surface import _ExtensionScanSurface
    from tests.cli.native_link_test_support import static_archive_bytes
    from tests.native_artifact_fixtures import native_relocatable_object

    target = "x86_64-unknown-linux-gnu"
    payload = native_relocatable_object(target_triple=target, symbols=("selected",))
    artifact = tmp_path / ("dependency.a" if archive else "dependency.o")
    artifact.write_bytes(static_archive_bytes(payload) if archive else payload)
    surface = _ExtensionScanSurface(
        frozenset(), frozenset(), frozenset(), tmp_path / "Python.h"
    )
    requirements = SourceExtensionLinkRequirements(
        target, (source_extension_link_file(artifact),)
    )
    original_open = symbols.open_stable_regular_file
    handles = []

    @contextmanager
    def own(path, **kwargs):
        with original_open(path, **kwargs) as opened:
            handles.append(opened)
            yield opened

    def inspect(argv, **kwargs):
        assert len(handles) == 1 and not handles[0].stream.closed
        return subprocess.CompletedProcess(
            argv, 0, ("object.o:\n" if archive else "") + "0000 T selected\n", ""
        )

    monkeypatch.setattr(symbols, "open_stable_regular_file", own)
    monkeypatch.setattr(symbols, "_nm_candidate_binaries", lambda: [sys.executable])
    monkeypatch.setattr(symbols, "_run_completed_command", inspect)
    monkeypatch.setattr(symbols, "_default_molt_cache", lambda: tmp_path / "cache")
    monkeypatch.setattr(symbols, "_NATIVE_OBJECT_SYMBOL_SETS_CACHE", OrderedDict())
    captured = LinkSelectionAdmission.capture(requirements, surface=surface)
    facts = captured.facts[artifact]
    assert facts.defined_functions == {"selected"}
    assert facts.artifact_digest == hashlib.sha256(artifact.read_bytes()).hexdigest()
    assert len(handles) == 1 and handles[0].stream.closed
    if archive:
        assert facts.members is not None and len(facts.members) == 1
        assert facts.members[0].identity.sha256 == hashlib.sha256(payload).hexdigest()
        assert set(captured.lazy_archives) == {artifact}
    else:
        assert facts.members is None
        assert not captured.lazy_archives
