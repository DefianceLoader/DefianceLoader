# Public plugin API

This reference describes the implemented surface, not planned game bindings.
Author workflow: [plugin authoring](plugin-authoring.md). Source of truth:
[Rust ABI](../crates/api/src/lib.rs), [C/C++ header](../crates/api/include/defiance.h),
[Rust SDK](../crates/feature-sdk/src/lib.rs).

## Base ABI 5

`defiance_plugin()` returns a permanent `Plugin` containing `abi_version`,
NUL-terminated UTF-8 `name` and `version`, `init(const Api*) -> i32`, and optional
`stop()`. `init` returns 0 on success. `Api.abi_version` must equal 5 and
`Api.reserved` must equal 0. Do not append fields to this structure yourself.
All ABI callbacks use C calling conventions; game detours must match the
original game's Microsoft x64 signature, not merely the loader callback type.

| Api function | Result and contract |
|---|---|
| `log(level, message)` | Log UTF-8 text. INFO=0, WARN=1, ERROR=2. The message must live through the call. |
| `module_base(name)` | Loaded module base, or null. Does not load modules. |
| `module_size(base)` | Mapped size, or zero. Use a loaded module base. |
| `find_pattern(base, size, pattern)` | Unique match or null; hex bytes with `??` wildcards. Caller supplies a valid readable range. |
| `find_pattern_at(base, size, pattern, offset)` | Unique match plus an offset inside the matched window; otherwise null. A signature match alone is not a supported-build guarantee. |
| `hook(target, detour, original_out)` | Installs an entry detour, decoding whole instructions. On success, `original_out` gets a callable trampoline. |
| `hook_exact(target, detour, displaced, original_out)` | Same, with an exact span. Rejects incomplete instructions; never expands the requested span. |
| `hook_call(site, detour, original_out)` | Redirects one direct rel32 call; other callers stay unchanged. Returns the original callee through the output. |
| `unhook(target)` | Restores the owned hook/patch at that address. Storage remains retained if restoration fails. |
| `rtti_method(class, method)` | Method pointer or null, using the loader's `defiance-rtti.ini` name-to-slot table. Class names are substring matches; RTTI contains no method names. |
| `vtable_slot(class, slot)` | Zero-based virtual slot pointer or null, using the first matching vtable in logic.dll then game.dll. Validate ambiguous class matches yourself. |
| `config_get(plugin_id, key)` | Canonical validated UTF-8 setting or null. IDs/keys are case-insensitive. Loader owns the returned process-lifetime string. |
| `patch_bytes(target, before, after, length)` | Compares expected bytes and installs an owned replacement; mismatch/overlap is refused. Caller supplies valid buffers. |

Hook/patch functions return 0 on success, nonzero on failure. Install during
init so ownership is attributed correctly. Overlapping hooks are refused;
automatic hook chaining is not provided. Entry trampolines currently reject
displaced instructions requiring relative/RIP-relative relocation. Distant
detours can use owned near relays. Failed initialization rolls back owned patches;
an incomplete rollback stops further plugin startup. Never free a detour while
any game code can still reach it.

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
| `query_service(provider, name, version, min_size)` | Returns a table or null. Requires the provider to be an initialized declared dependency, an exact service version, and at least `min_size` bytes. `min_size` must be nonzero. |

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
| `services::selection()` | Resolve the selection-v1 table from `defiance.selection`. |
| `services::game_access()` | Resolve Core's game-access-v1 table; declare a direct `defiance.core` dependency. |
| `services::members(game, entity)` | Copy the roster pointer array using Core's size/capacity protocol. Returns None on unavailability, malformed/changing data, or allocation failure; entities remain borrowed. |

`defiance_api::leak(Plugin)` is a convenience for allocating the permanent
plugin descriptor; call it once from the entry point as the examples do.

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

## Internal interfaces

`defiance_feature_sdk::install`, `install_native`, and `install_pickup_rust`, Core's
`defiance_install_feature_v1`, `defiance_install_pickup_rust_v1`,
`defiance_install_native_v1`, and `defiance_configure_enabled_v1` are built-in implementation interfaces, not
general author APIs. Shared assembly block addresses and private Core state
are not public contracts. Loader `test_host` helpers are test infrastructure,
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
