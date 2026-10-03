# Build Python admission

The source build owns one lazy `BuildPythonAdmission` through preparation,
runtime production, and final consumer admission. Installed prebuilt selection
does not start its process. Standalone producers own their own scope.

The first capture runs the selected interpreter in the canonical isolated
environment facade and retains its `PythonFileCaptureContext`. Portable runtime
identity material is unchanged. Each later boundary launches that same selected
entrypoint once in `--runtime-selection` mode, which observes actual startup
selection without hashing or inventorying runtime trees. It uses the same
runtime/import selectors and native loader census as full capture: executable,
base/active prefix, runtime library, lexical and resolved import/configuration
paths, and native image paths, aliases, contracts, and Mach-O selections. This
detects script-launcher environment changes and newly preferred native providers
inside unchanged loader search directories. The retained context then verifies
file generations and topology before returning a caller-owned receipt copy.

`CommandExecutor.start_guarded` retains the interpreter launch handle with an
exclusive cancellation path and durable custody record under the canonical
memory-guard state root. Interpreter launchers may delegate on any platform, so
the launch PID is distinct from the actual guard identity. One fresh launch
capability binds an immutable startup report to the terminal report, requiring
the same actual guard PID, exact child identity, and command. The capability and
startup path are stripped from the child environment before launch. A caller requests cancellation; `run_guarded` performs
its existing identity-checked child-tree or Windows Job cleanup. The caller does
not terminate or kill the guard. Terminal admission requires the matching
worker's `descendants_closed` result and child exit status. A stopped guard alone
does not establish closure.

Admission revokes immediately on failure but retains the owner, streams, and
custody evidence until terminal cleanup. It never closes a buffered response
stream while its reader may hold the stream lock. `close()` is explicitly
retriable after a nonterminal failure. Actual primary exceptions retain their
type and traceback; cleanup adds a note and a JSON stderr diagnostic. Consumers
that return failure values call `record_failure()` before returning, so scope
exit preserves the original failure while surfacing secondary cleanup evidence.
Successful operations must close admission before publishing success.


Installed runtime observation belongs to the same build operation. A shipped
member is captured once against its release record. Native archive semantics,
the canonical callable projection, and native link receipt facts are then
immutable values shared by retention, codegen, and actual link flag construction.
Retained members receive independent physical observations, and semantic facts
transfer only after their size and SHA-256 match the shipped observations.
Content reads and copies bind the opened handle to that observation; restored
mtime alone never establishes reuse.

Native custody keeps its distinct content policy: unchanged generations reuse
the canonical tar and file facts, identical-byte replacement requires a fresh
physical hash, changed bytes fail, and every consumer still scans exact
extracted membership. Flag construction carries its custody observation into
the actual native link plan. After the linker exits or the link cache is reused,
that closure is checked again before success can be published.

WASM generations own lazy facts for each member, never binary or section buffers.
Split layout observes only the shared member. Relocatable layout does not parse
exports, globals or types, and linking admission retains only requested names
matched against their required symbol kinds. Code and debug section bodies are
not copied into semantic facts. One typed admission report drives both acceptance
and rejection diagnostics for installed, built and cached pairs. Successful
structural validation is reused only behind live member fences; required exports
remain request policy.

Binding captures a separate immutable codegen receipt after checking its expected
digest, without parsing it again. Both installed and source-checkout flows bind
the captured generation that passed admission; final admission cannot reread a
mutable pointer and substitute another pair. Hydration consumes its already
observed source; staged copying and destination/winner content admission remain
independent boundaries. Mutable selection receipts may change concurrently:
publication validates its transaction-local payload, while codegen reuse fences
its pinned receipt and both members. Signed installed metadata is checked by
size and digest before JSON decoding.

Measure these observation components against frozen real artifact bytes before
retaining a performance change. Component timing does not establish installed
release qualification or whole-build improvement.
