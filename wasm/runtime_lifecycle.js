(function installRuntimeLifecycle(root, factory) {
  const api = factory();
  if (typeof module === 'object' && module && module.exports) module.exports = api;
  root.MoltRuntimeLifecycle = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, () => {
  // Keep the first failure as the cause; teardown must never erase it.
  const combinedError = (errors) => {
    if (errors.length === 1) return errors[0];
    return new AggregateError(errors, errors.map(error => String(error)).join('; '),
      { cause: errors[0] });
  };

  // A reusable browser owner finalizes its runtime before closing any backing
  // services. Keep this state machine shared by the full and minimal embeds.
  const createRuntimeDisposer = (getLifetime, cleanupActions) => {
    let state = 'live';
    let failed = false;
    let failure;
    return () => {
      if (state !== 'live') {
        if (failed) throw failure;
        return;
      }
      state = 'disposing';
      const errors = [];
      try { getLifetime()?.dispose(); } catch (error) {
        if (error?.code === 'MOLT_RUNTIME_ACTIVE') {
          state = 'live';
          throw error;
        }
        errors.push(error);
      }
      for (const cleanup of cleanupActions) {
        try { cleanup(); } catch (error) { errors.push(error); }
      }
      state = 'disposed';
      if (errors.length) {
        failed = true;
        failure = combinedError(errors);
        throw failure;
      }
    };
  };

  const createRuntimeLifetime = (instance, exportNames, checkPending) => {
    let state = 'live';
    let active = 0;
    let executionEntered = false;
    let disposalError;
    let disposalFailed = false;
    let bindings;
    let admissionError;
    let admissionFailed = false;
    const assertLive = () => {
      if (state !== 'live') throw new Error('Molt runtime has been disposed');
    };
    const resolve = () => {
      if (bindings) return bindings;
      if (admissionFailed) throw admissionError;
      const result = {};
      for (const key of ['runtime_execution_enter', 'runtime_execution_leave', 'runtime_shutdown']) {
        const name = exportNames?.[key];
        if (typeof name !== 'string' || typeof instance?.exports?.[name] !== 'function') {
          admissionFailed = true;
          admissionError = new Error(`runtime missing canonical lifetime export ${key}`);
          throw admissionError;
        }
        result[key] = instance.exports[name];
      }
      bindings = result;
      return bindings;
    };
    const execute = (operation) => {
      assertLive();
      const exports = resolve();
      const token = exports.runtime_execution_enter();
      if (typeof token !== 'bigint' || token === 0n) {
        throw new Error('runtime returned an invalid execution-boundary token');
      }
      executionEntered = true;
      active++;
      const errors = [];
      let value;
      try {
        checkPending();
        value = operation();
        if (value && typeof value.then === 'function') {
          throw new Error('guest execution boundary must be synchronous');
        }
        checkPending();
      } catch (error) {
        errors.push(error);
      } finally {
        try { exports.runtime_execution_leave(token); } catch (error) { errors.push(error); }
        active--;
      }
      // A successful leave may itself publish a deferred runtime failure.
      if (!errors.length) {
        try { checkPending(); } catch (error) { errors.push(error); }
      }
      if (errors.length) throw combinedError(errors);
      return value;
    };
    const initialize = (operation) => {
      assertLive();
      resolve();
      if (active) throw new Error('cannot initialize Molt runtime during active execution');
      // WASI reactor initialization binds libc/host memory and runs constructors
      // that runtime_execution_enter itself depends on. It is lifetime-owned
      // bootstrap, not an application execution boundary.
      active++;
      const errors = [];
      let value;
      try {
        value = operation();
        if (value && typeof value.then === 'function') {
          throw new Error('runtime initialization must be synchronous');
        }
        checkPending();
      } catch (error) {
        errors.push(error);
      } finally {
        active--;
      }
      if (errors.length) throw combinedError(errors);
      return value;
    };
    const dispose = () => {
      if (state !== 'live') {
        if (disposalFailed) throw disposalError;
        return;
      }
      if (active) {
        const error = new Error('cannot dispose Molt runtime during active execution');
        error.code = 'MOLT_RUNTIME_ACTIVE';
        throw error;
      }
      state = 'disposing';
      if (admissionFailed) { state = 'disposed'; return; }
      const errors = [];
      let exports = null;
      try { exports = resolve(); } catch (error) { errors.push(error); }
      // The canonical pending-status query is pure and non-initializing. Every
      // admitted lifetime snapshots it while the runtime objects still exist,
      // including setup and execution-entry failure paths.
      if (exports) {
        try { checkPending(); } catch (error) { errors.push(error); }
        try {
          const status = exports.runtime_shutdown();
          // WASI/libc bootstrap can run before Molt itself leaves Uninitialized.
          // Only a successful execution admission makes shutdown status 0 a
          // refused teardown rather than the canonical unused-runtime result.
          if (status !== 1n && !(status === 0n && !executionEntered)) {
            throw new Error(`runtime shutdown did not complete: status=${String(status)}`);
          }
        } catch (error) { errors.push(error); }
      }
      state = 'disposed';
      if (errors.length) {
        disposalFailed = true;
        disposalError = combinedError(errors);
        throw disposalError;
      }
    };
    return { execute, initialize, dispose, assertLive, admit: resolve };
  };
  return { createRuntimeLifetime, createRuntimeDisposer, combinedError };
});
