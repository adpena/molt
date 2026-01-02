const fs = require('fs');
const wasmBuffer = fs.readFileSync('output.wasm');

let memory_ptr = 1024; 
let sleep_called = false;

const importObject = {
  molt: {
    print_int: (i) => {
      console.log(i.toString());
    },
    alloc: (size) => {
      const ptr = memory_ptr;
      memory_ptr += Number(size);
      if (importObject.molt.memory) {
          const view = new DataView(importObject.molt.memory.buffer);
          // Initialize state to 0 (at ptr + 12)
          if (ptr + 12 < view.byteLength) {
              view.setBigInt64(ptr + 12, 0n, true); 
          }
      }
      return BigInt(ptr);
    },
    async_sleep: (obj) => {
      if (!sleep_called) {
        sleep_called = true;
        return BigInt("0x7ffc000000000000"); // PENDING
      } else {
        sleep_called = false;
        return 0n;
      }
    },
    block_on: (future_ptr) => {
      const table = importObject.molt.table;
      const memory = importObject.molt.memory;
      const view = new DataView(memory.buffer);
      
      const ptr = Number(future_ptr);
      const poll_fn_idx = view.getUint32(ptr + 8, true); 
      const poll_fn = table.get(poll_fn_idx);
      
      let res;
      do {
          res = poll_fn(future_ptr);
          if (res === BigInt("0x7ffc000000000000")) {
              // Yield? In JS we can just loop or setImmediate
              // But WASM call is synchronous here.
              // For MVP, busy loop.
          } else {
              break;
          }
      } while (true);
      return res;
    }
  }
};

WebAssembly.instantiate(wasmBuffer, importObject).then(wasmModule => {
  const { molt_main, molt_memory, molt_table } = wasmModule.instance.exports;
  if (molt_memory) importObject.molt.memory = molt_memory;
  if (molt_table) importObject.molt.table = molt_table;

  molt_main();
}).catch(e => {
  console.error(e);
  process.exit(1);
});
