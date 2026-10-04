# Public plugin API

This reference describes the implemented surface, not planned game bindings.
Author workflow: [plugin authoring](plugin-authoring.md). Source of truth:
[Rust ABI](../crates/api/src/lib.rs), [C/C++ header](../crates/api/include/defiance.h),
[Rust SDK](../crates/feature-sdk/src/lib.rs).

## Base ABI 5

`defiance_plugin()` returns a permanent `Plugin` containing `abi_version`,
NUL-terminated UTF-8 `name` and `version`, `init(const Api*) -> i32`, and optional
`stop()`. `init` returns 0 when initialization succeeds. The loader may still
call `stop()` after a managed plugin succeeds if its runtime patch plan loses an
overlap or one of its required providers is refused. `Api.abi_version` must equal 5 and
`Api.reserved` must equal 0. Do not append fields to this structure yourself.
All ABI callbacks use C calling conventions; game detours must match the
original game's Microsoft x64 signature, not merely the loader callback type.

Each plugin receives a process-lifetime API table whose logger is bound to its
loader-plan ID. Cached log callbacks keep that identity after a DLL reload;
callbacks run from any thread and messages need no source prefix. The loader
reserves a logger for at most 256 distinct IDs per process; reloading an ID
reuses its reservation.

| Api function | Result and contract |
|---|---|
| `log(level, message)` | Log UTF-8 text, attributed to this API table's plugin ID on any thread. INFO=0, WARN=1, ERROR=2, DEBUG=3 (written only with `[logging] level = debug`; loaders before 0.4.0 write it as info). Choose the level by the [log level policy](development.md#log-levels). The message must live through the call. Output also obeys the [plugin log filters](development.md#filter-plugin-logs). |
| `module_base(name)` | Loaded module base, or null. Does not load modules. |
| `module_size(base)` | Mapped size, or zero. Use a loaded module base. |
| `find_pattern(base, size, pattern)` | Unique match or null; hex bytes with `??` wildcards, matched against the code as it was before any plugin hooked it (the original service). Caller supplies a valid readable range. |
| `find_pattern_at(base, size, pattern, offset)` | Unique match plus an offset inside the matched window; otherwise null. A signature match alone is not a supported-build guarantee. |
| `hook(target, detour, original_out)` | Requests an entry detour, decoding whole instructions. On acceptance, `original_out` gets a callable trampoline. |
| `hook_exact(target, detour, displaced, original_out)` | Same, with an exact span. Rejects incomplete instructions; never expands the requested span. |
| `hook_call(site, detour, original_out)` | Requests redirecting one direct rel32 call; other callers stay unchanged. Returns the original callee through the output. |
| `unhook(target)` | Drops a staged request or restores the published hook/patch at that address. Storage remains retained if restoration fails. |
| `rtti_method(class, method)` | Method pointer or null, using the loader's `defiance-rtti.ini` name-to-slot table. Class names are substring matches; RTTI contains no method names. |
| `vtable_slot(class, slot)` | Zero-based virtual slot pointer or null, using the first matching vtable in logic.dll then game.dll. Validate ambiguous class matches yourself. |
| `config_get(plugin_id, key)` | Canonical validated UTF-8 setting or null. IDs/keys are case-insensitive. The diagnostics `probe_file` setting resolves valid relative filenames against the loader's accepted config directory and returns an absolute path. Loader owns the returned process-lifetime string. |
| `patch_bytes(target, before, after, length)` | Compares expected bytes and requests an owned replacement; mismatch/overlap is refused. Caller supplies valid buffers. |

## Optional expected-write contract v1

This export declares the writes a successful managed `init` is expected to
submit. It is separate from `Plugin` and leaves ABI 5 unchanged:

```c
__declspec(dllexport)
const DefiancePatchContractV1 *defiance_patch_contract_v1(const DefianceApi *api);
```

The loader calls it after successful `init`, before planning or publishing that
plugin's staged requests. Return a process-lifetime contract object and entry
array. Each entry names a loaded module basename, RVA, operation kind, and exact
original bytes; `after` may also specify exact replacement bytes. Leave
`after` null/zero for runtime-linked branches and hooks. An empty entry list
declares that `init` must stage no writes.

The loader requires a one-to-one match: a missing, unexpected, duplicated, or
different-kind request refuses the plugin before commit and blocks its
dependants. `patch_contract: true` in a managed plugin's sidecar manifest makes
the export mandatory; without that flag, exporting the contract still opts the
plugin into validation. Older manifests default to no requirement.

Built-in unit plugins can use
`defiance_feature_sdk::export_unit_patch_contract!(UNITS, native_names, has_call_detour)`.
The SDK derives the list from the descriptors for Core's selected build, with
the same native replacement names and call-detour mode that `init` passes to
`units::install`. A standalone plugin can instead return an explicit entry
list from its export.
The contract should be maintained independently of the calls made by `init`,
so it can expose an accidentally omitted request.

Hook/patch functions return 0 when the request is accepted, nonzero on failure.
Install during init so ownership is attributed correctly. Managed plugins stage
their hooks and byte writes until the next legacy initializer or the end of
startup. The loader checks each staged group for overlaps and changed live
bytes, stops the later owner and its dependants for each conflict, and publishes
the survivors in one transaction. A manifest dependency may place a legacy
initializer before a managed plugin; that later managed group checks against
already published spans. A conflict may call `stop()` after a successful
`init`. `original_out` is ready during init, but the new detour is not live until
the group commits; a call through the trampoline still reaches the original
code. Hot-added and reloaded managed plugins commit after their own init,
checked against already published spans. Legacy plugins keep immediate
ownership checks. Automatic hook chaining is not provided.
Entry trampolines reject displaced instructions requiring relative/RIP-relative
relocation. Distant detours can use owned near relays. Failed initialization
discards its staged requests; an incomplete published rollback stops further
plugin startup. Never free a detour while any game code can still reach it.

## Optional service API v1

The optional plugin export is:

```c
/* Use extern "C" as well when compiling this export as C++. */
__declspec(dllexport)
int32_t defiance_plugin_services(const DefianceServiceApiV1 *services);
```

It runs after manifest/export identity validation, before init. Verify
`version == 1` and `size >= sizeof(DefianceServiceApiV1)`, cache the pointer,
and return 0. A rejected handshake prevents init. A service plugin requires
a manifest. The handshake itself must not register/query; do that during init.
The Rust SDK macro implements this export and validation.

| Function (C header spelling) | Contract |
|---|---|
| `register_service(name, version, table, size)` | Publishes under the current plugin ID after successful init. Table/storage owned by provider, immutable and process-lifetime. Version and size must be nonzero. Returns 0 success, 1 invalid argument, 2 outside init context, 3 duplicate provider/name/version. |
| `query_service(provider, name, version, min_size)` | Returns a table or null. Requires the provider to be an initialized declared dependency (or the loader, `defiance.loader`, which needs no declaration), an exact service version, and at least `min_size` bytes. `min_size` must be nonzero. |

The Rust ABI fields are named `register` and `query`; their layouts/signatures
match the C fields above. Provider IDs and service names are case-insensitive
ASCII letters, digits, `.`, `_`, or `-`, 1–128 bytes. A null query can mean missing
provider/service, wrong version/size, undeclared dependency, invalid name, or a
call outside init. Log the required provider/name/version when refusing init.

Failed init removes registrations without publishing them. Stopping removes
discovery, not cached pointers. Tables are not copied, allocated, or freed by
the registry. Lookup is not a sandbox or proof that a table's callback is safe.

## Rust SDK functions

All pointer-taking functions marked unsafe retain the ABI's lifetime and
threading obligations; wrappers do not validate game objects.

| Function | Purpose |
|---|---|
| `raw(api, plugin_id, key)` | Copy a configuration string into `Option<String>`. |
| `string`, `boolean`, `integer` | Typed configuration lookup returning `Result<_, ConfigError>`. Missing values are errors, not silently chosen defaults. |
| `parse_bool`, `parse_integer` | Parse the corresponding textual forms without a host. |
| `service_handshake!()` | Export the optional handshake once per DLL. |
| `services::accept(ptr)` | Validate/cache the loader extension; normally called only by the macro. |
| `services::available()` | Whether a valid handshake was received; does not mean a provider exists. |
| `services::register<T>(name, version, &'static T)` | Register a permanent table, using `size_of::<T>()`; returns an error code. Requires a C-compatible contract, despite the generic type. |
| `services::query<T>(provider, name, version)` | Resolve a table with matching size/alignment. Caller must choose the correct C-compatible type. |
| `services::crash_ranges()` | Resolve the loader's crash-ranges-v1 table; no dependency needed. |
| `services::trace()` | Resolve the loader's trace-v1 table; no dependency needed. |
| `services::near_memory()` | Resolve the loader's near-memory-v1 table; no dependency needed. |
| `services::multiplayer()` | Resolve the loader's multiplayer-v1 table; no dependency needed. |
| `services::original()` | Resolve the loader's original-v1 table; no dependency needed. |
| `services::selection()` | Resolve the selection-v1 table from `defiance.selection`. |
| `services::game_access()` | Resolve Core's game-access-v1 table; declare a direct `defiance.core` dependency. |
| `services::members(game, entity)` | Copy the roster pointer array using Core's size/capacity protocol. Returns None on unavailability, malformed/changing data, or allocation failure; entities remain borrowed. |

`defiance_api::leak(Plugin)` is a convenience for allocating the permanent
plugin descriptor; call it once from the entry point as the examples do.

## Loader service: crash ranges v1

Provider: `defiance.loader` (`LOADER_PROVIDER`, C `DEFIANCE_LOADER_PROVIDER`).
Name: `crash-ranges`. Exact service version: `1`. Table: Rust `CrashRangesV1`,
C `DefianceCrashRangesV1`. The loader provides it, so any plugin may query it
during init without a manifest dependency; no plugin may register under
`defiance.loader`.

Use it for code the loader cannot see, such as a block a plugin allocates for
assembly: a fault in a mapped range is named `label+offset` in the crash
report's details, and faults there are captured first-chance
([crash-reporting.md](crash-reporting.md)). Hooks and plugin DLLs are mapped
already.

- `map(start, end, label) -> i32` maps `[start, end)`. The label is UTF-8,
  1–128 bytes, with no control characters, and is copied. A range with the same
  start replaces the earlier one. Returns 0, or 1 for an empty range or an
  invalid label.
- `unmap(start) -> i32` stops attributing faults to the range that starts at
  `start`. Returns 0, or 1 for a zero start.

Both may be called from any thread once the table is resolved. A mapping is
attribution, not proof of who caused a fault. Log text never maps a range.

## Loader service: near memory v1

Provider: `defiance.loader`. Name: `near-memory`. Exact service version: `1`.
Table: Rust `NearMemoryV1`, C `DefianceNearMemoryV1`. No manifest dependency
is needed.

Code that a module reaches with a rel32 must sit within 2GB of it. When the
game's DLLs are relocated low, the game's own allocations can fill that range
before a plugin asks. The loader therefore reserves 64K slots near
`logic.dll` and `game.dll` as each one loads. The startup log shows how many
it holds next to each module's base.

- `take(hint, size) -> usize` commits `size` bytes (1–0x10000) of executable,
  writable memory in the held slot nearest `hint` that a rel32 from `hint` can
  reach. It returns the slot's address, or 0 when `size` is out of range or no
  held slot reaches `hint`. On 0, search for free pages as before.
  `VirtualFree(address, 0, MEM_RELEASE)` releases the memory.

Callable from any thread. Each call uses up a whole slot, so allocate one block
and divide it up rather than calling once per small stub.

## Loader service: multiplayer v1

Provider: `defiance.loader`. Name: `multiplayer`. Exact service version: `1`.
Table: Rust `MultiplayerV1`, C `DefianceMultiplayerV1`. No manifest dependency
is needed.

The gameplay plugins change local simulation and send nothing over the network,
so an online game with one active would desync. Every active plugin whose
manifest does not declare `"multiplayer_safe": true` (legacy plugins included)
blocks multiplayer; Core refuses the game's online connection while any does.

The loader fails closed: a plugin that is not multiplayer-safe initializes after
Core, and only once Core has reported the guard installed. Without Core, or on a
build where Core cannot find the game's lobby connection, such plugins are
blocked (`the multiplayer guard is not installed`). The loader's own shipped
gameplay plugins cannot declare themselves safe; a manifest that tries is
rejected.

- `blockers(buffer, capacity) -> usize` copies the blocking plugins' IDs,
  comma-separated and NUL-terminated, when `capacity` exceeds their length, and
  returns that length: 0 when nothing blocks. Before startup has finished it
  reports a placeholder, never 0.
- `guard_installed()` is Core's report that the guard is in place. Other
  plugins must not call it.

This is accident prevention for honest players, not anticheat: everything runs
on the player's machine, and a modified install can remove any check.

Callable from any thread. The startup log states the outcome
(`multiplayer: allowed` or `multiplayer: blocked while active: ...`).

## Loader service: session v1

Provider: `defiance.loader`. Name: `session`. Table: Rust `SessionV1`, C
`DefianceSessionV1`. For Core: `tracking()` once it reports missions,
`mission(delta)` once a mission state's constructor (1) or destructor (-1) has
returned, and `before_mission()` just before one is constructed, when the
loader applies pending hot reloads; from then until `mission(1)` the state
counts as being built, so no reload runs during it. Other plugins have no use
for it.

## Loader service: trace v1

Provider: `defiance.loader`. Name: `trace`. Exact service version: `1`. Table:
Rust `TraceV1`, C `DefianceTraceV1`. No manifest dependency is needed.

A diagnostic for development: it finds who calls an address in the running
game. Each hit at a traced address is logged to `defiance-loader.log` with the
argument registers and the call stack (see "Tracing callers in game" in
[development.md](development.md)), and execution resumes unchanged. It uses
hardware breakpoints, so it changes no code and works on addresses other
patches own. There are four, shared with `[trace] sites`.

- `trace(address, hits, label) -> i32` logs the next `hits` (1–1000) hits at
  `address`, which must be executable code, under `label` (the crash-ranges
  label rules). The site is then released. Returns 0; 1 for an invalid
  argument; 2 when all four sites are in use; 3 when the address is already
  traced; 4 when tracing could not start.
- `stop(address) -> i32` releases the site early. Returns 0, or 1 when the
  address is not traced.

Both may be called from any thread once the table is resolved. Tracing slows
every hit; keep it out of builds meant for play.

## Loader service: trace-capture v1

Provider: `defiance.loader`. Name: `trace-capture`. Exact service version: `1`.
Rust table: `TraceCaptureV1`; C table: `DefianceTraceCaptureV1`. The SDK
resolves it with `services::trace_capture()`.

This service captures register values, typed memory fields, and optional stack
frames at an execution breakpoint. The loader copies the plan and owns the
bounded event queue; the exception handler calls no plugin code. Stack frames
are not unwound: the event starts with the captured instruction pointer, then
the loader scans a bounded 512-byte window from the stack pointer for return
addresses inside loaded modules that follow a call instruction. These candidate
frames are marked in `scanned_frames`; the diagnostics plugin prints them with
a `?` prefix because they may be stale. The frame limit includes the instruction
pointer when present. Capture and queue capacity are bounded, and the four
hardware sites are shared with `trace` v1 and `[trace] sites`. A site keeps its
slot after reaching its hit limit until its handle is stopped or its session is
closed.

Call `open()` during plugin initialization to get a nonzero session capability.
Use `start(session, request, &handle)` on any thread to copy a plan into the
loader and create a generation handle. A request must have the exact current
`TraceRequestV1` size, zero reserved fields, an executable address, 1–1,000,000
hits, `every >= 1`, no more than 16 fields, and no more than 16 stack frames.
Hits count every encounter, including ones excluded by a filter or sampling.
The optional filter compares `(field & filter_mask)` to
`(filter_value & filter_mask)`; the field must be readable. Field types are
`u8`, `u16`, `u32`, `u64`, `f32`, and `f64`; floating-point values are returned
as raw bits. Registers use the order `rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi,
r8..r15, rip`. A field path has at most four signed offsets: intermediate
steps read pointers, and the last step reads the requested type. With no path,
the field contains the low bits selected by its type. Failed field reads clear
that field's bit in `valid_fields`.

`poll(handle, events, capacity, &stats)` copies up to `capacity` queued events
and returns the count. A zero capacity permits a null event pointer; the stats
pointer is optional. Stats report total encountered hits, captured events,
dropped events, and whether the hit limit is still active. Each site's queue
holds 128 events; check `dropped` when interpreting a capture. `stop(handle)`
discards remaining queued events and frees the handle's site. `close(session)`
stops all handles in that session. The loader also closes plugin sessions after
failed init or unload.

`start` returns 0 on success, 1 for an invalid request, 2 when all shared sites
are occupied, 3 for a duplicate address, 4 when tracing is unavailable, and 5
for a closed or unknown session. `poll` returns a negative value for an invalid
or closed handle. `stop` and `close` return 0 on success or 1 for an unknown or
closed handle/session. A completed handle does not identify or stop a later
capture that reuses its address.

## Loader service: original v1

Provider: `defiance.loader`. Name: `original`. Exact service version: `1`.
Table: Rust `OriginalV1`, C `DefianceOriginalV1`. No manifest dependency is
needed.

Memory as it was before any plugin hooked or patched it through the loader.
Check code you only call or read here, so your plugin works whether or not
another plugin starting before it has hooked that code. Check the live bytes
where you write: ownership refuses a write over another plugin's anyway.
`find_pattern` and `find_pattern_at` search the same view.

- `read(address, out, length) -> i32` copies `length` bytes at `address` into
  `out`, with the loader-owned writes they overlap undone. The range must lie
  in one readable region (one section of a module). Returns 0; 1 for a null
  argument or zero length; 2 when the range is not readable.

Callable from any thread. Writes a plugin makes without the loader are not
undone.

## Game service: selection v1

Provider: `defiance.selection`. Name: `selection`. Exact service version: `1`.
Table: Rust `SelectionV1`, C `DefianceSelectionV1`.

`is_selected(selectable) -> u8` returns 0 or 1 using the supported build's
selectable facet virtual getter. Null returns 0. Otherwise the caller must
provide a **live selectable facet**, not an entity or squad pointer. Call only
on the game's owning thread; do not retain facets across destruction or level
transitions. The callback does not retain the pointer or change selection.

Selection registers the service only after its verified patches install.
Regroup is the production consumer: it resolves the table during init and uses
it during game input handling. Older selection DLLs may have the same package
version but no service; regroup explicitly refuses init if the table is absent.
Upgrade loader, selection, and regroup together.

## Game service: Core game access v1

Provider `defiance.core`, name `game-access`, exact version `1`; table
`GameAccessV1` / `DefianceGameAccessV1`. Core publishes it only after supported
build preparation. All calls require live objects on the owning game thread.
Null objects return an empty/error result; this is not arbitrary-pointer validation.

| Callback | Contract |
|---|---|
| `entity_from_facet(facet)` | Follows a facet's entity-holder link, returning the entity or null. Use only a facet type with this documented holder layout. |
| `selectable(entity)` | Returns the entity's selectable facet or null. |
| `copy_members(squad, out, capacity)` | Returns roster count, or `SIZE_MAX` if unavailable/malformed. Capacity zero queries the count. No writes if capacity is too small. A sufficient buffer receives borrowed entity pointers; it must not overlap game storage. Call again and verify the count when using a two-pass allocation. |
| `read_member(entity, expected_squad, out)` | For a soldier with a `SquadUnitSelectableFacet`, copies `MemberStateV1`, including selectable and parent pointers, selected/enabled bytes, and validated firing/posture pins. Returns 0 on success, 1 if unavailable or wrong parent. A null expected parent disables filtering. Disabled members are reported, not silently omitted. Output is cleared on failure when non-null. |
| `set_firing_pin(selectable, value, has_pin)` | Requires a soldier's `SquadUnitSelectableFacet`. With nonzero has_pin, stores value+1 using byte wrapping; first use initializes the firing/behaviour marker and clears uninitialized shared pin bytes. With zero has_pin, clears only an already-valid firing pin. Returns 0 on success, 1 for null. |
| `squad_firing(ai)` | Requires `SquadAiFacet`; reads its raw firing-mode byte, or 0 for null. Does not calculate the selected-soldier UI aggregate. |
| `set_squad_firing(ai, value)` | Requires `SquadAiFacet`; writes that squad byte. 0 success, 1 null. Does not clear soldier overrides. |

Snapshots copy values but do not extend object lifetimes. Embedded pointers can
become stale after a game operation or level transition. These helpers do not
use RTTI to validate the types above: a live pointer to a different kind of object
is not a valid argument. No callback retains objects or
transfers allocation ownership. `selected` is normalized to 0/1; `enabled` is
the raw byte; `raw_mark` is the underlying own-mark byte before enabled/parent
selection filtering. `pin_flags` bit 0 identifies a valid firing marker and bit 1 a
valid posture marker, including when the associated pin byte is zero. A firing pin encodes value+1;
posture pins use 1 standing / 3 prone. Preserve other values when inspecting
data, rather than assuming every byte is boolean. Reserved fields are zero.

Firing is the first consumer of this service. Its setter/query take snapshots,
so virtual getters must remain read-only and stable for the duration of a call.
Public mutations must be coordinated by consumers; the registry does not resolve
conflicting gameplay policies.

## Core services: patch v1 and build v1

Provider `defiance.core`; names `patch` and `build`; version `1`; tables
`PatchV1` / `DefiancePatchV1` and `BuildV1` / `DefianceBuildV1`. Declare a
dependency on Core and query both during init. They install *patch units*:
assembly assembled with `tools/units.py`, one unit per module a plugin patches,
resolved per game build by `tools/variant.py` into
`tools/variants/<build>/units/<plugin>-<module>.{bin,json}`.

- `build.name()` names the build whose units apply: `reference` (the build the
  units are written for, or one Core finds them in by signature) or a layout
  variant. Pick the units resolved for that build.
- `patch.prepare(api, units, count)` checks the units against this build
  (finding their sites by signature where the build calls for it) and links
  each near its module. It writes nothing to the game and returns a handle for
  the process's life, or null (the reason is logged). A unit already linked
  from the same content is reused, so a reloaded plugin gets the same copy.
- `patch.cell(prepared, name)` is the address of a unit's named cell, for the
  plugin to fill before its hooks read it; 0 when no unit has one.
- `patch.install(api, prepared, replacements, count, call_detour)` checks every
  site, then stages them under the calling plugin's ownership: 0 when accepted,
  or nonzero. A managed plugin must propagate failure from `init` so the loader
  discards requests already staged by that plugin.
  `replacements` replace functions the units list as `natives` outright in
  Rust; a non-null `call_detour` takes the units' one call write.
- `patch.contract(api, prepared)` returns the writes accepted by `install`,
  using Core's relocated sites and original bytes. The Feature SDK uses this
  when the loader checks a managed plugin's declared patch contract after init.

Call `prepare`, `cell` and `install` during your own init: the host charges the
writes to the plugin initializing. The Feature SDK calls `contract` after
successful init. Managed writes stay invisible until their group commits; the
host restores them when the plugin is unloaded. The linked code is never freed,
since game code may still be returning through it.
In Rust, `defiance_feature_sdk::units::install` does all of this for units
embedded with `defiance_build_support::embed_units`.

## Internal interfaces

`defiance_feature_sdk` is the built-in plugins' helper crate, not a stable
author API. Patch unit addresses and private Core state are not public
contracts. Loader `test_host` helpers are test infrastructure,
not exports on which a game plugin should depend.

There is currently no public global-variable store, game-thread scheduler,
event bus, hotkey manager, hot reload, or automatic game-object lifetime tracking.

Crash and panic reporting is documented in [crash-reporting.md](crash-reporting.md).

## Core ammo-menu service v1

Provider defiance.core; name ammo-menu; version 1. Declare a direct dependency
on Core and query during init with services::ammo_menu() (Rust) or
DefianceAmmoMenuV1 (C). A missing table means the installed Core is too old;
consumers must refuse before installing hooks. This adds a service, not a change
to the ABI 5 base Api layout.

The immutable table contains two C callbacks:

- uint32_t capacity(void): thread-safe installed slot count. Initially 9.
  Consumers should call at operation time, after startup, rather than snapshotting
  in init; the optional layout provider may initialize later.
- int32_t publish(uint32_t slots): exclusively for the menu-layout provider.
  Accepts multiples of three from 9 through 126. Returns 0 once, 1 for invalid
  counts, 2 for a second publication (including a second publication of 9).

Core owns the atomic state; linking the SDK into multiple DLLs does not duplicate
it. The writer must validate and install every layout patch before publication.
Publication must be the last fallible operation in init, and successful
publication MUST be followed by successful init. Failed publication must roll
back owned patches. There is deliberately no reset or hot unload: menu object
layouts and the published capacity remain valid for the process lifetime.
This is a trusted native plugin contract, not an authorization boundary.

The expanded-menu plugin implements the writer contract; regroup is a reader.
Neither reads the other's config file. Disabled/missing/failed expansion leaves
the stock capacity. Configuring columns alone does not publish a capability.
