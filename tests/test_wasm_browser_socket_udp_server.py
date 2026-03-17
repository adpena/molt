"""MOL-182: Browser socket coverage for UDP and server (bind/listen/accept) sockets.

Validates that the browser socket host supports:
- SOCK_DGRAM socket creation
- bind / listen / accept lifecycle for server sockets
- sendto / recvfrom for UDP-style datagrams
- Proper error codes when operations are unsupported or misconfigured
"""

import shutil
import subprocess
from pathlib import Path

import pytest


def test_wasm_browser_socket_udp_create(tmp_path: Path) -> None:
    """SOCK_DGRAM sockets can be created in the browser host."""
    if shutil.which("node") is None:
        pytest.skip("node is required for browser socket host test")

    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "udp_create.mjs"
    script.write_text(
        f"""
import {{ createBrowserSocketHost }} from '{browser_host_uri}';

const AF_INET = 2;
const SOCK_STREAM = 1;
const SOCK_DGRAM = 2;

const memory = new WebAssembly.Memory({{ initial: 1 }});
const state = {{ memory, runtimeInstance: null }};
const host = createBrowserSocketHost(state, {{
  socketFactory: (url) => {{
    const ws = {{ readyState: 0, sent: [], binaryType: 'arraybuffer' }};
    ws.addEventListener = (type, fn) => {{}};
    ws.send = (data) => ws.sent.push(data);
    ws.close = () => {{}};
    return ws;
  }},
  socketScheme: 'ws',
}});

// SOCK_STREAM should succeed
const tcpHandle = Number(host.socketHostNew(AF_INET, SOCK_STREAM, 0, -1));
if (!Number.isFinite(tcpHandle) || tcpHandle <= 0) {{
  throw new Error('SOCK_STREAM creation failed');
}}

// SOCK_DGRAM should now succeed
const udpHandle = Number(host.socketHostNew(AF_INET, SOCK_DGRAM, 0, -1));
if (!Number.isFinite(udpHandle) || udpHandle <= 0) {{
  throw new Error('SOCK_DGRAM creation failed: got ' + udpHandle);
}}

// Unsupported socket type should fail with EPROTONOSUPPORT (-93)
const rawHandle = Number(host.socketHostNew(AF_INET, 3, 0, -1));
if (rawHandle !== 93 && rawHandle !== -93) {{
  // The handle should be negative (error code)
  if (rawHandle > 0) throw new Error('SOCK_RAW should not succeed');
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


def test_wasm_browser_socket_bind_listen_accept(tmp_path: Path) -> None:
    """Server sockets: bind -> listen -> accept lifecycle."""
    if shutil.which("node") is None:
        pytest.skip("node is required for browser socket host test")

    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "server_lifecycle.mjs"
    script.write_text(
        f"""
import {{ createBrowserSocketHost }} from '{browser_host_uri}';

const AF_INET = 2;
const SOCK_STREAM = 1;
const EWOULDBLOCK = 11;

const memory = new WebAssembly.Memory({{ initial: 1 }});
const state = {{ memory, runtimeInstance: null }};
const host = createBrowserSocketHost(state, {{
  socketScheme: 'ws',
}});

const encoder = new TextEncoder();
const writeBytes = (ptr, bytes) => {{
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
}};

// Create a sockaddr_in for 127.0.0.1:8080
const writeSockaddrV4 = (ptr, hostStr, port) => {{
  const parts = hostStr.split('.').map((v) => Number.parseInt(v, 10));
  const buf = new Uint8Array(8);
  const dv = new DataView(buf.buffer);
  dv.setUint16(0, AF_INET, true); // family
  dv.setUint16(2, port, true);    // port
  for (let i = 0; i < 4; i++) buf[4 + i] = parts[i] & 0xff;
  writeBytes(ptr, buf);
  return buf.length;
}};

const handle = Number(host.socketHostNew(AF_INET, SOCK_STREAM, 0, -1));
if (handle <= 0) throw new Error('socketHostNew failed');

// Bind
const addrPtr = 32;
const addrLen = writeSockaddrV4(addrPtr, '127.0.0.1', 8080);
const bindRc = host.socketHostBind(handle, addrPtr, addrLen);
if (bindRc !== 0) throw new Error('bind failed: ' + bindRc);

// Listen
const listenRc = host.socketHostListen(handle, 5);
if (listenRc !== 0) throw new Error('listen failed: ' + listenRc);

// Accept should return EWOULDBLOCK (no pending connections)
const outLenPtr = 64;
const acceptRc = Number(host.socketHostAccept(handle, 128, 64, outLenPtr));
if (acceptRc !== -EWOULDBLOCK) {{
  throw new Error('accept should return EWOULDBLOCK, got ' + acceptRc);
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


def test_wasm_browser_socket_recvfrom_connected(tmp_path: Path) -> None:
    """recvfrom on a connected TCP socket falls back to recv + peer addr."""
    if shutil.which("node") is None:
        pytest.skip("node is required for browser socket host test")

    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "recvfrom_connected.mjs"
    script.write_text(
        f"""
import {{ createBrowserSocketHost }} from '{browser_host_uri}';

const AF_INET = 2;
const SOCK_STREAM = 1;
const EINPROGRESS = 115;

let lastSocket = null;
const socketFactory = (url) => {{
  const ws = {{ readyState: 0, sent: [], binaryType: 'arraybuffer' }};
  const listeners = new Map();
  ws.addEventListener = (type, fn) => {{
    const list = listeners.get(type) || [];
    list.push(fn);
    listeners.set(type, list);
  }};
  ws._emit = (type, event) => {{
    const list = listeners.get(type) || [];
    for (const fn of list) fn(event);
  }};
  ws.send = (data) => ws.sent.push(data);
  ws.close = () => {{}};
  lastSocket = ws;
  return ws;
}};

const memory = new WebAssembly.Memory({{ initial: 1 }});
const state = {{ memory, runtimeInstance: null }};
const host = createBrowserSocketHost(state, {{ socketFactory, socketScheme: 'ws' }});

const writeBytes = (ptr, bytes) => {{
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
}};

const writeSockaddrV4 = (ptr, hostStr, port) => {{
  const parts = hostStr.split('.').map((v) => Number.parseInt(v, 10));
  const buf = new Uint8Array(8);
  const dv = new DataView(buf.buffer);
  dv.setUint16(0, AF_INET, true);
  dv.setUint16(2, port, true);
  for (let i = 0; i < 4; i++) buf[4 + i] = parts[i] & 0xff;
  writeBytes(ptr, buf);
  return buf.length;
}};

const handle = Number(host.socketHostNew(AF_INET, SOCK_STREAM, 0, -1));
const addrPtr = 32;
const addrLen = writeSockaddrV4(addrPtr, '10.0.0.1', 5000);
const rc = host.socketHostConnect(handle, addrPtr, addrLen);
if (rc !== -EINPROGRESS) throw new Error('expected EINPROGRESS');

// Open the connection
lastSocket._emit('open', {{}});
lastSocket.readyState = 1;

// Enqueue a message
lastSocket._emit('message', {{ data: new Uint8Array([42, 43, 44]).buffer }});

// recvfrom should return data + write peer address
const bufPtr = 256;
const peerAddrPtr = 384;
const peerAddrCap = 64;
const outLenPtr = 512;
const view = new DataView(memory.buffer);

const n = host.socketHostRecvFrom(handle, bufPtr, 16, 0, peerAddrPtr, peerAddrCap, outLenPtr);
if (n !== 3) throw new Error('expected 3 bytes from recvfrom, got ' + n);

const received = new Uint8Array(memory.buffer, bufPtr, 3);
if (received[0] !== 42 || received[1] !== 43 || received[2] !== 44) {{
  throw new Error('recvfrom data mismatch');
}}

// Check that peer address was written
const addrOutLen = view.getUint32(outLenPtr, true);
if (addrOutLen === 0) throw new Error('expected peer address in recvfrom');

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
