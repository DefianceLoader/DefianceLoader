# Writing a loader plugin

A plugin is a DLL that exports one function returning a small C struct. The
loader discovers plugins in `Game/DefianceLoader/plugins`, reads each sidecar
manifest *without loading the DLL*, resolves enablement, dependencies and
conflicts, and initializes in dependency order. A plugin never writes to the
game's memory directly; it asks the loader to find a signature, install a hook,
or look up an RTTI class.

A managed plugin ships `<dll-stem>.plugin.json` beside its DLL (see
[the manifest](#the-manifest)). A manifest-less ABI 5 plugin still loads through
the legacy path, with no declarative enablement or dependency guarantees and its
own `defiance-loader.ini` sections readable through `config_get`.

This crate is the Rust template. The C contract is
[`crates/api/include/defiance.h`](../api/include/defiance.h); the Rust types are
[`crates/api/src/lib.rs`](../api/src/lib.rs). Both must agree, and both carry the
ABI version.

## The contract

```rust
#[no_mangle]
pub unsafe extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: NAME.as_ptr() as *const c_char,
        version: VERSION.as_ptr() as *const c_char,
        init,
        stop: None,
    })
}

unsafe extern "C" fn init(api: *const Api) -> i32 { /* ... */ 0 }
```

`init` runs once, after `logic.dll` and `game.dll` are loaded. Returning non-zero
means the plugin failed: the loader calls `stop` and removes any hooks the
plugin installed. A plugin's `init` and `stop` are the only places it is called;
everything else happens inside a hook it installed.

## The `Api`

| call | does |
| --- | --- |
| `log(level, message)` | the loader's log file and the debugger |
| `module_base(name)` / `module_size(base)` | a loaded module (`logic.dll`, `game.dll`) |
| `find_pattern(base, size, pattern)` | the one match of a signature, or null |
| `find_pattern_at(base, size, pattern, offset)` | as above, `offset` bytes into the window |
| `hook(target, detour, &original)` | redirect, decoding whole instructions |
| `hook_exact(target, detour, displaced, &original)` | the same, with the byte count given |
| `hook_call(site, detour, &original)` | redirect one direct call; `original` is what it reached |
| `unhook(target)` | put the original bytes back |
| `patch_bytes(target, before, after, length)` | compare/replace an owned assembly patch span |
| `rtti_method(class, name)` | a method address, from the loader's name table |
| `vtable_slot(class, slot)` | the function at a vtable slot |
| `config_get(plugin_id, key)` | a validated setting, or null |

Signatures are the text `tools/sigs.py` writes: hex bytes with `??` wildcards,
for example `488b05????????4885c0`. `find_pattern` returns the window's first
address; when the address wanted is inside the window (a branch target, a resume
point), pass its offset to `find_pattern_at`. The signature must match exactly
once or the call returns null.

`hook` works out how many bytes to displace by decoding instructions, and
refuses a site it cannot decode rather than guessing. The trampoline in
`original` runs the displaced instructions and returns to just past them, so a
detour calls it for the stock behaviour. Both `hook` and `hook_exact` reject
relative branches/calls and RIP/EIP-relative operands in the displaced span:
the engine does not relocate those instructions. Use `hook_call` to redirect
a direct call site. Overlapping hook spans are refused, even when their start
addresses differ.

## Settings

A managed plugin declares its settings in its manifest and puts its section in
its declared config group file. The section is the stable plugin ID:

```json
{
  "schema": 1,
  "id": "defiance.example",
  "dll": "defiance_plugin_example.dll",
  "version": "0.1.0",
  "abi": 5,
  "group": "diagnostics",
  "settings": [
    { "key": "enabled", "type": "bool", "default": "true", "description": "..." }
  ],
  "depends": [],
  "conflicts": []
}
```

```ini
; DefianceLoader/config/diagnostics.ini
[defiance.example]
enabled = true
```

```rust
let value = unsafe {
    (api.config_get)(
        b"defiance.example\0".as_ptr() as *const c_char,
        b"enabled\0".as_ptr() as *const c_char,
    )
};
```

The host returns the canonical string of the declared setting; a declared but
unwritten setting returns its default. Null means no such value or the owner is
blocked by invalid configuration, in which case the plugin must not initialize.
The string belongs to the loader and stays valid for the process; copy it to
keep it. Rust plugins should prefer the typed accessors in
`defiance-feature-sdk` (`string`/`boolean`/`integer`, which return an explicit
error). Every setting is startup-only: edit, save, restart. The group file is
generated with commented defaults on first run and never rewritten.

## The manifest

`<dll-stem>.plugin.json` beside the DLL declares the schema version, stable
plugin ID, DLL basename, plugin version, supported host ABI, config group,
settings, hard dependencies by plugin ID with optional `min`/`max` versions,
and conflicts. Discovery reads it without executing the DLL, so a disabled or
blocked plugin is never loaded. Before `init`, the loader checks the plugin's
exported name, ABI and version against the manifest. Built-in manifests are
generated from one authoritative table; a third-party manifest is yours to
write. A known built-in with a missing or mismatched manifest is refused as a
packaging error.

## Build and stage

```powershell
cargo build --release
python tools/stage.py --game "C:\Games\...\bin"   # copies each DLL and its manifest
```

The proxy stays in `Game/bin`, beside `trm.exe`. Plugins default to
`Game/DefianceLoader/plugins`, under the `root` from `bin/defiance-loader.ini`
(default `../DefianceLoader`, resolved against `bin`). To override discovery,
set the unsectioned `plugins` key in that file; relative paths are relative to
`bin`. Staging and uninstall use the same configured path and copy manifests
with their DLLs.

Initialization order comes from the manifest dependencies with the stable plugin
ID breaking ties; a plugin that wants to be late can declare a dependency on one
that does not. A missing or failed dependency blocks the dependent explicitly.
Two plugins cannot hook the same target; the second is refused and told who
owns it.

## ABI

`ABI_VERSION` is checked before `init`. The loader only ever appends fields to
`Api`, so a plugin built against an older, shorter `Api` still reads the fields
it knows. Read `abi_version` and use only what you compiled against.
