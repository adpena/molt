"""Every JavaScript and TypeScript source under ``deploy/`` parses.

The deploy sources are ES modules: the Cloudflare Worker, the browser engines
and the enjoice integration files. Node parses each one in a single process
with ``vm.SourceTextModule``, which parses without linking or running; it
strips TypeScript types first with Node's own ``stripTypeScriptTypes``. Brace
and regex counts cannot tell valid code from broken code; a parser can.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
SUFFIXES = {".js", ".mjs", ".ts"}

_PARSER = """
import vm from 'node:vm';
import { stripTypeScriptTypes } from 'node:module';
import { readFileSync } from 'node:fs';
const sources = JSON.parse(process.argv[1]);
const failures = {};
for (const [name, text] of Object.entries(sources)) {
  const source = text ?? readFileSync(name, 'utf8');
  try {
    const js = name.endsWith('.ts') ? stripTypeScriptTypes(source) : source;
    new vm.SourceTextModule(js, { identifier: name });
  } catch (err) {
    failures[name] = `${err.name}: ${String(err.message).split('\\n')[0]}`;
  }
}
process.stdout.write(JSON.stringify(failures) + '\\n');
"""


def _deploy_sources() -> list[str]:
    return sorted(
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "deploy").rglob("*")
        if path.suffix in SUFFIXES
        and path.is_file()
        and "node_modules" not in path.parts
    )


def _parse(sources: dict[str, str | None]) -> dict[str, str]:
    if shutil.which("node") is None:
        pytest.skip("node is required to parse the deploy sources")
    run = run_guarded_test_process(
        [
            "node",
            "--experimental-vm-modules",
            "--no-warnings",
            "--input-type=module",
            "-e",
            _PARSER,
            json.dumps(sources),
        ],
        prefix="MOLT_PYTEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout.strip().splitlines()[-1])


def test_every_deploy_source_parses_as_an_es_module() -> None:
    sources = _deploy_sources()
    assert {Path(name).parts[1] for name in sources} >= {
        "browser",
        "cloudflare",
        "enjoice",
    }
    assert _parse(dict.fromkeys(sources)) == {}


def test_the_parser_rejects_broken_javascript_and_typescript() -> None:
    failures = _parse(
        {
            "broken.js": "export const value = (;",
            "broken.ts": "export interface Config { size: number ",
            "typed.ts": "export const size: number = 1;",
        }
    )
    assert set(failures) == {"broken.js", "broken.ts"}
    assert all(reason.startswith("SyntaxError") for reason in failures.values())
