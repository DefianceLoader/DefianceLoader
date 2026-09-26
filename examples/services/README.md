# DLLs sharing a service

This game-independent author example demonstrates one provider-owned global
counter, exposed through a versioned function table. `example.counter-user`
queries `example.counter`, increments twice, and verifies that both calls and
the getter reach the same state. No game hooks or raw game pointers are involved.
It also offers a `total` service passing on to the counter, which the third
plugin, `example.counter-watch`, holds and passes on in its own `watch`: a
chain of cached tables, which the tests use to check that a hot reload keeps
alive every old copy a startup-only holder still reaches.

From the repository root:

```powershell
mise run services-test
```

This builds the separate example workspace and the test host, runs loader/API
unit tests, and exercises seven fresh-process scenarios through the production
discovery/planning/loading path: success, disabled provider, failed provider,
missing provider, wrong service version, undeclared dependency, and incompatible
plugin version. Registry unit tests additionally cover pending publication,
duplicate registration, cleanup, size checks, and wrong-thread access.

The parts to copy:

- `contract/src/lib.rs`: a C-compatible table and threading/lifetime contract.
- `provider/src/lib.rs`: the atomic state, callbacks, handshake, and registration.
- `consumer/src/lib.rs`: dependency lookup and calls through the shared table.
- Both `.plugin.json` files: package identity, configuration, and dependency declarations.

For a manual install, close the game and copy these DLLs from
`examples/services/target/release`, each with the matching manifest from its
source folder, to `<game>/DefianceLoader/plugins`:

```text
defiance_example_counter.dll
defiance_example_counter.plugin.json
defiance_example_counter_user.dll
defiance_example_counter_user.plugin.json
```

Use a loader built with the service extension. The consumer logs
`counter-user: shared state verified (two increments)` on success. Remove the
four example files with the game closed to uninstall; optional example config
can remain. These example DLLs are not included in the standard release ZIP.

Settings are under `[example.counter]` and `[example.counter-user]` in
`DefianceLoader/config/examples.ini`. `fail_init=true` deliberately fails the
provider after registration; `service_version=2` deliberately requests an
unsupported table. Both are tutorial test controls, not required design patterns
for real plugins. Restore defaults and restart after experimenting.

The example consumer operates during sequential startup, so it can compare
exact counter values. The service operations themselves are thread-safe, but
multiple concurrent consumers must not assume nobody increments between calls.

Continue with the [author guide](../../docs/plugin-authoring.md) and
[public API reference](../../docs/plugin-api.md). The real selection/regroup
integration shows the same mechanism with stricter game-thread and pointer rules.
