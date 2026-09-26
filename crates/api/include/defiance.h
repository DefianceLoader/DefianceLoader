/* The plugin ABI, for a plugin written in C or C++. This mirrors
 * crates/api/src/lib.rs byte for byte; if you change one, change the other and
 * bump DEFIANCE_ABI_VERSION.
 *
 * A plugin is a DLL that defines:
 *
 *     __declspec(dllexport) const DefiancePlugin *defiance_plugin(void);
 *
 * The returned struct must stay valid for the process. Build as x64, matching
 * the game; MSVC or clang-cl both work, and nothing here depends on the C
 * runtime being shared with the game (do not pass allocator-owning types
 * across the boundary). */
#ifndef DEFIANCE_H
#define DEFIANCE_H

#include <stdint.h>
#include <stddef.h>

#define DEFIANCE_ABI_VERSION 5u

enum {
    DEFIANCE_LOG_INFO = 0,
    DEFIANCE_LOG_WARN = 1,
    DEFIANCE_LOG_ERROR = 2,
};

typedef struct DefianceApi {
    uint32_t abi_version;
    uint32_t reserved; /* must be zero */
    void (*log)(uint32_t level, const char *message);
    void *(*module_base)(const char *name);
    size_t (*module_size)(void *base);
    /* hex with ?? wildcards, as tools/sigs.py writes it; unique match or null */
    void *(*find_pattern)(void *base, size_t size, const char *pattern);
    /* offset bytes into the signature window; offset 0 is find_pattern */
    void *(*find_pattern_at)(void *base, size_t size, const char *pattern, size_t offset);
    /* Decode whole instructions; refuse spans needing relative relocation.
       Distant detours use an owned nearby relay when the span is short. */
    int32_t (*hook)(void *target, void *detour, void **original);
    /* Exact count is never expanded; same validation and relay support as hook. */
    int32_t (*hook_exact)(void *target, void *detour, size_t displaced, void **original);
    /* redirect one direct call; *original gets the address it reached before */
    int32_t (*hook_call)(void *site, void *detour, void **original);
    int32_t (*unhook)(void *target);
    void *(*rtti_method)(const char *class_name, const char *method);
    void *(*vtable_slot)(const char *class_name, size_t slot);
    /* A validated setting, by stable plugin id and key, both case-insensitive.
       Returns the canonical string form: booleans are "true"/"false", integers
       decimal, a choice its declared spelling. A declared but unwritten setting
       still returns its default; null means no such value or the owner is
       blocked by invalid config (a blocked plugin must not initialize). The
       string is owned by the loader and valid for the process's life; copy it
       to keep it. Legacy ABI 5 plugins can still read their own
       defiance-loader.ini sections here. */
    const char *(*config_get)(const char *plugin_id, const char *key);
    /* compare/replace an owned byte span; unhook restores it */
    int32_t (*patch_bytes)(void *target, const uint8_t *before, const uint8_t *after, size_t length);
} DefianceApi;

typedef struct DefiancePlugin {
    uint32_t abi_version; /* must be DEFIANCE_ABI_VERSION */
    const char *name;
    const char *version;
    int32_t (*init)(const DefianceApi *api); /* 0 = loaded */
    void (*stop)(void);
} DefiancePlugin;

/* Optional extension, independent of ABI 5. A plugin may export:
 * __declspec(dllexport) int32_t defiance_plugin_services(const DefianceServiceApiV1 *);
 * Called after identity validation, before init. Cache the pointer (process
 * lifetime), return 0 if accepted. Check version == 1 and size >= sizeof(...).
 * Registration/query run ONLY on the init thread. Providers own permanent,
 * immutable C-compatible tables. Publication is conditional on successful init.
 * Consumers must declare the provider dependency (except for the loader's own
 * DEFIANCE_LOADER_PROVIDER) and cache the resolved table.
 * Exact service version; size is a minimum, not a substitute for matching ABI.
 * register: 0 success, 1 invalid argument, 2 outside init, 3 duplicate.
 * query: NULL for unavailable/version/size/dependency/context failure.
 */
typedef struct DefianceServiceApiV1 {
    uint32_t version;
    uint32_t size;
    int32_t (*register_service)(const char *name, uint32_t version, const void *table, size_t size);
    const void *(*query_service)(const char *provider, const char *name, uint32_t version, size_t min_size);
} DefianceServiceApiV1;

/* Services the loader itself provides, under this provider ID. Any plugin may
 * query them without declaring a dependency; no plugin may register under it.
 */
#define DEFIANCE_LOADER_PROVIDER "defiance.loader"

/* Provider defiance.loader, name crash-ranges, service version 1.
 * Names mod code in crash reports: a fault in a mapped range is reported as
 * label+offset. Attribution only. Callable from any thread once resolved.
 * map: label is UTF-8, 1..128 bytes, printable, no line breaks, copied; a
 * range with the same start replaces the earlier one. 0 success, 1 empty
 * range or invalid label. unmap: 0 success, 1 zero start.
 */
typedef struct DefianceCrashRangesV1 {
    int32_t (*map)(uintptr_t start, uintptr_t end, const char *label);
    int32_t (*unmap)(uintptr_t start);
} DefianceCrashRangesV1;

/* Provider defiance.loader, name trace, service version 1. Diagnostic
 * call-stack tracing with a hardware breakpoint: each hit is logged with its
 * registers and stack, and execution resumes unchanged. Four sites at most,
 * shared with [trace] sites in core.ini. Callable from any thread once
 * resolved. trace: hits 1..1000, address executable code, label as for
 * crash-ranges; the site is released after its hits. 0 success, 1 invalid
 * argument, 2 all sites in use, 3 already traced, 4 tracing unavailable.
 * stop: 0 success, 1 not traced.
 */
typedef struct DefianceTraceV1 {
    int32_t (*trace)(uintptr_t address, uint32_t hits, const char *label);
    int32_t (*stop)(uintptr_t address);
} DefianceTraceV1;

/* Provider defiance.loader, name multiplayer, service version 1. Which active
 * plugins block multiplayer (no multiplayer_safe in their manifest). Any
 * thread. blockers: copies the comma-separated IDs, NUL-terminated, when
 * capacity exceeds their length; returns that length, 0 when nothing blocks,
 * never 0 before startup has finished.
 */
typedef struct DefianceMultiplayerV1 {
    size_t (*blockers)(char *buffer, size_t capacity);
    /* For Core: the guard is installed. Until then the loader starts no plugin
     * that is not multiplayer-safe. */
    void (*guard_installed)(void);
} DefianceMultiplayerV1;

/* Provider defiance.loader, name session, service version 1. For Core:
 * whether a mission is loaded, which hot reload waits on. Any thread.
 * tracking: Core tracks missions from now on. mission: a mission state's
 * constructor (delta 1) or destructor (-1) has returned, so the whole
 * construction and destruction count. before_mission: see the field.
 */
typedef struct DefianceSessionV1 {
    void (*tracking)(void);
    void (*mission)(int32_t delta);
    /* A mission's state is about to be constructed (mission start or save
     * load): it counts as being built until mission(1), and pending reloads are
     * applied now, on the calling thread, after any reload in progress. */
    void (*before_mission)(void);
} DefianceSessionV1;

/* Provider defiance.selection, name selection, service version 1.
 * Game thread only. NULL -> 0; otherwise argument must be a live selectable
 * facet in the supported game build. Returns 0 or 1. Does not retain pointers.
 */
typedef struct DefianceSelectionV1 {
    uint8_t (*is_selected)(void *selectable);
} DefianceSelectionV1;

typedef struct DefianceMemberStateV1 {
    void *selectable;
    void *squad_selectable;
    uint8_t selected, enabled, firing_pin, posture_pin;
    uint8_t pin_flags; /* bit 0 firing marker valid, bit 1 posture marker valid */
    uint8_t raw_mark; /* underlying own-mark byte, independent of parent selection */
    uint8_t reserved[2];
} DefianceMemberStateV1;

/* Provider defiance.core, name game-access, service version 1.
 * All operations require live game objects on their owning game thread.
 * No buffers/pointers are retained. See docs/plugin-api.md for full semantics.
 */
typedef struct DefianceGameAccessV1 {
    void *(*entity_from_facet)(void *facet);
    void *(*selectable)(void *entity);
    /* Required count or SIZE_MAX on unavailable/invalid roster. Capacity 0
       queries size. A short buffer is not written. No ownership is transferred. */
    size_t (*copy_members)(void *entity, void **out, size_t capacity);
    /* Soldier with SquadUnitSelectableFacet; caller guarantees the type. */
    int32_t (*read_member)(void *entity, void *expected_squad, DefianceMemberStateV1 *out);
    /* Soldier SquadUnitSelectableFacet only. */
    int32_t (*set_firing_pin)(void *selectable, uint8_t value, uint8_t has_pin);
    /* Both flag operations below require SquadAiFacet. */
    uint8_t (*squad_firing)(void *ai);
    int32_t (*set_squad_firing)(void *ai, uint8_t value);
} DefianceGameAccessV1;

/* Core service "ammo-menu" v1; see docs/plugin-api.md for publication rules. */
typedef struct DefianceAmmoMenuV1 {
    uint32_t (*capacity)(void);
    int32_t (*publish)(uint32_t slots);
} DefianceAmmoMenuV1;

#endif /* DEFIANCE_H */
