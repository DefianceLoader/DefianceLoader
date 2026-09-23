# Crash reports

The loader starts a hidden `bin/defiance-crash-helper.exe` during initialization.
It waits quietly until a crash, then writes a dump from outside the failing game
process. It exits when the game closes. Nothing is uploaded automatically.

Reports default to `DefianceLoader/logs/crashes`, following the configured loader
root. Each run has a unique `defiance-PID-TIMESTAMP` prefix:

| File | Contents |
| --- | --- |
| `.session.txt` | Effective settings (declared sensitive values redacted), plugin activation/failure, exact DLL SHA-256 hashes, patch/relay ownership, Core payload ranges and entry addresses. Independent of the ordinary log level. |
| `.crash.txt` | Exception code, instruction address, thread ID, access-violation operation/address, AMD64 general registers and EFLAGS, helper completion/timeout. |
| `.dmp` | Windows minidump with exception context, thread stacks, modules, thread information and indirectly referenced memory. |
| `.details.txt` | Module addresses, fault module+offset, matching registered patch/payload ranges, and a small raw stack-word snapshot. |
| `.status.txt` | Whether dump writing completed or failed. A timeout in the text report means the dump may be incomplete. |

Send the matching report files and `defiance-loader.log`, along with what you
were doing, the game edition/version, and steps to reproduce. Dumps contain
process memory and may contain personal data; share them privately with someone
you trust. Session files may contain paths and settings from third-party plugins.

Reports explicitly labeled **first-chance** record an exception in registered
plugin DLLs, assembly payloads, or hook/relay ranges. A later game handler may
recover; such a report alone does not prove the game terminated. This observer
continues exception dispatch unchanged and helps when another component replaces
the final unhandled-exception filter. Removed hook ranges are no longer observed.
Only the first captured event in a process is retained. If the game recovers,
restart it before collecting another reproduction.

Empty `.crash.txt` files are reserved at startup; their existence is not evidence
of a crash. Normal runs leave session information but produce no dump. Files are
not automatically deleted: old sessions/dumps can be removed when no longer
needed, after the game exits. Keep a problematic session before testing again.

## Reading a report

For an access violation, `access=0`, `1`, or `8` means read, write, or execute.
`instruction`/`RIP` is the failing instruction; `address` is the memory it tried
to access. They are often different. Module-relative offsets remain useful when
Windows loads DLLs at different addresses.

The helper maps the instruction to a DLL or a registered allocation. Patch and
trampoline mappings name their installing plugin; Core payload mappings identify
the shared block. `payload-entry` rows in the session provide named logic entry
addresses or game entry feature IDs for disassembly. A shared block address does
not by itself identify the responsible feature. Fault location is not proof of
who originally corrupted an object.

Open the `.dmp` in WinDbg or Visual Studio, load matching symbols, and inspect the
exception context and stack. In WinDbg, `.ecxr`, `r`, and `k` select the exception
context, show registers, and attempt a stack trace. The text stack-word listing
is **not** a call stack: it includes arbitrary data and is explicitly labeled.
Custom assembly/relay code without unwind metadata may stop or confuse unwinding.

Rust panics include the panic message and source location. Their synthetic
exception code is `0xe042444c`; registers describe the reporting callback, not
the original panic instruction. The originating runtime then follows its panic
policy (shipped plugins abort). This does not recover from the failure.

## Release symbols and building

`mise run loader` builds the helper, loader, plugins and regroup with release
debug symbols. `mise run loader-package` produces the player ZIP and a separate
`out/defiance-loader-symbols.zip`. Keep both archives together for each published
release, using versioned filenames before building another release. The symbol
archive's `builds.json` records binary and PDB hashes. PDBs stay out of the game
installation; optimized builds still omit some locals or inline frames.

## Plugin authors

Rust plugins using the feature SDK can export
`defiance_feature_sdk::crash_handshake!();` once per DLL. The loader discovers
`defiance_plugin_crash_v1` after identity/ABI validation and before initialization,
and supplies a process-lifetime callback with signature
`unsafe extern "C" fn(message: *const u8, length: usize)`. The handshake returns
void and does not change ABI 5. The bytes are borrowed for the synchronous call;
the callback reads at most 4096 bytes. The SDK formats at most 2048 bytes without
allocating in the panic hook. Install this only once; it replaces that DLL's
default panic hook. Old loaders ignore the optional export. Third-party plugins
can still be diagnosed from native exceptions without opting into Rust reporting.

## Reliability limits and tests

Capture begins after the loader resolves its configuration paths. The unhandled
exception filter preserves the previous filter and does not swallow exceptions.
Handled exceptions outside registered mod ranges are not captured. Inside those
ranges, selected serious exceptions are recorded with the first-chance label.
Another component can replace the final filter later; debugger interception,
forced termination, fail-fast/OOM,
stack exhaustion or severe corruption can prevent capture. There is no guarantee
that an in-process handler gets to run. Missing helper/event support leaves text
capture working; an unwritable report directory disables capture and logs a warning.

The exception path uses preopened handles, fixed stack buffers, direct Win32
writes, and a single-capture guard. It does not call the ordinary logger, allocate
Rust heap storage, take its mutex, or perform stack symbolization. The waiting
thread gives the helper at most ten seconds to finish, then normal exception
handling continues. No gameplay hook runs diagnostic code on every update.

`mise run crash-test` deliberately crashes disposable subprocesses. It verifies
native registers against the minidump exception stream, plugin DLL and loader
panics, module/ownership mapping and removal, previous-filter chaining,
missing-helper fallback, replaced final filters, later recovery from owned-code
faults, and no reports for unrelated handled exceptions or normal exits.
It does not start or modify the game. The fixture binaries are never packaged.
