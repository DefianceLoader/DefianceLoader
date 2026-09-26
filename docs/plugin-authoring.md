# Writing a Defiance plugin

Start with the runnable [service examples](../examples/services/README.md).
They build two real DLLs and use the production loader to test discovery,
configuration, dependency ordering, initialization, and shared state. No game
files are required for those examples. The older [single-plugin template](../plugins/example/src/lib.rs)
shows the basic entry point and commented hook setup.

The [public API reference](plugin-api.md) lists supported functions and their
constraints. Rust definitions live in `crates/api`; C/C++ definitions are in
`crates/api/include/defiance.h`. Both target Windows x64. Rust convenience
wrappers live in `crates/feature-sdk`.

## Create and build

1. Copy an example crate, choose a unique plugin ID such as `yourname.feature`,
   and give the DLL a distinct filename. Keep its Cargo `crate-type = ["cdylib"]`.
   Inside this repository, add the crate to its chosen workspace. Outside it,
   adjust the API/SDK path dependencies to your checkout and pin the SDK revision
   you build against. Shared contract crates contain types, not shared storage.
2. Export `defiance_plugin()` returning a permanent `Plugin`. Keep the exported
   ID, version, DLL filename, and manifest in agreement. Validate the incoming
   API version and `reserved == 0` before using it. Return zero from `init` only
   when all required setup succeeded.
3. Include a sidecar named exactly like the DLL with `.dll` replaced by
   `.plugin.json`. Follow the example manifests. Declare settings, dependencies,
   and conflicts there. `group` selects a config file, not a dependency.
   Add `"multiplayer_safe": true` only if the plugin changes nothing another
   player's game would need to match (display only). Without it, an active
   plugin blocks multiplayer: the loader's `multiplayer` service lists it, and
   the game refuses to connect online while it is active. Such a plugin also
   starts only after Core has installed that guard, so it cannot run in an
   install without Core.
   Add `"hot_reload": false` if the plugin can only be loaded at startup.
   Otherwise the development hot reload may unload it while the game runs: its
   `stop` must end its own threads, and it must leave no pointer to its code
   outside the hooks and services the loader tracks. Its service functions
   must keep working after `stop`: a consumer that can only load at startup
   keeps calling the old copy, which stays loaded for it. State it left in game
   objects stays for the next copy.
4. Build from the repository root with `mise exec -- cargo build --release
   --manifest-path path/to/Cargo.toml`. Compile DLLs and the game for x64.
   To build and test the included service example, run `mise run services-test`.

`init` runs on the loader's startup thread after game modules are present. It
is appropriate for resolving addresses, reading configuration, registering
services, and installing hooks. It is **not** a game-thread callback. Game-object
operations must run in a verified game-thread hook with live object pointers.

## Add to an installation

Close the game. Copy the DLL **and its sidecar manifest** to
`<game>/DefianceLoader/plugins`, beside the other plugins, not into `bin/plugins`.
The proxy stays at `<game>/bin/dxgi.dll` beside `trm.exe`.

On startup, managed settings are materialized in
`DefianceLoader/config/<group>.ini`, under the plugin's ID. Existing user values
are preserved. Read them through `config_get` or the typed SDK functions; do
not add another INI parser. Configuration changes require restarting the game.

Check the loader log for your plugin's active/blocked/failed state. Missing,
disabled, incompatible, or failed dependencies block consumers; they are never
silently enabled. `defiance-config --game <game>/bin --report` prints every
resolved setting and where it came from; `--game` takes the directory holding
`trm.exe` (the game folder above it also works). `defiance-config --help` lists
the other options. Do not overwrite users' INI files in an upgrade.

## Share functionality with another plugin

1. Define an immutable `#[repr(C)]` function table in a small contract crate,
   with a matching C declaration. Document a provider ID, service name, exact
   service version, parameter types, ownership, threading, and lifetime rules.
2. Export the optional handshake with
   `defiance_feature_sdk::service_handshake!();`. The loader supplies the
   process-lifetime service API before `init`; the original ABI-5 `Api` is unchanged.
3. During provider `init`, use `services::register(c"name", 1, &STATIC_TABLE)`.
   The loader assigns the current plugin's identity; providers cannot publish
   under a different plugin ID. Registration becomes visible only after init
   returns success. Failure discards every registration from that attempt.
4. The consumer declares the provider in `depends`, then calls
   `services::query::<Table>(c"provider.id", c"name", 1)` during its own init.
   Fail initialization if a required service is missing. Cache the returned table
   for later calls on the threads allowed by that service.

Lookup requires both an active provider and a declared dependency. It does not
initialize a provider, add dependencies, or choose a different version. An
optional service still needs a declared dependency in this initial design;
optional dependency discovery is not implemented.

Plugin version ranges and service versions have different jobs: manifest
versions order/check packages; the service version identifies a table's ABI.
Bump the service version for incompatible layouts or semantics. Multiple
versions may coexist. A minimum table size check catches truncation, not a
wrong type or incompatible semantics.

Store shared state in the provider DLL. For example, the tutorial counter uses
an atomic global internally and exposes get/increment operations. Putting a
global in a Rust crate linked separately into two DLLs creates separate state.
Do not expose a writable global map or pass Rust `String`, `Vec`, references
with undocumented lifetimes, or allocator ownership across the DLL boundary.

## Failure, stopping, and compatibility

There is no supported hot reload or live plugin removal. DLLs and published
tables must remain mapped for the process lifetime. Consumers stop before
providers; stop and join your own workers before provider state becomes unusable.
Removing a registration prevents new discovery; it cannot revoke cached pointers.
Registration and lookup are allowed only on the thread executing `init`.
Callback threading is a separate part of each service's contract.

If init fails, the loader calls `stop` when supplied and rolls back owned hooks.
Be prepared for `stop` after partial initialization. Registration rollback does
not undo side effects from arbitrary service calls: providers must document
whether operations are reversible. No foreign-language exception or Rust panic
may cross a callback boundary.

Older ABI-5 plugins continue to work without the service handshake. Older
loaders do not deliver it: a service-requiring plugin must fail cleanly when the
SDK reports no service API. Upgrade loader and provider together when introducing
a new required service. Services are cooperation between trusted native DLLs,
not a security sandbox or validation of arbitrary game pointers.

## Test and distribute

Test pure decisions without a game, then exercise the compiled DLL through a
host fixture. Cover disabled/missing dependencies, wrong versions, failed init,
duplicate registration, cleanup, and configuration errors. For game hooks, add
build-specific verification and an in-game smoke test; synthetic objects alone
cannot establish live object lifetimes or correct reverse-engineered offsets.

Publish a ZIP with the DLL, matching sidecar, brief installation/removal notes,
supported game builds, required provider versions, and documented service
contracts. Config defaults belong in the manifest. The tutorial DLLs are built
in a separate workspace and are not added to the normal game release package.
