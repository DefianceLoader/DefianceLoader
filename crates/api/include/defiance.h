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
    DEFIANCE_LOG_DEBUG = 3, /* only with [logging] level = debug */
};

typedef struct DefianceApi {
    uint32_t abi_version;
    uint32_t reserved; /* must be zero */
    /* The API table binds log messages to its plugin ID, on any thread. */
    void (*log)(uint32_t level, const char *message);
    void *(*module_base)(const char *name);
    size_t (*module_size)(void *base);
    /* hex with ?? wildcards, as tools/sigs.py writes it; unique match or null */
    void *(*find_pattern)(void *base, size_t size, const char *pattern);
    /* offset bytes into the signature window; offset 0 is find_pattern */
    void *(*find_pattern_at)(void *base, size_t size, const char *pattern, size_t offset);
    /* Managed requests stage until their group commits before the next legacy
       initializer or at startup end. Decode whole instructions. */
    int32_t (*hook)(void *target, void *detour, void **original);
    /* Exact count is never expanded; same validation as hook. */
    int32_t (*hook_exact)(void *target, void *detour, size_t displaced, void **original);
    /* redirect one direct call; managed requests commit with their group */
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
    /* compare/replace an owned span; managed writes stage until plan commit */
    int32_t (*patch_bytes)(void *target, const uint8_t *before, const uint8_t *after, size_t length);
} DefianceApi;

typedef struct DefiancePlugin {
    uint32_t abi_version; /* must be DEFIANCE_ABI_VERSION */
    const char *name;
    const char *version;
    /* 0 = initialized; managed plan conflicts may still call stop() */
    int32_t (*init)(const DefianceApi *api);
    void (*stop)(void);
} DefiancePlugin;

/* Optional versioned export, separate from ABI 5's DefiancePlugin layout. */
#define DEFIANCE_PATCH_KIND_ENTRY 1u
#define DEFIANCE_PATCH_KIND_BYTES 2u
#define DEFIANCE_PATCH_KIND_CALL 3u

typedef struct DefiancePatchContractEntryV1 {
    const char *module; /* loaded module basename, e.g. "logic.dll" */
    size_t rva;
    uint32_t kind; /* one of DEFIANCE_PATCH_KIND_* */
    const uint8_t *before;
    size_t before_len;
    /* Optional exact replacement; null/zero for runtime-linked branches/hooks. */
    const uint8_t *after;
    size_t after_len;
} DefiancePatchContractEntryV1;

typedef struct DefiancePatchContractV1 {
    uint32_t version; /* must be 1 */
    uint32_t size; /* must be at least sizeof(DefiancePatchContractV1) */
    const DefiancePatchContractEntryV1 *entries;
    size_t count;
} DefiancePatchContractV1;

/* Optional export: const DefiancePatchContractV1 *fn(const DefianceApi *api). */
typedef const DefiancePatchContractV1 *(*DefiancePatchContractFnV1)(const DefianceApi *api);

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

/* Provider defiance.loader, name near-memory, service version 1. 64K slots the
 * loader holds near logic.dll and game.dll from the moment each loads.
 * take: commits size (1..0x10000) bytes of executable, writable memory in a
 * held slot within rel32 reach of hint; returns its address, released with
 * VirtualFree(address, 0, MEM_RELEASE), or 0 when none reaches hint.
 * Callable from any thread once resolved.
 */
typedef struct DefianceNearMemoryV1 {
    uintptr_t (*take)(uintptr_t hint, uintptr_t size);
} DefianceNearMemoryV1;

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

#define DEFIANCE_TRACE_FIELDS 16
#define DEFIANCE_TRACE_FRAMES 16
#define DEFIANCE_TRACE_REGISTERS 17
#define DEFIANCE_TRACE_U8 1
#define DEFIANCE_TRACE_U16 2
#define DEFIANCE_TRACE_U32 3
#define DEFIANCE_TRACE_U64 4
#define DEFIANCE_TRACE_F32 5
#define DEFIANCE_TRACE_F64 6
#define DEFIANCE_TRACE_NO_FILTER UINT32_MAX

/* Registers: rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15, rip.
 * Depth 0 captures the register's low bits at the declared width.
 * Depth 1..4 adds each signed offset, reads
 * pointers at intermediate steps, then reads kind at the final address.
 * Floating-point values are raw bits. Unreadable paths clear the valid bit. */
typedef struct DefianceTraceFieldV1 {
    uint32_t reg, kind;
    int32_t offsets[4];
    uint32_t depth, reserved;
} DefianceTraceFieldV1;

typedef struct DefianceTraceRequestV1 {
    uint32_t size, hits;
    uintptr_t address;
    uint32_t every, min_interval_ms, field_count, stack_frames;
    uint32_t filter_field, reserved;
    uint64_t filter_value, filter_mask;
    DefianceTraceFieldV1 fields[DEFIANCE_TRACE_FIELDS];
} DefianceTraceRequestV1;

typedef struct DefianceTraceEventV1 {
    uintptr_t address;
    uint64_t sequence, timestamp_ms;
    uint32_t thread_id, valid_fields;
    uint64_t registers[DEFIANCE_TRACE_REGISTERS];
    uint64_t values[DEFIANCE_TRACE_FIELDS];
    uint32_t frame_count, scanned_frames;
    uintptr_t frames[DEFIANCE_TRACE_FRAMES];
} DefianceTraceEventV1;

typedef struct DefianceTraceStatsV1 {
    uint64_t hits, captured, dropped;
    uint32_t active, reserved;
} DefianceTraceStatsV1;

/* Provider defiance.loader, name trace-capture, version 1. Plans/events are
 * copied; no plugin callbacks run in the exception handler. Four sites shared
 * with trace-v1. Open only during init; all other operations any thread.
 * Failed init/unload revokes sessions. Request size must match, reserved=0,
 * hits=1..1000000, every>=1, field_count<=16, stack_frames<=16.
 * A filter requires a readable field and masked equality. Hits count all
 * encounters. Finished sites retain their queue and slot until stop/close.
 * open: nonzero session or 0. start: 0 success, 1 invalid, 2 full, 3 duplicate,
 * 4 unavailable, 5 closed session. poll: count copied or negative invalid;
 * events may be NULL only for capacity=0, stats may be NULL. stop/close:
 * 0 success or 1 unknown/closed. Handles never identify a replacement site. */
typedef struct DefianceTraceCaptureV1 {
    uint64_t (*open)(void);
    int32_t (*start)(uint64_t session, const DefianceTraceRequestV1 *request, uint64_t *handle);
    int32_t (*poll)(uint64_t handle, DefianceTraceEventV1 *events, size_t capacity, DefianceTraceStatsV1 *stats);
    int32_t (*stop)(uint64_t handle);
    int32_t (*close)(uint64_t session);
} DefianceTraceCaptureV1;

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

/* Provider defiance.loader, name original, service version 1. Memory as it was
 * before any plugin hooked or patched it through the loader, so checking bytes
 * a plugin only reads or calls does not depend on start order; check the live
 * bytes where you write. Any thread. read: copies length bytes at address into
 * out with the loader-owned writes they overlap undone; the range must lie in
 * one readable region. 0 success, 1 null argument or zero length, 2 not
 * readable.
 */
typedef struct DefianceOriginalV1 {
    int32_t (*read)(uintptr_t address, uint8_t *out, size_t length);
} DefianceOriginalV1;

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

typedef int32_t (*DefianceAmmoStepHandlerV1)(void *menu, void *widget,
                                             int32_t direction);
typedef struct DefianceAmmoStepV1 {
    void (*set_handler)(DefianceAmmoStepHandlerV1 handler);
} DefianceAmmoStepV1;

/* A function a patch unit names in its natives, replaced by detour. */
typedef struct DefianceNativeReplacementV1 {
    const char *name;
    void *detour;
} DefianceNativeReplacementV1;

/* One patch unit: its JSON descriptor and its blob; not retained. */
typedef struct DefiancePatchUnitV1 {
    const uint8_t *descriptor;
    size_t descriptor_len;
    const uint8_t *code;
    size_t code_len;
} DefiancePatchUnitV1;

/* Core service "patch" v1: prepare, fill cells, install during init; contract
 * reads the accepted writes after successful init. See docs/plugin-api.md. */
typedef struct DefiancePatchV1 {
    void *(*prepare)(const DefianceApi *api, const DefiancePatchUnitV1 *units, size_t count);
    size_t (*cell)(void *prepared, const char *name);
    int32_t (*install)(const DefianceApi *api, void *prepared,
                       const DefianceNativeReplacementV1 *replacements, size_t count,
                       void *call_detour);
    const DefiancePatchContractV1 *(*contract)(const DefianceApi *api, void *prepared);
} DefiancePatchV1;

/* Core service "build" v1: the recognised build's name, process lifetime. */
typedef struct DefianceBuildV1 {
    const char *(*name)(void);
} DefianceBuildV1;

#endif /* DEFIANCE_H */
