// Executable providers for the current generated physical execution ABI.
const assert = require('node:assert/strict');
const abi = require('../wasm/wasm_abi_generated.json');
const {createRuntimeLifetime, runtimeExport} = require('../wasm/runtime_lifecycle.js');
const FIXTURE_CONTEXT = 32768n;
const fixtureCompiledSymbol = logical => logical + abi.execution_context_abi.suffix;
const fixturePhysicalRuntimeSymbol = logical => {
  const spec = abi.runtime_execution_abis[logical];
  assert.ok(spec, `missing generated fixture execution identity ${logical}`);
  return spec.physical_symbol;
};
const fixtureScopeNames = names => ({...names,
  attached_context: fixturePhysicalRuntimeSymbol('molt_attached_context'),
  compiled_execution_enter: fixturePhysicalRuntimeSymbol('molt_compiled_execution_enter'),
  compiled_execution_leave: fixturePhysicalRuntimeSymbol('molt_compiled_execution_leave'),
});
const fixtureScopeExports = providers => {
  const exports = {};
  for (const [logical, fn] of Object.entries(providers)) {
    const spec = abi.runtime_execution_abis[logical];
    if (!spec) {
      assert.ok(!logical.startsWith('molt_'), `unclassified fixture export ${logical}`);
      exports[logical] = fn;
      continue;
    }
    exports[spec.physical_symbol] = spec.context === 'Attached'
      ? (context, ...args) => { assert.equal(context, FIXTURE_CONTEXT); return fn(...args); }
      : fn;
  }
  const scopes = [];
  let nextScope = 0n;
  exports[fixturePhysicalRuntimeSymbol('molt_attached_context')] = () => FIXTURE_CONTEXT;
  exports[fixturePhysicalRuntimeSymbol('molt_compiled_execution_enter')] = context => {
    assert.equal(context, FIXTURE_CONTEXT);
    const scope = ++nextScope; scopes.push(scope); return scope;
  };
  exports[fixturePhysicalRuntimeSymbol('molt_compiled_execution_leave')] = (context, scope) => {
    assert.equal(context, FIXTURE_CONTEXT); assert.equal(scopes.pop(), scope);
  };
  return exports;
};
const createAttachedRuntimeFixture = providers => {
  const guards = [];
  let nextGuard = 0n;
  const instance = {exports: fixtureScopeExports({
    molt_runtime_execution_enter() { const guard = ++nextGuard; guards.push(guard); return guard; },
    molt_runtime_execution_leave(guard) { assert.equal(guards.pop(), guard); },
    molt_runtime_shutdown() { assert.equal(guards.length, 0); return 1n; },
    ...providers,
  })};
  const names = fixtureScopeNames(Object.fromEntries(
    ['runtime_execution_enter', 'runtime_execution_leave', 'runtime_shutdown'].map(
      name => [name, fixturePhysicalRuntimeSymbol(`molt_${name}`)])));
  const lifetime = createRuntimeLifetime(instance, names, () => {});
  lifetime.admit();
  return {instance, lifetime};
};
module.exports = {fixtureAbi: abi, fixtureCompiledSymbol, FIXTURE_CONTEXT, fixturePhysicalRuntimeSymbol, fixtureScopeNames,
  fixtureScopeExports, createAttachedRuntimeFixture, fixtureRuntimeExport: runtimeExport};
