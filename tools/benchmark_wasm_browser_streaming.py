from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
PW = ["npx.cmd", "--yes", "--package", "@playwright/cli", "playwright-cli"]
HTML = r"""<!doctype html><script type="module">
import {parseMoltWasmImports as parse} from '/embed.js';
const mode=new URL(location).searchParams.get('mode'), urls=['/app.wasm','/runtime.wasm'];
async function load(url){
 const response=await fetch(url,{cache:'no-store'}); if(!response.ok) throw Error(response.status);
 if(mode==='serial'){const bytes=await response.arrayBuffer(),imports=parse(bytes),module=await WebAssembly.compile(bytes);return{bytes,imports,module};}
 const copy=response.clone(), [bytes,module]=await Promise.all([response.arrayBuffer(),WebAssembly.compileStreaming(copy)]);
 return{bytes,imports:parse(bytes),module};
}
const start=performance.now(), loaded=await Promise.all(urls.map(load));
window.result={mode,elapsed_ms:performance.now()-start,app_bytes:loaded[0].bytes.byteLength,runtime_bytes:loaded[1].bytes.byteLength,app_function_imports:loaded[0].imports.funcImports.length,runtime_function_imports:loaded[1].imports.funcImports.length};
</script>"""


class Handler(BaseHTTPRequestHandler):
    files: dict[str, tuple[Path, str]]

    def do_GET(self) -> None:
        path = urlparse(self.path).path
        if path == "/probe.html":
            payload, kind = HTML.encode(), "text/html"
        elif path in self.files:
            source, kind = self.files[path]
            payload = source.read_bytes()
        elif (
            path.endswith(".js") and (ROOT / "wasm" / path.removeprefix("/")).is_file()
        ):
            payload, kind = (
                (ROOT / "wasm" / path.removeprefix("/")).read_bytes(),
                "text/javascript",
            )
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, format: str, *args: object) -> None:
        return


def sample(base: str, mode: str, index: int) -> dict[str, object]:
    session = f"optmatrix-browser-{mode}-{index}-{time.time_ns()}"
    command = [*PW, f"-s={session}"]
    subprocess.run(
        [*command, "open", f"{base}/probe.html?mode={mode}", "--browser", "msedge"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    try:
        done = subprocess.run(
            [
                *command,
                "--raw",
                "eval",
                "async()=>{const end=Date.now()+120000;while(!window.result&&Date.now()<end)await new Promise(r=>setTimeout(r,10));if(!window.result)throw Error('probe timeout');return window.result}",
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=150,
        )
        return json.loads(done.stdout)
    finally:
        subprocess.run(
            [*command, "close"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("A12 requires at least 3 samples")
    Handler.files = {
        "/embed.js": (ROOT / "wasm/browser_embed.js", "text/javascript"),
        "/app.wasm": (args.app.resolve(), "application/wasm"),
        "/runtime.wasm": (args.runtime.resolve(), "application/wasm"),
    }
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{server.server_port}"
    results = {"serial": [], "streaming": []}
    try:
        for index in range(args.samples):
            order = (
                ("serial", "streaming") if index % 2 == 0 else ("streaming", "serial")
            )
            for mode in order:
                results[mode].append(sample(base, mode, index))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    serial = [float(x["elapsed_ms"]) for x in results["serial"]]
    streaming = [float(x["elapsed_ms"]) for x in results["streaming"]]
    sm = statistics.median(serial)
    tm = statistics.median(streaming)
    payload = {
        "schema_version": 1,
        "claim": "OPT-MATRIX-R8",
        "target": "wasm-browser split",
        "axis": "startup",
        "profile": "release final-form split artifacts in fresh system Edge processes",
        "samples_per_variant": args.samples,
        "serial": {"samples_ms": serial, "median_ms": sm},
        "streaming": {"samples_ms": streaming, "median_ms": tm},
        "streaming_speedup": sm / tm,
        "metadata_parity": {
            "serial": results["serial"][0],
            "streaming": results["streaming"][0],
        },
        "artifacts": {
            "app": str(args.app.resolve()),
            "app_bytes": args.app.stat().st_size,
            "runtime": str(args.runtime.resolve()),
            "runtime_bytes": args.runtime.stat().st_size,
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
