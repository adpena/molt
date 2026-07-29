'use strict';

/** Queue-owned transport worker for the Node custody preload. */

const net = require('net');
const { parentPort, workerData } = require('worker_threads');

let ready = false;
let closed = false;
let input = '';
const pending = [];

function signal(headerBuffer, bodyBuffer, status, payload) {
  const header = new Int32Array(headerBuffer);
  const body = new Uint8Array(bodyBuffer);
  let encoded = Buffer.from(
    typeof payload === 'string' ? payload : JSON.stringify(payload),
    'utf8',
  );
  if (encoded.length > body.length) {
    status = -1;
    encoded = Buffer.from(
      'proof child custody broker response exceeded transport capacity',
      'utf8',
    );
  }
  body.fill(0);
  body.set(encoded);
  Atomics.store(header, 1, encoded.length);
  Atomics.store(header, 0, status);
  Atomics.notify(header, 0, 1);
}

function fail(error) {
  const message = error instanceof Error ? error.message : String(error);
  if (!ready) {
    signal(workerData.readyHeader, workerData.readyBody, -1, message);
  }
  while (pending.length) {
    const request = pending.shift();
    signal(request.header, request.body, -1, message);
  }
}

const channel = net.createConnection({
  host: workerData.host,
  port: workerData.port,
});
channel.setNoDelay(true);
channel.on('connect', () => {
  channel.write(JSON.stringify({
    event: 'hook-start',
    runtime: 'node',
    pid: workerData.pid,
    token: workerData.token,
    admitted: true,
  }) + '\n');
});
channel.on('data', (chunk) => {
  input += chunk.toString('utf8');
  for (;;) {
    const newline = input.indexOf('\n');
    if (newline < 0) break;
    const line = input.slice(0, newline);
    input = input.slice(newline + 1);
    let payload;
    try {
      payload = JSON.parse(line);
    } catch (error) {
      fail(new Error('proof child custody broker returned malformed JSON'));
      channel.destroy();
      return;
    }
    if (!ready) {
      if (payload?.event !== 'hook-ready' || payload.runtime !== 'node') {
        fail(new Error('proof child custody broker handshake mismatch'));
        channel.destroy();
        return;
      }
      ready = true;
      signal(workerData.readyHeader, workerData.readyBody, 1, payload);
      continue;
    }
    const request = pending.shift();
    if (!request) {
      fail(new Error('proof child custody broker sent an unsolicited decision'));
      channel.destroy();
      return;
    }
    if (payload?.event !== 'spawn-decision' || payload.sequence !== request.sequence) {
      signal(
        request.header,
        request.body,
        -1,
        'proof child custody broker decision mismatch',
      );
      channel.destroy();
      return;
    }
    signal(request.header, request.body, 1, payload);
  }
});
channel.on('error', fail);
channel.on('end', () => {
  if (!closed) fail(new Error('proof child custody broker closed unexpectedly'));
});
channel.on('close', () => {
  if (!closed) {
    fail(new Error('proof child custody broker transport closed unexpectedly'));
  }
});

parentPort.on('message', (message) => {
  if (message.operation === 'request') {
    if (!ready || closed) {
      signal(
        message.header,
        message.body,
        -1,
        'proof child custody broker is not ready',
      );
      return;
    }
    pending.push(message);
    channel.write(JSON.stringify(message.intent) + '\n', (error) => {
      if (error) fail(error);
    });
    return;
  }
  if (message.operation === 'violation') {
    if (!ready || closed) {
      signal(
        message.header,
        message.body,
        -1,
        'proof child custody broker is not ready',
      );
      return;
    }
    channel.write(JSON.stringify(message.event) + '\n', (error) => {
      if (error) signal(message.header, message.body, -1, error.message);
      else {
        signal(
          message.header,
          message.body,
          1,
          { event: 'violation-recorded' },
        );
      }
    });
    return;
  }
  if (message.operation === 'close') {
    if (closed) {
      signal(message.header, message.body, 1, { event: 'hook-closed' });
      return;
    }
    closed = true;
    channel.end(JSON.stringify({
      event: 'hook-end',
      runtime: 'node',
      pid: workerData.pid,
      admitted: true,
    }) + '\n', () => {
      signal(message.header, message.body, 1, { event: 'hook-closed' });
    });
  }
});
