'use strict';

/**
 * Mandatory Node child-process custody.
 *
 * The preload is deliberately a thin synchronous client.  It never resolves or
 * hashes an executable and it never authors an admission result: the parent
 * broker owns those decisions.  A private Worker keeps the socket event loop
 * live while the proof thread waits through SharedArrayBuffer/Atomics, so both
 * asynchronous and synchronous child_process APIs receive a decision before
 * they can launch.
 */

const childProcess = require('child_process');
const moduleAuthority = require('module');
const path = require('path');
const workerThreads = require('worker_threads');
const OriginalWorker = workerThreads.Worker;

const POLICY_ENV = 'MOLT_PROOF_CHILD_CUSTODY_JSON';
const ENDPOINT_ENV = 'MOLT_PROOF_CHILD_CUSTODY_ENDPOINT';
const TOKEN_ENV = 'MOLT_PROOF_CHILD_CUSTODY_TOKEN';
const RESERVED_ENVIRONMENT = Object.freeze([
  POLICY_ENV,
  ENDPOINT_ENV,
  TOKEN_ENV,
  'NODE_OPTIONS',
]);
const RESPONSE_BYTES = 64 * 1024;
const BROKER_TIMEOUT_MS = 10_000;

const rawPolicy = process.env[POLICY_ENV];
if (rawPolicy) {
    const policy = JSON.parse(rawPolicy);
    if (!policy || policy.schema !== 'molt.proof-child-custody.v1') {
      throw new Error('malformed proof child custody policy');
    }

    const endpoint = process.env[ENDPOINT_ENV] || '';
    const token = process.env[TOKEN_ENV] || '';
    const separator = endpoint.lastIndexOf(':');
    const port = Number(endpoint.slice(separator + 1));
    if (separator <= 0 || !Number.isInteger(port) || port <= 0 || port > 65535 || !token) {
      throw new Error('proof child custody broker authority is unavailable');
    }

    // Capture the queue-owned values before payload code can mutate process.env.
    const custodyEnvironment = Object.freeze(Object.fromEntries(
      RESERVED_ENVIRONMENT.map((name) => {
        const value = process.env[name];
        if (typeof value !== 'string' || !value) {
          throw new Error(`proof child custody environment is missing ${name}`);
        }
        return [name, value];
      }),
    ));

    // Main-thread broker exchanges are synchronous by construction, so one
    // fixed shared frame serves every launch without allocating 64 KiB per
    // child-process decision.
    const exchangeHeaderBuffer = new SharedArrayBuffer(
      Int32Array.BYTES_PER_ELEMENT * 2,
    );
    const exchangeBodyBuffer = new SharedArrayBuffer(RESPONSE_BYTES);
    const exchangeHeader = new Int32Array(exchangeHeaderBuffer);

    function exchange(operation, payload = {}) {
      Atomics.store(exchangeHeader, 0, 0);
      Atomics.store(exchangeHeader, 1, 0);
      brokerWorker.postMessage({
        operation,
        ...payload,
        header: exchangeHeaderBuffer,
        body: exchangeBodyBuffer,
      });
      const wait = Atomics.wait(exchangeHeader, 0, 0, BROKER_TIMEOUT_MS);
      if (wait === 'timed-out') {
        throw new Error(`proof child custody broker ${operation} timed out`);
      }
      const length = Atomics.load(exchangeHeader, 1);
      const text = Buffer.from(
        new Uint8Array(exchangeBodyBuffer, 0, length),
      ).toString('utf8');
      if (Atomics.load(exchangeHeader, 0) !== 1) {
        throw new Error(`proof child custody broker ${operation} failed: ${text}`);
      }
      try {
        return JSON.parse(text);
      } catch (error) {
        throw new Error(`proof child custody broker ${operation} returned invalid JSON`);
      }
    }

    const readyHeader = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT * 2);
    const readyBody = new SharedArrayBuffer(RESPONSE_BYTES);
    const brokerWorker = new OriginalWorker(
      path.join(__dirname, 'node_child_custody_worker.cjs'),
      {
      // The transport is queue code, not proof payload.  Start it without the
      // proof preload or proof environment; endpoint authority is passed only
      // through its private workerData object.
      execArgv: [],
      env: {},
      workerData: {
        host: endpoint.slice(0, separator),
        port,
        token,
        pid: process.pid,
        readyHeader,
        readyBody,
      },
    });
    // Transport lifetime must never keep a proof alive by itself.
    brokerWorker.unref();
    const readyState = new Int32Array(readyHeader);
    const readyWait = Atomics.wait(readyState, 0, 0, BROKER_TIMEOUT_MS);
    const readyLength = Atomics.load(readyState, 1);
    const readyText = Buffer.from(new Uint8Array(readyBody, 0, readyLength)).toString('utf8');
    if (readyWait === 'timed-out') {
      throw new Error('proof child custody broker handshake timed out');
    }
    if (Atomics.load(readyState, 0) !== 1) {
      throw new Error(`proof child custody broker handshake failed: ${readyText}`);
    }
    let readyPayload;
    try {
      readyPayload = JSON.parse(readyText);
    } catch (error) {
      throw new Error('proof child custody broker handshake returned invalid JSON');
    }
    if (readyPayload?.event !== 'hook-ready' || readyPayload.runtime !== 'node') {
      throw new Error('proof child custody broker handshake mismatch');
    }

    let sequence = 0;
    let ending = false;

    function environmentValue(environment, name) {
      if (process.platform !== 'win32') return environment[name];
      const key = Object.keys(environment).find((candidate) => candidate.toUpperCase() === name);
      return key === undefined ? undefined : environment[key];
    }

    function withCustodyEnvironment(options) {
      const source = options?.env === undefined ? process.env : options.env;
      const environment = { ...(source || {}) };
      for (const [name, value] of Object.entries(custodyEnvironment)) {
        if (process.platform === 'win32') {
          for (const candidate of Object.keys(environment)) {
            if (candidate.toUpperCase() === name && candidate !== name) delete environment[candidate];
          }
        }
        environment[name] = value;
      }
      return { ...(options || {}), env: environment };
    }

    function custodyWorker(filename, options = {}) {
      const rewritten = withCustodyEnvironment(options);
      const execArgv = Array.isArray(options.execArgv)
        ? [...options.execArgv]
        : [...process.execArgv];
      const preload = `--require=${__filename}`;
      if (!execArgv.includes(preload)) execArgv.unshift(preload);
      rewritten.execArgv = execArgv;
      return Reflect.construct(OriginalWorker, [filename, rewritten], OriginalWorker);
    }
    custodyWorker.prototype = OriginalWorker.prototype;
    Object.defineProperty(custodyWorker.prototype, 'constructor', {
      configurable: true,
      value: custodyWorker,
      writable: true,
    });
    for (const [name, descriptor] of Object.entries(
      Object.getOwnPropertyDescriptors(OriginalWorker),
    )) {
      if (name === 'length' || name === 'name' || name === 'prototype') continue;
      Object.defineProperty(custodyWorker, name, descriptor);
    }
    workerThreads.Worker = custodyWorker;
    moduleAuthority.syncBuiltinESMExports();

    function optionsIndex(args) {
      return args.findIndex((value) => value !== null && typeof value === 'object' && !Array.isArray(value));
    }

    function rewriteOptions(args) {
      const rewritten = [...args];
      const index = optionsIndex(rewritten);
      if (index >= 0) {
        rewritten[index] = withCustodyEnvironment(rewritten[index]);
      } else if (Array.isArray(rewritten[0])) {
        rewritten.splice(1, 0, withCustodyEnvironment(undefined));
      } else {
        rewritten.unshift(withCustodyEnvironment(undefined));
      }
      return rewritten;
    }

    function effectiveOptions(args) {
      const index = optionsIndex(args);
      return index >= 0 ? args[index] : undefined;
    }

    function decide(requested, options) {
      const environment = options?.env || process.env;
      const cwd = options?.cwd === undefined
        ? process.cwd()
        : (Buffer.isBuffer(options.cwd) ? options.cwd.toString() : String(options.cwd));
      sequence += 1;
      const decision = exchange('request', {
        sequence,
        intent: {
          event: 'spawn-intent',
          sequence,
          requested: String(requested),
          path: environmentValue(environment, 'PATH'),
          path_ext: environmentValue(environment, 'PATHEXT'),
          cwd,
          shell: options?.shell ?? false,
        },
      });
      if (decision.event !== 'spawn-decision' || decision.sequence !== sequence) {
        throw new Error('proof child custody broker returned a mismatched decision');
      }
      if (options?.shell && decision.admitted === true) {
        throw new Error('proof child custody broker admitted an opaque shell launch');
      }
      if (decision.admitted !== true || typeof decision.resolved !== 'string' || !decision.resolved) {
        const reason = typeof decision.reason === 'string' ? decision.reason : 'not-admitted';
        throw new Error(`proof child executable is outside admitted toolchain closure (${reason}): ${requested}`);
      }
      return decision;
    }

    function recordViolation(event, reason) {
      return exchange('violation', {
        event: {
          event: 'policy-violation',
          operation: event,
          reason,
          admitted: false,
        },
      });
    }

    for (const name of ['spawn', 'spawnSync', 'execFile', 'execFileSync']) {
      const original = childProcess[name];
      childProcess[name] = function custodySpawn(requested, ...args) {
        const rewritten = rewriteOptions(args);
        const options = effectiveOptions(rewritten);
        const decision = decide(requested, options);
        return original.call(this, decision.resolved, ...rewritten);
      };
    }

    for (const name of ['exec', 'execSync']) {
      childProcess[name] = function opaqueExec() {
        recordViolation(`child_process.${name}`, 'opaque-shell');
        throw new Error(`opaque child_process.${name} is forbidden in proof custody`);
      };
    }
    childProcess.fork = function opaqueFork() {
      recordViolation('child_process.fork', 'implicit-node-launch');
      throw new Error('child_process.fork is forbidden in proof custody; invoke node explicitly');
    };

    function closeBroker() {
      if (ending) return;
      ending = true;
      exchange('close');
    }
    process.once('beforeExit', closeBroker);
    // beforeExit is skipped by process.exit() and fatal termination.  The exit
    // hook still permits the synchronous SAB exchange and preserves normal
    // process.exit semantics without monkeypatching the public function.
    process.once('exit', closeBroker);
}
