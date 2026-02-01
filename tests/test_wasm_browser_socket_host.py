import shutil
import subprocess
from pathlib import Path

import pytest


def test_wasm_browser_socket_host(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser socket host test")

    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "browser_socket_host.mjs"
    script.write_text(
        f"""
import {{ createBrowserSocketHost }} from '{browser_host_uri}';

const AF_INET = 2;
const SOCK_STREAM = 1;
const IO_EVENT_READ = 1;
const IO_EVENT_WRITE = 2;
const EINPROGRESS = 115;

class FakeWebSocket {{
  constructor(url) {{
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    this.listeners = new Map();
  }}
  addEventListener(type, fn) {{
    const list = this.listeners.get(type) || [];
    list.push(fn);
    this.listeners.set(type, list);
  }}
  _emit(type, event) {{
    const list = this.listeners.get(type) || [];
    for (const fn of list) {{
      fn(event);
    }}
  }}
  send(data) {{
    this.sent.push(new Uint8Array(data));
  }}
  open() {{
    this.readyState = 1;
    this._emit('open', {{ }});
  }}
  message(data) {{
    this._emit('message', {{ data }});
  }}
  error() {{
    this.readyState = 3;
    this._emit('error', {{ }});
  }}
  close() {{
    this.readyState = 3;
    this._emit('close', {{ }});
  }}
}}

let lastSocket = null;
const socketFactory = (url) => {{
  lastSocket = new FakeWebSocket(url);
  return lastSocket;
}};

const memory = new WebAssembly.Memory({{ initial: 1 }});
const state = {{ memory, runtimeInstance: null }};
const host = createBrowserSocketHost(state, {{
  socketFactory,
  socketScheme: 'ws',
}});

const encoder = new TextEncoder();
const view = new DataView(memory.buffer);

const writeBytes = (ptr, bytes) => {{
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
}};

const writeString = (ptr, text) => {{
  const bytes = encoder.encode(text);
  writeBytes(ptr, bytes);
  return bytes.length;
}};

const writeSockaddrV4 = (ptr, host, port) => {{
  const parts = host.split('.').map((val) => Number.parseInt(val, 10));
  const buf = new Uint8Array(8);
  const dv = new DataView(buf.buffer);
  dv.setUint16(0, AF_INET, true);
  dv.setUint16(2, port, true);
  for (let i = 0; i < 4; i += 1) {{
    buf[4 + i] = parts[i] & 0xff;
  }}
  writeBytes(ptr, buf);
  return buf.length;
}};

const decodeSockaddrV4 = (buf) => {{
  const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const port = dv.getUint16(2, true);
  const host = `${{buf[4]}}.${{buf[5]}}.${{buf[6]}}.${{buf[7]}}`;
  return {{ host, port }};
}};

const handle = Number(host.socketHostNew(AF_INET, SOCK_STREAM, 0, -1));
if (!Number.isFinite(handle) || handle <= 0) {{
  throw new Error('socketHostNew failed');
}}

const addrPtr = 32;
const addrLen = writeSockaddrV4(addrPtr, '127.0.0.1', 8080);
const rc = host.socketHostConnect(handle, addrPtr, addrLen);
if (rc !== -EINPROGRESS) {{
  throw new Error(`expected EINPROGRESS, got ${{rc}}`);
}}
if (!lastSocket || lastSocket.url !== 'ws://127.0.0.1:8080') {{
  throw new Error(`unexpected socket url ${{lastSocket ? lastSocket.url : 'none'}}`);
}}

lastSocket.open();
const rc2 = host.socketHostConnectEx(handle);
if (rc2 !== 0) {{
  throw new Error(`expected connect_ex 0, got ${{rc2}}`);
}}

const sendPtr = 128;
const payload = new Uint8Array([1, 2, 3]);
writeBytes(sendPtr, payload);
const sent = host.socketHostSend(handle, sendPtr, payload.length, 0);
if (sent !== payload.length) {{
  throw new Error(`send returned ${{sent}}`);
}}
if (lastSocket.sent.length !== 1 || lastSocket.sent[0].length !== payload.length) {{
  throw new Error('send did not reach fake socket');
}}

lastSocket.message(new Uint8Array([9, 8, 7, 6]));
const poll = host.socketHostPoll(handle, IO_EVENT_READ | IO_EVENT_WRITE);
if ((poll & IO_EVENT_READ) === 0) {{
  throw new Error('expected read-ready after message');
}}

const recvPtr = 256;
const recv = host.socketHostRecv(handle, recvPtr, 4, 0);
if (recv !== 4) {{
  throw new Error(`recv returned ${{recv}}`);
}}
const recvBytes = new Uint8Array(memory.buffer, recvPtr, 4);
if (recvBytes[0] !== 9 || recvBytes[3] !== 6) {{
  throw new Error('recv data mismatch');
}}

const hostPtr = 400;
const servicePtr = 460;
const hostLen = writeString(hostPtr, 'example.com');
const serviceLen = writeString(servicePtr, '1234');
const outPtr = 512;
const outCap = 256;
const outLenPtr = 560;
const gaiRc = host.socketHostGetaddrinfo(
  hostPtr,
  hostLen,
  servicePtr,
  serviceLen,
  0,
  SOCK_STREAM,
  0,
  0,
  outPtr,
  outCap,
  outLenPtr,
);
if (gaiRc !== 0) {{
  throw new Error(`getaddrinfo failed: ${{gaiRc}}`);
}}
const outLen = view.getUint32(outLenPtr, true);
const gaiBytes = new Uint8Array(memory.buffer, outPtr, outLen);
const dv = new DataView(gaiBytes.buffer, gaiBytes.byteOffset, gaiBytes.byteLength);
let offset = 0;
const count = dv.getUint32(offset, true);
offset += 4;
if (count !== 1) {{
  throw new Error('expected one addrinfo entry');
}}
offset += 12;
const canonLen = dv.getUint32(offset, true);
offset += 4 + canonLen;
const addrLen2 = dv.getUint32(offset, true);
offset += 4;
const addrBytes = gaiBytes.subarray(offset, offset + addrLen2);
const addr = decodeSockaddrV4(addrBytes);
const handle2 = Number(host.socketHostNew(AF_INET, SOCK_STREAM, 0, -1));
if (!Number.isFinite(handle2) || handle2 <= 0) {{
  throw new Error('socketHostNew failed for handle2');
}}
const addrPtr2 = 768;
writeBytes(addrPtr2, addrBytes);
const rc3 = host.socketHostConnect(handle2, addrPtr2, addrBytes.length);
if (rc3 !== -EINPROGRESS) {{
  throw new Error(`expected EINPROGRESS for synthetic connect, got ${{rc3}}`);
}}
if (!lastSocket || lastSocket.url !== 'ws://example.com:1234') {{
  throw new Error(`unexpected synthetic url ${{lastSocket ? lastSocket.url : 'none'}}`);
}}

console.log('ok');
""".lstrip()
    )

    run = subprocess.run(
        ["node", str(script)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "ok"
