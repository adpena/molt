'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const childProcess = require('child_process');

const rawPolicy = process.env.MOLT_PROOF_CHILD_CUSTODY_JSON;
if (rawPolicy) {
  const policy = JSON.parse(rawPolicy);
  const journal = process.env.MOLT_PROOF_CHILD_CUSTODY_JOURNAL;
  const append = (event) => {
    if (journal) fs.appendFileSync(journal, JSON.stringify(event) + '\n', { mode: 0o600 });
  };
  const resolveExecutable = (token, options) => {
    if (typeof token !== 'string' || !token) return null;
    if (path.isAbsolute(token) || path.dirname(token) !== '.') return path.resolve(token);
    const env = options && options.env ? options.env : process.env;
    const names = process.platform === 'win32'
      ? (env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').map((ext) => token.toLowerCase().endsWith(ext.toLowerCase()) ? token : token + ext)
      : [token];
    for (const directory of (env.PATH || '').split(path.delimiter)) {
      for (const name of names) {
        const candidate = path.resolve(directory, name);
        if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) return candidate;
      }
    }
    return null;
  };
  const admit = (token, options) => {
    const resolved = resolveExecutable(token, options);
    let digest = null;
    if (resolved) digest = crypto.createHash('sha256').update(fs.readFileSync(resolved)).digest('hex');
    const authority = policy.descendants === 'declared-toolchains'
      ? policy.allowed.find((item) => item.path === (process.platform === 'win32' ? resolved?.toLowerCase() : resolved) && item.sha256 === digest)
      : null;
    const event = { event: 'child-process', requested: String(token), resolved, admitted: Boolean(authority) };
    if (authority) event.toolchain = authority.toolchain;
    append(event);
    if (!authority) throw new Error(`proof child executable is outside admitted toolchain closure: ${token}`);
  };
  for (const name of ['spawn', 'spawnSync', 'execFile', 'execFileSync']) {
    const original = childProcess[name];
    childProcess[name] = function custodySpawn(token, ...args) {
      const options = args.find((value) => value && typeof value === 'object' && !Array.isArray(value));
      admit(token, options);
      return original.call(this, token, ...args);
    };
  }
  for (const name of ['exec', 'execSync']) {
    childProcess[name] = function opaqueExec() {
      append({ event: `child_process.${name}`, admitted: false });
      throw new Error(`opaque child_process.${name} is forbidden in proof custody`);
    };
  }
  childProcess.fork = function custodyFork() {
    append({ event: 'child_process.fork', admitted: false });
    throw new Error('child_process.fork is forbidden in proof custody');
  };
}
