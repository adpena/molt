#!/usr/bin/env python3
"""Cross-runtime WASM validation matrix for Molt.

Builds a small WASM smoke corpus by compiling Python source through
``molt build --target wasm`` (linked) and then runs each binary against every
WASM runtime that's available on the host. Aggregates pass/fail per
(test, runtime) and reports any divergence (one runtime accepts a binary while
another rejects it).

Supported runtimes
==================

* ``node``             - Node.js >= 18 with ``wasm/run_wasm.js`` (canonical)
* ``molt-wasm-host``   - Rust wasmtime embedder bundled in this repo
                         (``runtime/molt-wasm-host``). This is the
                         "wasmtime" lane. Built on demand if missing.
* ``wasmtime``         - stock ``wasmtime`` CLI on PATH. Probed only.
                         Molt WASM imports a host shim under ``env.molt_*_host``
                         that the stock CLI cannot satisfy, so this lane is
                         informational and reports incompatible imports as
                         "skipped: incompatible-imports".
* ``wasmer``           - stock ``wasmer`` CLI on PATH. Same caveat.
* ``wasmedge``         - stock ``wasmedge`` CLI on PATH. Same caveat.
* ``browser``          - Headless Chromium via Puppeteer/Playwright (auto
                         detect). Spins up a local HTTP server serving the
                         WASM module + the existing ``wasm/browser_host.html``
                         harness, captures the page console output, and
                         compares against the expected output. Browser lane is
                         skipped automatically when no driver is available.

Smoke corpus
============

Mirrors the native cross-run smoke set: arith, dict, class, try/except,
listcomp, typing, dataclass, t-string. Each entry pairs a Python source with
the expected stdout (``\\n`` separated).

Usage
=====

    # Build the corpus only (no execution)
    python3 tools/wasm_run_matrix.py --build-only

    # Run the matrix across selected runtimes (skip whatever's missing)
    python3 tools/wasm_run_matrix.py --runtime node,molt-wasm-host

    # Browser lane (only if Puppeteer or Playwright is installed)
    python3 tools/wasm_run_matrix.py --runtime browser

    # All runtimes that are present on the host
    python3 tools/wasm_run_matrix.py

Exits non-zero on any pass/fail divergence (one runtime accepts a binary while
another rejects it on the same test). When a runtime is unavailable / not
applicable, that lane is reported as "skipped" but does not cause divergence.
"""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import http.server
import json
import os
import shlex
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
SRC_DIR = REPO_ROOT / "src"
WASM_DIR = REPO_ROOT / "wasm"
RUN_WASM_JS = WASM_DIR / "run_wasm.js"
BROWSER_HOST_HTML = WASM_DIR / "browser_host.html"
RUNTIME_WASM = WASM_DIR / "molt_runtime.wasm"

# ----------------------------------------------------------------------------
# Smoke corpus
# ----------------------------------------------------------------------------


@dataclass(frozen=True)
class SmokeCase:
    """A single Python source -> expected stdout pair."""

    name: str
    source: str
    expected: str


SMOKE_CORPUS: list[SmokeCase] = [
    SmokeCase(
        name="arith",
        source=(
            "x = 7\n"
            "y = 5\n"
            "print(x + y, x - y, x * y, x // y, x % y)\n"
        ),
        expected="12 2 35 1 2",
    ),
    SmokeCase(
        name="dict",
        source=(
            "d = {'a': 1, 'b': 2, 'c': 3}\n"
            "for k in sorted(d):\n"
            "    print(k, d[k])\n"
        ),
        expected="a 1\nb 2\nc 3",
    ),
    SmokeCase(
        name="class",
        source=(
            "class Counter:\n"
            "    def __init__(self):\n"
            "        self.n = 0\n"
            "    def inc(self):\n"
            "        self.n += 1\n"
            "        return self.n\n"
            "c = Counter()\n"
            "for _ in range(3):\n"
            "    c.inc()\n"
            "print(c.n)\n"
        ),
        expected="3",
    ),
    SmokeCase(
        name="try_except",
        source=(
            "def f(x):\n"
            "    try:\n"
            "        return 10 // x\n"
            "    except ZeroDivisionError:\n"
            "        return -1\n"
            "print(f(2), f(0))\n"
        ),
        expected="5 -1",
    ),
    SmokeCase(
        name="listcomp",
        source=(
            "evens = [x for x in range(10) if x % 2 == 0]\n"
            "print(evens)\n"
            "print(sum(evens))\n"
        ),
        expected="[0, 2, 4, 6, 8]\n20",
    ),
    SmokeCase(
        name="typing",
        source=(
            "def add(a: int, b: int) -> int:\n"
            "    return a + b\n"
            "print(add(2, 3))\n"
        ),
        expected="5",
    ),
    SmokeCase(
        name="dataclass",
        source=(
            "from dataclasses import dataclass\n"
            "@dataclass\n"
            "class Point:\n"
            "    x: int\n"
            "    y: int\n"
            "p = Point(3, 4)\n"
            "print(p.x, p.y)\n"
        ),
        expected="3 4",
    ),
    SmokeCase(
        name="tstring",
        source=(
            "name = 'world'\n"
            "n = 3\n"
            "print(f\"hello {name} x{n}\")\n"
        ),
        expected="hello world x3",
    ),
]


SMOKE_BY_NAME = {case.name: case for case in SMOKE_CORPUS}


# ----------------------------------------------------------------------------
# Build pipeline (delegates to the canonical wasm linked-runner helpers)
# ----------------------------------------------------------------------------


def _ensure_pythonpath() -> None:
    src = str(SRC_DIR)
    current = os.environ.get("PYTHONPATH", "")
    parts = current.split(os.pathsep) if current else []
    if src not in parts:
        os.environ["PYTHONPATH"] = (
            src + os.pathsep + current if current else src
        )


def _build_one(case: SmokeCase, *, out_dir: Path, rebuild: bool) -> Path:
    """Compile one smoke case to a linked WASM artifact.

    Returns the path to the linked output. Raises RuntimeError on failure.
    """
    _ensure_pythonpath()

    # Reuse the canonical helper used by the wasm parity tests so we don't
    # diverge from the in-repo build pipeline.
    sys.path.insert(0, str(REPO_ROOT))
    try:
        from tests.wasm_linked_runner import build_wasm_linked  # type: ignore
    finally:
        sys.path.pop(0)

    case_dir = out_dir / case.name
    case_dir.mkdir(parents=True, exist_ok=True)
    src = case_dir / f"{case.name}.py"
    src.write_text(case.source, encoding="utf-8")
    if rebuild:
        for stale in case_dir.glob("output*.wasm"):
            stale.unlink()
    linked = build_wasm_linked(REPO_ROOT, src, case_dir)
    return linked


def build_corpus(
    *, out_dir: Path, rebuild: bool, names: list[str] | None
) -> dict[str, Path]:
    """Build (or rebuild) every selected smoke case. Returns name -> wasm path."""
    selected = [
        case for case in SMOKE_CORPUS if names is None or case.name in names
    ]
    if not selected:
        raise RuntimeError("No smoke cases selected to build")
    artifacts: dict[str, Path] = {}
    for case in selected:
        print(f"[build] {case.name}", flush=True)
        artifacts[case.name] = _build_one(
            case, out_dir=out_dir, rebuild=rebuild
        )
    return artifacts


# ----------------------------------------------------------------------------
# Runtime drivers
# ----------------------------------------------------------------------------


@dataclass
class RunResult:
    """Outcome of running a single (case, runtime)."""

    runtime: str
    case: str
    status: str  # "pass" | "fail" | "skipped"
    detail: str = ""
    stdout: str = ""
    stderr: str = ""
    elapsed_s: float | None = None


def _node_bin() -> str | None:
    requested = os.environ.get("MOLT_NODE_BIN", "").strip()
    if requested:
        if shutil.which(requested) or Path(requested).exists():
            return requested
    return shutil.which("node")


def _normalise(out: str) -> str:
    return out.replace("\r\n", "\n").rstrip("\n")


def _run_node(case: SmokeCase, wasm: Path) -> RunResult:
    node = _node_bin()
    if node is None:
        return RunResult(
            "node", case.name, "skipped", detail="node binary not found"
        )
    if not RUN_WASM_JS.exists():
        return RunResult(
            "node",
            case.name,
            "skipped",
            detail=f"missing {RUN_WASM_JS}",
        )
    cmd = [
        node,
        "--no-warnings",
        "--no-wasm-tier-up",
        "--no-wasm-dynamic-tiering",
        "--wasm-num-compilation-tasks=1",
        str(RUN_WASM_JS),
        str(wasm),
    ]
    env = os.environ.copy()
    env.setdefault("NODE_NO_WARNINGS", "1")
    env["MOLT_WASM_PATH"] = str(wasm)
    env["MOLT_WASM_LINKED"] = "1"
    env["MOLT_WASM_LINKED_PATH"] = str(wasm)
    start = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd, env=env, capture_output=True, text=True, timeout=120
        )
    except subprocess.TimeoutExpired as exc:
        return RunResult(
            "node",
            case.name,
            "fail",
            detail="timeout",
            stdout=exc.stdout or "",
            stderr=exc.stderr or "",
        )
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        return RunResult(
            "node",
            case.name,
            "fail",
            detail=f"exit={proc.returncode}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    actual = _normalise(proc.stdout)
    if actual != _normalise(case.expected):
        return RunResult(
            "node",
            case.name,
            "fail",
            detail=f"stdout-mismatch: got={actual!r} want={case.expected!r}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    return RunResult(
        "node",
        case.name,
        "pass",
        stdout=proc.stdout,
        stderr=proc.stderr,
        elapsed_s=elapsed,
    )


def _resolve_molt_wasm_host() -> Path | None:
    """Resolve the in-repo wasmtime embedder.

    Search order:
      1. ``MOLT_WASM_HOST_PATH`` env var
      2. ``target/release-fast/molt-wasm-host`` (project's standard fast build)
      3. ``target/release/molt-wasm-host``
      4. ``target/debug/molt-wasm-host``
      5. ``shutil.which("molt-wasm-host")``
    """
    override = os.environ.get("MOLT_WASM_HOST_PATH", "").strip()
    if override:
        path = Path(override).expanduser()
        if path.exists():
            return path
    for profile in ("release-fast", "release", "debug"):
        path = REPO_ROOT / "target" / profile / "molt-wasm-host"
        if path.exists():
            return path
    found = shutil.which("molt-wasm-host")
    if found:
        return Path(found)
    return None


def _run_molt_wasm_host(case: SmokeCase, wasm: Path) -> RunResult:
    host = _resolve_molt_wasm_host()
    if host is None:
        return RunResult(
            "molt-wasm-host",
            case.name,
            "skipped",
            detail=(
                "molt-wasm-host binary not found; build with "
                "`cargo build --release -p molt-wasm-host`"
            ),
        )
    cmd = [str(host), str(wasm)]
    env = os.environ.copy()
    env["MOLT_WASM_PATH"] = str(wasm)
    env["MOLT_WASM_LINKED"] = "1"
    env["MOLT_WASM_LINKED_PATH"] = str(wasm)
    start = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd, env=env, capture_output=True, text=True, timeout=120
        )
    except subprocess.TimeoutExpired as exc:
        return RunResult(
            "molt-wasm-host",
            case.name,
            "fail",
            detail="timeout",
            stdout=exc.stdout or "",
            stderr=exc.stderr or "",
        )
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        return RunResult(
            "molt-wasm-host",
            case.name,
            "fail",
            detail=f"exit={proc.returncode}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    actual = _normalise(proc.stdout)
    if actual != _normalise(case.expected):
        return RunResult(
            "molt-wasm-host",
            case.name,
            "fail",
            detail=f"stdout-mismatch: got={actual!r} want={case.expected!r}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    return RunResult(
        "molt-wasm-host",
        case.name,
        "pass",
        stdout=proc.stdout,
        stderr=proc.stderr,
        elapsed_s=elapsed,
    )


# Stock-CLI runtimes (wasmtime / wasmer / wasmedge) require a fully self-
# contained module that imports only ``wasi_snapshot_preview1``. Molt's linked
# WASM additionally imports an ``env`` shim (``molt_*_host`` functions for
# sockets, processes, GPU, DB, time, etc.). The stock CLIs cannot synthesize
# stubs for unknown imports, so attempting to instantiate Molt WASM under them
# will fail at link time. We probe each binary so the matrix output makes the
# constraint explicit instead of silently passing/failing.


def _probe_unsupported_imports(wasm: Path) -> list[str] | None:
    """Return the list of imports that block stock-CLI runtimes (or None on err).

    Molt WASM imports ``wasi_snapshot_preview1`` (fine), ``env.memory`` /
    ``env.__indirect_function_table`` (fine - the runtime supplies these), and
    ``env.molt_*_host`` host shim functions which stock CLIs cannot satisfy.
    Returns the list of incompatible imports for diagnostics.
    """
    try:
        data = wasm.read_bytes()
    except OSError:
        return None
    if data[:4] != b"\x00asm":
        return None

    # Minimal WASM import-section parser. We only care about identifying
    # imports whose (module, name) pair is _not_ wasi_snapshot_preview1 and
    # not env.memory / env.__indirect_function_table.
    pos = 8  # skip magic + version

    def _read_u8(p: int) -> tuple[int, int]:
        return data[p], p + 1

    def _read_uleb(p: int) -> tuple[int, int]:
        result = 0
        shift = 0
        while True:
            b = data[p]
            p += 1
            result |= (b & 0x7F) << shift
            if (b & 0x80) == 0:
                return result, p
            shift += 7
            if shift > 63:
                raise ValueError("uleb overflow")

    def _read_name(p: int) -> tuple[str, int]:
        n, p = _read_uleb(p)
        s = data[p : p + n].decode("utf-8", errors="replace")
        return s, p + n

    incompatible: list[str] = []
    while pos < len(data):
        section_id, pos = _read_u8(pos)
        section_len, pos = _read_uleb(pos)
        section_end = pos + section_len
        if section_id == 2:  # Import section
            count, pos = _read_uleb(pos)
            for _ in range(count):
                module, pos = _read_name(pos)
                name, pos = _read_name(pos)
                kind, pos = _read_u8(pos)
                # Skip the type immediate. 0=func (typeidx u32),
                # 1=table (reftype u8 + limits), 2=memory (limits),
                # 3=global (valtype u8 + mut u8).
                if kind == 0:
                    _, pos = _read_uleb(pos)
                elif kind == 1:
                    pos += 1  # reftype
                    flags, pos = _read_uleb(pos)
                    _, pos = _read_uleb(pos)
                    if flags & 1:
                        _, pos = _read_uleb(pos)
                elif kind == 2:
                    flags, pos = _read_uleb(pos)
                    _, pos = _read_uleb(pos)
                    if flags & 1:
                        _, pos = _read_uleb(pos)
                elif kind == 3:
                    pos += 2
                else:
                    return None
                if module == "wasi_snapshot_preview1":
                    continue
                if module == "env" and name in {
                    "memory",
                    "__indirect_function_table",
                }:
                    continue
                incompatible.append(f"{module}.{name}")
            return incompatible
        pos = section_end
    return incompatible


def _stock_cli_skip_reason(wasm: Path) -> str:
    incompatible = _probe_unsupported_imports(wasm)
    if not incompatible:
        return ""
    sample = ", ".join(incompatible[:3])
    suffix = f" (+{len(incompatible) - 3} more)" if len(incompatible) > 3 else ""
    return f"incompatible-imports: {sample}{suffix}"


def _run_stock_cli(
    runtime: str, exe: str, case: SmokeCase, wasm: Path
) -> RunResult:
    skip = _stock_cli_skip_reason(wasm)
    if skip:
        return RunResult(runtime, case.name, "skipped", detail=skip)
    if runtime == "wasmtime":
        cmd = [exe, "run", "--", str(wasm)]
    elif runtime == "wasmer":
        cmd = [exe, "run", str(wasm)]
    elif runtime == "wasmedge":
        cmd = [exe, str(wasm)]
    else:
        return RunResult(
            runtime, case.name, "skipped", detail="unknown stock runtime"
        )
    start = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=60
        )
    except subprocess.TimeoutExpired as exc:
        return RunResult(
            runtime,
            case.name,
            "fail",
            detail="timeout",
            stdout=exc.stdout or "",
            stderr=exc.stderr or "",
        )
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        return RunResult(
            runtime,
            case.name,
            "fail",
            detail=f"exit={proc.returncode}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    actual = _normalise(proc.stdout)
    if actual != _normalise(case.expected):
        return RunResult(
            runtime,
            case.name,
            "fail",
            detail=f"stdout-mismatch: got={actual!r} want={case.expected!r}",
            stdout=proc.stdout,
            stderr=proc.stderr,
            elapsed_s=elapsed,
        )
    return RunResult(
        runtime,
        case.name,
        "pass",
        stdout=proc.stdout,
        stderr=proc.stderr,
        elapsed_s=elapsed,
    )


# ----------------------------------------------------------------------------
# Browser lane (Puppeteer / Playwright)
# ----------------------------------------------------------------------------


def _detect_browser_driver() -> tuple[str, str] | None:
    """Detect a usable headless browser driver.

    Returns (kind, hint) where kind is one of "puppeteer-node",
    "playwright-node", "playwright-python". Returns None when none are
    available.
    """
    node = _node_bin()
    if node is not None:
        for pkg, kind in (
            ("puppeteer", "puppeteer-node"),
            ("playwright", "playwright-node"),
        ):
            try:
                proc = subprocess.run(
                    [node, "-e", f"require.resolve('{pkg}')"],
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
            except (OSError, subprocess.TimeoutExpired):
                continue
            if proc.returncode == 0:
                return kind, pkg
    # Python Playwright
    try:
        import playwright  # type: ignore  # noqa: F401

        return "playwright-python", "playwright"
    except Exception:
        pass
    return None


@dataclass
class _BrowserServer:
    httpd: socketserver.TCPServer
    port: int
    thread: threading.Thread


def _start_static_server(root: Path) -> _BrowserServer:
    handler_cls = type(
        "MatrixHandler",
        (http.server.SimpleHTTPRequestHandler,),
        {
            "directory": str(root),
            "log_message": lambda self, fmt, *args: None,
        },
    )

    def _handler_factory(*args, **kwargs):
        return handler_cls(*args, directory=str(root), **kwargs)

    httpd = socketserver.TCPServer(("127.0.0.1", 0), _handler_factory)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    return _BrowserServer(httpd=httpd, port=port, thread=thread)


def _stop_static_server(server: _BrowserServer) -> None:
    server.httpd.shutdown()
    server.httpd.server_close()
    server.thread.join(timeout=5)


_BROWSER_DRIVER_JS = r"""
'use strict';

const driver = process.argv[2];
const url = process.argv[3];
const timeoutMs = Number(process.argv[4] || '30000');

async function runPuppeteer() {
  const puppeteer = require('puppeteer');
  const browser = await puppeteer.launch({
    headless: 'new',
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  });
  try {
    const page = await browser.newPage();
    const consoleLines = [];
    page.on('console', (msg) => {
      consoleLines.push(msg.text());
    });
    page.on('pageerror', (err) => {
      consoleLines.push(`[pageerror] ${err.message}`);
    });
    await page.goto(url, { waitUntil: 'load', timeout: timeoutMs });
    // Wait for the molt_main run to flag completion. The harness sets
    // window.__moltMatrixDone = true and exposes window.__moltMatrixOutput.
    await page.waitForFunction(
      'window.__moltMatrixDone === true',
      { timeout: timeoutMs }
    );
    const out = await page.evaluate(() => window.__moltMatrixOutput || '');
    process.stdout.write(out);
  } finally {
    await browser.close();
  }
}

async function runPlaywright() {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext();
    const page = await context.newPage();
    page.on('console', (msg) => {
      // captured via window.__moltMatrixOutput; nothing to do here.
    });
    await page.goto(url, { waitUntil: 'load', timeout: timeoutMs });
    await page.waitForFunction(
      'window.__moltMatrixDone === true',
      { timeout: timeoutMs }
    );
    const out = await page.evaluate(() => window.__moltMatrixOutput || '');
    process.stdout.write(out);
  } finally {
    await browser.close();
  }
}

(async () => {
  try {
    if (driver === 'puppeteer') {
      await runPuppeteer();
    } else if (driver === 'playwright') {
      await runPlaywright();
    } else {
      throw new Error(`unknown driver: ${driver}`);
    }
  } catch (err) {
    process.stderr.write(`${err.stack || err.message || String(err)}\n`);
    process.exit(2);
  }
})();
"""


_BROWSER_HARNESS_HTML = r"""<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>Molt WASM Matrix</title>
  </head>
  <body>
    <pre id="log"></pre>
    <script type="module">
      import { loadMoltWasm } from './browser_host.js';
      const logEl = document.getElementById('log');
      const lines = [];
      const log = (msg) => {
        lines.push(msg);
        logEl.textContent += msg + '\n';
      };
      window.__moltMatrixOutput = '';
      window.__moltMatrixDone = false;
      (async () => {
        try {
          const host = await loadMoltWasm({
            wasmUrl: './CASE.wasm',
            linkedUrl: './CASE.wasm',
            runtimeUrl: './molt_runtime.wasm',
            log: (level, msg) => log(`[${level}] ${msg}`),
            captureStdout: (chunk) => {
              window.__moltMatrixOutput += chunk;
            },
          });
          await host.run();
          window.__moltMatrixDone = true;
        } catch (err) {
          log(`[error] ${err.message}`);
          window.__moltMatrixOutput += `[error] ${err.message}\n`;
          window.__moltMatrixDone = true;
        }
      })();
    </script>
  </body>
</html>
"""


def _run_browser(case: SmokeCase, wasm: Path) -> RunResult:
    driver = _detect_browser_driver()
    if driver is None:
        return RunResult(
            "browser",
            case.name,
            "skipped",
            detail=(
                "no headless browser driver: install puppeteer "
                "(`npm i -g puppeteer`) or playwright "
                "(`npm i -g playwright && npx playwright install chromium`)"
            ),
        )
    if not BROWSER_HOST_HTML.exists():
        return RunResult(
            "browser",
            case.name,
            "skipped",
            detail=f"missing {BROWSER_HOST_HTML}",
        )
    if not RUNTIME_WASM.exists():
        return RunResult(
            "browser",
            case.name,
            "skipped",
            detail=f"missing {RUNTIME_WASM}",
        )

    kind, _pkg = driver
    with tempfile.TemporaryDirectory(prefix="molt-wasm-matrix-") as tmp:
        site = Path(tmp)
        # Required browser harness assets — symlink/copy to keep relative
        # imports intact.
        for src_name in ("browser_host.js", "browser_gpu_worker.js", "molt_vfs_browser.js"):
            src = WASM_DIR / src_name
            if src.exists():
                try:
                    os.symlink(src, site / src_name)
                except OSError:
                    shutil.copyfile(src, site / src_name)
        try:
            os.symlink(RUNTIME_WASM, site / "molt_runtime.wasm")
        except OSError:
            shutil.copyfile(RUNTIME_WASM, site / "molt_runtime.wasm")
        case_wasm_dst = site / f"{case.name}.wasm"
        try:
            os.symlink(wasm, case_wasm_dst)
        except OSError:
            shutil.copyfile(wasm, case_wasm_dst)
        html = _BROWSER_HARNESS_HTML.replace("CASE", case.name)
        index = site / f"{case.name}.html"
        index.write_text(html, encoding="utf-8")
        driver_js = site / "_matrix_driver.js"
        driver_js.write_text(_BROWSER_DRIVER_JS, encoding="utf-8")

        server = _start_static_server(site)
        try:
            url = f"http://127.0.0.1:{server.port}/{case.name}.html"
            node = _node_bin()
            if node is None:
                return RunResult(
                    "browser",
                    case.name,
                    "skipped",
                    detail="node binary not found",
                )
            if kind == "puppeteer-node":
                cmd = [node, str(driver_js), "puppeteer", url, "30000"]
            elif kind == "playwright-node":
                cmd = [node, str(driver_js), "playwright", url, "30000"]
            elif kind == "playwright-python":
                # Python driver path — invoke a small inline script.
                py_script = site / "_matrix_driver.py"
                py_script.write_text(
                    "import sys\n"
                    "from playwright.sync_api import sync_playwright\n"
                    "url = sys.argv[1]\n"
                    "with sync_playwright() as pw:\n"
                    "    browser = pw.chromium.launch(headless=True)\n"
                    "    try:\n"
                    "        page = browser.new_page()\n"
                    "        page.goto(url, wait_until='load', timeout=30000)\n"
                    "        page.wait_for_function('window.__moltMatrixDone === true', timeout=30000)\n"
                    "        sys.stdout.write(page.evaluate('window.__moltMatrixOutput || \"\"'))\n"
                    "    finally:\n"
                    "        browser.close()\n",
                    encoding="utf-8",
                )
                cmd = [sys.executable, str(py_script), url]
            else:
                return RunResult(
                    "browser",
                    case.name,
                    "skipped",
                    detail=f"unsupported driver kind {kind}",
                )

            start = time.perf_counter()
            try:
                proc = subprocess.run(
                    cmd, capture_output=True, text=True, timeout=120
                )
            except subprocess.TimeoutExpired as exc:
                return RunResult(
                    "browser",
                    case.name,
                    "fail",
                    detail="timeout",
                    stdout=exc.stdout or "",
                    stderr=exc.stderr or "",
                )
            elapsed = time.perf_counter() - start
            if proc.returncode != 0:
                return RunResult(
                    "browser",
                    case.name,
                    "fail",
                    detail=f"exit={proc.returncode}",
                    stdout=proc.stdout,
                    stderr=proc.stderr,
                    elapsed_s=elapsed,
                )
            actual = _normalise(proc.stdout)
            if actual != _normalise(case.expected):
                return RunResult(
                    "browser",
                    case.name,
                    "fail",
                    detail=(
                        f"stdout-mismatch: got={actual!r} want={case.expected!r}"
                    ),
                    stdout=proc.stdout,
                    stderr=proc.stderr,
                    elapsed_s=elapsed,
                )
            return RunResult(
                "browser",
                case.name,
                "pass",
                stdout=proc.stdout,
                stderr=proc.stderr,
                elapsed_s=elapsed,
            )
        finally:
            _stop_static_server(server)


# ----------------------------------------------------------------------------
# Matrix orchestration
# ----------------------------------------------------------------------------


ALL_RUNTIMES = ("node", "molt-wasm-host", "wasmtime", "wasmer", "wasmedge", "browser")


def _resolve_runtime_set(spec: str | None) -> list[str]:
    if not spec:
        return list(ALL_RUNTIMES)
    parts = [p.strip() for p in spec.split(",") if p.strip()]
    bad = [p for p in parts if p not in ALL_RUNTIMES]
    if bad:
        raise SystemExit(
            f"Unknown runtime(s): {', '.join(bad)}. "
            f"Supported: {', '.join(ALL_RUNTIMES)}"
        )
    return parts


def _runtime_present(runtime: str) -> tuple[bool, str]:
    if runtime == "node":
        if _node_bin() is None:
            return False, "node binary not on PATH (or missing MOLT_NODE_BIN)"
        return True, ""
    if runtime == "molt-wasm-host":
        if _resolve_molt_wasm_host() is None:
            return False, (
                "molt-wasm-host binary missing; "
                "build with `cargo build --release -p molt-wasm-host`"
            )
        return True, ""
    if runtime == "wasmtime":
        return shutil.which("wasmtime") is not None, "wasmtime CLI not on PATH"
    if runtime == "wasmer":
        return shutil.which("wasmer") is not None, "wasmer CLI not on PATH"
    if runtime == "wasmedge":
        return shutil.which("wasmedge") is not None, "wasmedge CLI not on PATH"
    if runtime == "browser":
        present = _detect_browser_driver() is not None
        return present, (
            "no headless browser driver (puppeteer/playwright)"
            if not present
            else ""
        )
    return False, f"unknown runtime {runtime}"


def _run_one(runtime: str, case: SmokeCase, wasm: Path) -> RunResult:
    if runtime == "node":
        return _run_node(case, wasm)
    if runtime == "molt-wasm-host":
        return _run_molt_wasm_host(case, wasm)
    if runtime == "wasmtime":
        return _run_stock_cli("wasmtime", "wasmtime", case, wasm)
    if runtime == "wasmer":
        return _run_stock_cli("wasmer", "wasmer", case, wasm)
    if runtime == "wasmedge":
        return _run_stock_cli("wasmedge", "wasmedge", case, wasm)
    if runtime == "browser":
        return _run_browser(case, wasm)
    return RunResult(runtime, case.name, "skipped", detail="unknown runtime")


def _print_matrix(results: list[RunResult], runtimes: list[str]) -> None:
    cases = sorted({r.case for r in results})
    by_pair: dict[tuple[str, str], RunResult] = {
        (r.runtime, r.case): r for r in results
    }
    col_widths = {
        rt: max(len(rt), max((len(by_pair[(rt, c)].status) for c in cases if (rt, c) in by_pair), default=4))
        for rt in runtimes
    }
    name_width = max(len("test"), max((len(c) for c in cases), default=4))
    header = " | ".join([f"{'test':<{name_width}}"] + [f"{rt:<{col_widths[rt]}}" for rt in runtimes])
    print(header)
    print("-" * len(header))
    for case in cases:
        row = [f"{case:<{name_width}}"]
        for rt in runtimes:
            res = by_pair.get((rt, case))
            row.append(
                f"{(res.status if res else 'n/a'):<{col_widths[rt]}}"
            )
        print(" | ".join(row))


def _detect_divergences(results: list[RunResult]) -> list[str]:
    """A divergence = at least one runtime passes and at least one fails for the
    same case. Skipped results don't count."""
    divergences: list[str] = []
    by_case: dict[str, list[RunResult]] = {}
    for r in results:
        by_case.setdefault(r.case, []).append(r)
    for case, rs in by_case.items():
        passes = [r.runtime for r in rs if r.status == "pass"]
        fails = [(r.runtime, r.detail) for r in rs if r.status == "fail"]
        if passes and fails:
            divergences.append(
                f"{case}: pass={passes} fail={[f'{rt}({d})' for rt, d in fails]}"
            )
    return divergences


def _summarise(results: list[RunResult]) -> dict[str, dict[str, int]]:
    summary: dict[str, dict[str, int]] = {}
    for r in results:
        s = summary.setdefault(r.runtime, {"pass": 0, "fail": 0, "skipped": 0})
        s[r.status] = s.get(r.status, 0) + 1
    return summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="WASM cross-runtime validation matrix for Molt."
    )
    parser.add_argument(
        "--runtime",
        default=None,
        help=(
            "Comma-separated runtime selector. "
            f"Supported: {','.join(ALL_RUNTIMES)} (default: all available)"
        ),
    )
    parser.add_argument(
        "--case",
        default=None,
        help=(
            "Comma-separated test case selector "
            f"(default: all of {','.join(c.name for c in SMOKE_CORPUS)})"
        ),
    )
    parser.add_argument(
        "--build-only",
        action="store_true",
        help="Compile the smoke corpus only, then exit.",
    )
    parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Force a rebuild of the smoke corpus (clear cached outputs).",
    )
    parser.add_argument(
        "--out-dir",
        default=None,
        help=(
            "Where to put compiled smoke artifacts "
            "(default: build/wasm_matrix/ under repo root)."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON in addition to the human matrix.",
    )
    args = parser.parse_args(argv)

    runtimes = _resolve_runtime_set(args.runtime)
    case_names = (
        [n.strip() for n in args.case.split(",") if n.strip()]
        if args.case
        else None
    )
    if case_names:
        unknown = [n for n in case_names if n not in SMOKE_BY_NAME]
        if unknown:
            raise SystemExit(
                f"Unknown smoke case(s): {', '.join(unknown)}. "
                f"Supported: {', '.join(c.name for c in SMOKE_CORPUS)}"
            )

    out_dir = (
        Path(args.out_dir).expanduser().resolve()
        if args.out_dir
        else REPO_ROOT / "build" / "wasm_matrix"
    )
    out_dir.mkdir(parents=True, exist_ok=True)

    print(f"[matrix] runtimes={runtimes}")
    print(f"[matrix] out_dir={out_dir}")

    # Probe runtime availability up front.
    presence: dict[str, tuple[bool, str]] = {
        rt: _runtime_present(rt) for rt in runtimes
    }
    for rt, (ok, why) in presence.items():
        marker = "ok" if ok else "missing"
        suffix = f" ({why})" if not ok and why else ""
        print(f"[matrix] runtime {rt}: {marker}{suffix}")

    artifacts = build_corpus(
        out_dir=out_dir, rebuild=args.rebuild, names=case_names
    )

    if args.build_only:
        print(f"[matrix] built {len(artifacts)} smoke artifacts; build-only: exiting")
        return 0

    results: list[RunResult] = []
    for rt in runtimes:
        ok, why = presence[rt]
        for case_name, wasm_path in artifacts.items():
            case = SMOKE_BY_NAME[case_name]
            if not ok:
                results.append(
                    RunResult(rt, case.name, "skipped", detail=why)
                )
                continue
            res = _run_one(rt, case, wasm_path)
            print(
                f"[matrix] {rt:<14} {case.name:<10} -> {res.status}"
                + (f" ({res.detail})" if res.detail else "")
            )
            results.append(res)

    print()
    _print_matrix(results, runtimes)
    print()

    summary = _summarise(results)
    for rt in runtimes:
        s = summary.get(rt, {})
        print(
            f"[summary] {rt:<14} pass={s.get('pass', 0)} "
            f"fail={s.get('fail', 0)} skipped={s.get('skipped', 0)}"
        )

    divergences = _detect_divergences(results)
    if divergences:
        print()
        print("[matrix] DIVERGENCES (one runtime accepts, another rejects):")
        for d in divergences:
            print(f"  - {d}")

    if args.json:
        payload = {
            "runtimes": runtimes,
            "presence": {rt: {"available": ok, "reason": why} for rt, (ok, why) in presence.items()},
            "results": [dataclasses.asdict(r) for r in results],
            "summary": summary,
            "divergences": divergences,
        }
        json_path = out_dir / "matrix_results.json"
        json_path.write_text(json.dumps(payload, indent=2) + "\n")
        print(f"[matrix] wrote {json_path}")

    if divergences:
        return 2
    if any(r.status == "fail" for r in results):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
