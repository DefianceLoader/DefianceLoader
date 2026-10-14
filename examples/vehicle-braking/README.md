# Vehicle braking examples

These Rust and C++ examples implement the same vehicle braking adjustment.
Each calls the game's original vehicle update once, then shortens its final
slowdown distance. The default setting, 50, halves that distance; 75 gives an
intermediate window, and 100 preserves stock behavior without installing a
hook. Both plugins affect standard car and tank chassis for every owner in
single-player.

The Rust plugin is `example.vehicle-braking.rust` and builds as
`defiance_example_vehicle_braking_rust.dll`. The C++ plugin is
`example.vehicle-braking.cpp` and builds as
`defiance_example_vehicle_braking_cpp.dll`. Both are version `0.1.0`.

These are source examples for manual installation, excluded from the normal mod
bundle. Use them with Defiance Loader and Core enabled; Core provides the
single-player guard required by these gameplay plugins.

## Build

For C++, install Visual Studio Build Tools with the **Desktop development with
C++** workload and a Windows SDK. The script discovers the x64 compiler and SDK,
uses the static runtime (`/MT`), and needs no Rust or CMake installation.

Rust requires the Windows MSVC target and linker toolchain; this is needed only
for the Rust example. Use mise if available:

```powershell
mise exec -- cargo build --release --manifest-path examples/vehicle-braking/rust/Cargo.toml --target x86_64-pc-windows-msvc
```

You can run the same `cargo build` command directly if mise is unavailable.
The DLL and sidecar are written to
`examples/vehicle-braking/rust/target/x86_64-pc-windows-msvc/release/`.

From the repository root, build C++17 with:

```bat
examples\vehicle-braking\cpp\build.cmd
```

The DLL and sidecar are written to `examples/vehicle-braking/cpp/target/`.
To build both examples, run the repository task:

```powershell
mise run examples-build
```

## How the plugins work

Both use public ABI 5: Rust imports `defiance-api`; C++ includes
[`defiance.h`](../../crates/api/include/defiance.h). Each manifest matches its
plugin ID, DLL basename, and version. The loader calls `init` on its startup
thread after the game modules load. `init` validates the API version and
reserved fields, reads the setting, resolves a guarded signature, and requests
a managed hook. After successful initialization, the loader reads
`defiance_patch_contract_v1` and checks the expected-write contract before
committing the hook. `init` does not access live game objects.

Each language uses the complete `logic.dll` callback signature with only
relocation-dependent bytes wildcarded. The signature guards the body containing
the native field accesses, and a separate ten-byte `ENTRY_BEFORE` check guards
the hook entry. Both implementations name these checked offsets relative to the
callback objects:

| Field | Offset | Meaning |
| --- | ---: | --- |
| `CHASSIS_MODE` / `MOVEMENT_MODE` | `0x320` | Chassis mode; only mode 1 is adjusted. |
| `REQUESTED_SPEED` / `SPEED` | `0x1d0` | Requested speed after slowdown. |
| `ARRIVAL_RATIO` / `SLOWDOWN` | `0x1d4` | Final destination slowdown ratio. |
| `VELOCITY_X` | `0x200` | Requested velocity, x component. |
| `VELOCITY_Y` | `0x204` | Requested velocity, y component. |
| `VELOCITY_Z` | `0x208` | Requested velocity, z component. |

The game-thread detour preserves the native callback signature and calls the
original update exactly once. It adjusts output only for ordinary path movement
when the ratio is between zero and one, the requested speed is nonnegative, and
all computed outputs are finite. Other cases keep the original result. The
loader owns hook installation, rollback, removal, and the trampoline. The
trampoline output is stored in permanent plugin state and may be filled after
`init`, when the managed request commits; the callback calls it only once the
hook is active. Neither implementation writes raw code bytes or builds its own
trampoline.

For a fractional stock ratio, the new ratio is
`min(stock_ratio * 100 / braking_window_percent, 1)`. Multiplying the requested
speed and velocity by `new_ratio / stock_ratio` keeps their proportions while
capping them at the native unbraked output. Validation finishes before any
field is written, so an invalid output leaves all stock fields intact.

Keep the plugin descriptor, its strings, and callback state alive as long as
the loader can call them. Callback object pointers are borrowed and valid only
inside that callback; do not retain them. Follow the public API's
[function-specific ownership and lifetime rules](../../docs/plugin-api.md).
Never let a Rust panic or C++ exception cross the ABI boundary.

Compare the matching implementations: [Rust](rust/src/lib.rs) and
[C++](cpp/main.cpp).

| Step | Rust | C++ |
| --- | --- | --- |
| Read configuration and find the callback | `prepare` | `read_percent`, `resolve` |
| Request the managed hook | `init` | `init` |
| Call the original and adjust its output | `arrival_update` | `vehicle_arrival_update`, `tighten` |
| Describe the guarded write | `defiance_patch_contract_v1` | `defiance_patch_contract_v1` |

## Configuration and install

Both manifests use group `examples`, are enabled by default, and set
`hot_reload` to false. Settings take effect at startup. Put the selected plugin's
settings in `DefianceLoader/config/examples.ini`:

```ini
[example.vehicle-braking.rust]
enabled = true
braking_window_percent = 50
```

For C++, use its ID and the same values instead:

```ini
[example.vehicle-braking.cpp]
enabled = true
braking_window_percent = 50
```

`braking_window_percent` accepts any integer from 50 through 100, inclusive.
At 50 the final slowdown distance is half of stock; 75 is an intermediate
setting; 100 leaves stock braking and installs no hook. The plugins conflict
with one another and with the production `defiance.vehicle-arrival` plugin. If
Vehicle Arrival is installed, disable it in
`DefianceLoader/config/vehicles.ini` while using either example:

```ini
[defiance.vehicle-arrival]
enabled = false
```

With the game closed, copy one DLL and its same-stem `.plugin.json` sidecar to
`<game>/DefianceLoader/plugins`. Do not install both examples together. Remove
the selected DLL and sidecar to uninstall it; the config file can remain.

At 50%, expect the log substring
`vehicle braking: installed (50% braking window)`. At 100%, expect
`vehicle braking: stock braking; no hook installed`. These appear in
`DefianceLoader/logs/defiance-loader.log`. Check the loader startup summary first
if neither line appears.

An invalid setting logs the accepted 50–100 range. A missing or ambiguous
signature, changed entry bytes, or refused hook logs an error and fails
initialization. Read the startup summary to confirm the loader accepted the
plugin; the plugin's own success message precedes the loader's final commit.

## Build and test tasks

From the repository root, the examples have dedicated tasks:

```powershell
mise run examples-build
mise run examples-test
mise run examples-native-test
```

`examples-test` loads the actual DLLs in the game-free mock host.
`examples-native-test` uses local game fixtures to check requested speeds. These
checks do not establish physical stopping accuracy. Test each language DLL
separately in game at 50 and 100, using both wheeled and tracked vehicles.
Compare straight approaches, short moves, queued waypoints, reverse movement,
blocked destinations, and overshoot. Physical stopping behavior still needs
in-game confirmation; include the loader startup summary and relevant lines
from `DefianceLoader/logs/defiance-loader.log` when reporting results.

## Adapting the example

Use this checklist when turning a copy into your own mod:

- Choose a unique plugin ID and update the descriptor, manifest, and config
  section together.
- Rename the Rust package/Cargo library or C++ output DLL. Update the manifest's
  `dll` value and filename, and its copy step in `build.rs` or `build.cmd`.
- Update the version in the descriptor and manifest, plus Rust's `Cargo.toml`.
  Review the settings, config group, conflicts, and multiplayer/reload flags
  for the new feature.
- Preserve both ABI entry exports: `defiance_plugin` and
  `defiance_patch_contract_v1`. Keep their returned storage alive for the DLL's
  lifetime.

For a Rust project outside this repository, change the `defiance-api` path in
its `Cargo.toml` to point to the crate in your checkout. For C++, change the
include directory in `build.cmd` to that checkout's
`crates/api/include` directory. Keep the full-body signature, entry bytes,
expected-write contract, and named offsets aligned with the verified target
build.
