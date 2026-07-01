from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def _source(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def _stripped_source(path: str) -> str:
    return "\n".join(line.strip() for line in _source(path).splitlines())


def test_browser_embed_prefers_manifest_table_ref_signatures() -> None:
    source = _stripped_source("wasm/browser_embed.js")

    assert (
        "const directSignature =\n"
        "appTableRefSignatures[directName] || runtimeTableRefSignatures[directName] || null;"
    ) in source
    assert (
        "if (typeof tableFn === 'function' && directSignature) {\n"
        "try {\n"
        "return callWithSignature(tableFn, directSignature, args);"
    ) in source
    assert (
        "const runtimeDirectSignature = runtimeTableRefSignatures[directName] || null;\n"
        "if (typeof rtDirectFn === 'function' && runtimeDirectSignature) {\n"
        "try {\n"
        "return callWithSignature(rtDirectFn, runtimeDirectSignature, args);"
    ) in source
    assert "return callWithSignature(tableFn, callIndirectObjectSignature(name), args);" in source
    assert "callIndirectObjectSignature(name) ||\nappTableRefSignatures[directName]" not in source
    assert (
        "appTableRefSignatures[directName] ||\n"
        "runtimeTableRefSignatures[directName] ||\n"
        "callIndirectObjectSignature(name)"
    ) not in source


def test_node_runner_prefers_export_signatures_for_call_indirect() -> None:
    source = _stripped_source("wasm/run_wasm.js")

    app_indirect_index = source.index("if (typeof appIndirectFn === 'function')")
    assert source.index("if (typeof appDirectFn === 'function')") < app_indirect_index
    assert source.index("const runtimeDirectFn =") < app_indirect_index

    assert (
        "outputExportSignatures[`molt_call_indirect${arity}`] ||\n"
        "callIndirectObjectSignature(name, { includeIndex: true })"
    ) in source
    assert (
        "const appDirectSignature = directName && outputExportSignatures[directName];\n"
        "try {\n"
        "return callWithWasmSignature(\n"
        "appDirectFn,\n"
        "appDirectSignature || callIndirectObjectSignature(name),"
    ) in source
    assert (
        "(directName && outputExportSignatures[directName]) ||\n"
        "(directName && runtimeExportSignatures[directName]) ||\n"
        "null;"
    ) in source
    assert (
        "if (typeof fn === 'function' && directSignature) {\n"
        "return callWithWasmSignature(fn, directSignature, args.slice(1));\n"
        "}\n"
        "if (typeof fn === 'function') {\n"
        "return callWithWasmSignature(fn, callIndirectObjectSignature(name), args.slice(1));"
    ) in source
    assert (
        "const runtimeDirectSignature = directName && runtimeExportSignatures[directName];\n"
        "try {\n"
        "return callWithWasmSignature(\n"
        "runtimeDirectFn,\n"
        "runtimeDirectSignature || callIndirectObjectSignature(name),"
    ) in source

    assert (
        "callIndirectObjectSignature(name) || outputExportSignatures[directName]"
    ) not in source
    assert (
        "callIndirectObjectSignature(name) || runtimeExportSignatures[directName]"
    ) not in source
    assert (
        "(directName && outputExportSignatures[directName]) ||\n"
        "(directName && runtimeExportSignatures[directName]) ||\n"
        "callIndirectObjectSignature(name)"
    ) not in source
