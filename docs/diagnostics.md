# Diagnostics probes

`defiance.diagnostics` uses the loader's buffered `trace-capture` service to
observe function calls without changing game code. The plugin is enabled by
default, but no probes are active until the configuration enables them. It
does not install diagnostic patch units and has no dependency on Core, so it
can run while Core is disabled.

The default probe file is
`DefianceLoader/config/diagnostics-probes.json`. Set `probe_file` in
`DefianceLoader/config/diagnostics.ini` to an absolute path or to a filename
under the loader's config directory. A bootstrap `root` override changes that
directory too. The loader resolves the filename before passing it to the
plugin, and the startup log reports the full path. The worker checks the file
every 250 ms. A missing file produces a warning and means an empty probe list.
A valid edit replaces the active
definitions without rebuilding the DLL or restarting the game. If parsing or
site resolution fails, the log explains the failure and the last valid set
remains active. If arming the replacement fails, the plugin reports the failed
site and restores the previous definitions when possible; a failed restoration
is logged as an error. Probe-file messages use the `diagnostics:` log prefix.

Probe hits and census reports are written at debug
([log levels](development.md#log-levels)); arming, site resolution and the
finished summary are info. To read hits, set `level = debug` under `[logging]`
in `core.ini`. To focus the written log on selected plugins, use the
[plugin log filters](development.md#filter-plugin-logs). Diagnostics messages
carry the `[defiance.diagnostics]` source tag. Filtering output does not stop
an enabled probe's capture work.

Start with the [sample configuration](diagnostics-probes.json). Its preset
entries and custom example are disabled. Change `enabled` to `true` only for
the probes you want. The available presets are `selection-select`,
`selection-toggle`, `selection-deselect`, `selection-clear`, and
`behaviour-census`.

## Custom probes

A custom probe selects a module (`logic.dll` by default, or `game.dll`) and a
site in one of two ways:

- `signature` is a byte pattern with hexadecimal bytes and `??` wildcards. It
  must resolve uniquely in the module's original view, before loader-owned
  writes. `site_offset` selects a byte within the matched signature window as
  the capture site. An optional `rva` asserts the expected RVA of that site.
- `rva` requires `module_sha256`, making the address specific to an exact module
  build.

For either custom site form, the address must start an instruction. Signature
matching verifies bytes and uniqueness; it does not decode instruction boundaries.

Captures copy data without invoking game methods. Diagnostics that depend on
virtual calls or custom before/after correlation still need Rust logic. Any
values used later by a decoder must be copied while they remain valid at the hit.

A probe can define up to 16 named `fields`. Each field has a `register`, a
`type` (`u8`, `u16`, `u32`, `u64`, `f32`, or `f64`), and an optional `path` of
up to four signed offsets. Registers are `rax`, `rcx`, `rdx`, `rbx`, `rsp`,
`rbp`, `rsi`, `rdi`, `r8` through `r15`, and `rip`. With no path, the field
captures the low bits selected by its type (for example, a `u32` field keeps
the low 32 bits of the register). For a path, each offset is added in turn;
intermediate steps read a pointer and the final step reads the requested type.
A failed read marks that field `unreadable` in an event.

Optional capture settings are `hits` (1–1,000,000, default 1,000), `every`
(positive sampling interval, default 1), `interval_ms` (minimum time between
captured events, default 0 for presets, 100 for custom events, and 0 for custom
census), and `stack_frames` (0–16, default 8 for events and 0 for census).
`mode` is `events` by default; `census` aggregates observations by the field
named `caller`, which must be a `u64` field in a custom census probe. For event
probes, the first requested stack frame is the current instruction pointer;
the rest come from a bounded stack scan and are marked `?` in the log, not
unwound frames. An optional `filter` names one configured field and compares
its value under an optional mask:

```json
"filter": { "field": "entity", "value": "0x1234", "mask": "0xffff" }
```

Integer values can be JSON integers or hexadecimal strings. A field used by a
filter must be readable and match the masked raw bits, including IEEE-754 bits
for float fields. The hit budget counts all
site encounters, including filtered and sampled encounters.

## Limits and output

There are four hardware breakpoint slots shared by all diagnostics probes,
the loader's `[trace] sites`, and plugins using the trace-v1 or trace-capture
services. At most four probes can be enabled, and a slot occupied by any of
those other tracing users reduces that number. If a definition cannot be
armed because the slots are full, the log reports the refusal and the previous
definitions are restored when possible.

An enabled probe incurs hardware-breakpoint handler work on every encounter.
`every`, `interval_ms`, and `filter` reduce captured or logged events, but do
not avoid that per-hit cost. Probes default to off and have a finite hit budget.
After the budget is reached, capture stops and the helper clears debug registers
on its next periodic update, within two seconds. Breakpoint exceptions can
continue during that interval. The probe retains its slot until disabled or edited.

The loader captures register, field, and stack data into a fixed 128-event
queue for each site before resuming the game thread. The diagnostics worker
drains that queue and writes formatted events to `defiance-loader.log`; the
exception handler never calls plugin code. When a probe reaches its hit limit,
the log reports its `hits`, `captured`, and `dropped` counts. A full queue
drops events and increments `dropped`. `hits` includes every encounter, while
`captured` counts events placed in the queue. After the hit limit, the site
becomes inactive but keeps its slot and queued results until the probe is
stopped by a valid configuration change or the plugin unloads. Disable or edit
a completed probe to release its slot.

The `behaviour-census` preset groups observations by caller and periodically
reports caller counts; it aggregates in the plugin worker rather than emitting
one event line per call. Census stores at most 256 distinct callers per probe.
Samples for additional caller values are counted as overflow and included in
the hit-limit summary. For the service ABI and standalone injector commands,
see [Plugin API](plugin-api.md) and [Development](development.md).
