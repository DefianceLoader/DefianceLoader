// A complete plugin using only the public C ABI. See ../README.md for the walkthrough.
#include "defiance.h"

#include <algorithm>
#include <array>
#include <charconv>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <intrin.h>
#include <limits>
#include <system_error>

#if !defined(_M_X64)
#error Build this plugin for Windows x64, matching the game.
#endif

namespace {

constexpr char PLUGIN_ID[] = "example.vehicle-braking.cpp";

// This complete callback signature comes from Vehicle Arrival. Only relocation-
// dependent displacements are wildcards; the host requires exactly one match.
constexpr char ARRIVAL_PATTERN[] =
    "48895c2410574883ec20488bf9488bda8b892003000083e901743d83f9020f85????????"
    "488b07488bcfff5058f30f1183d00100008b87540300008987640400008b875803000089"
    "8768040000488b5c24384883c4205fc3f6824402000001488bcb4889742430488db200"
    "020000488bd67407e8????????eb05e8????????488b07488bcfff505848638bc8020000"
    "f30f1183d001000085c975050f57d2eb45f68419a702000002744e8d04490f57d24863c8"
    "f30f108c8b54020000f30f5c8be8010000f30f10848b5c020000f30f5c83f0010000"
    "f30f59c9f30f59c0f30f58c8f30f51d1f30f1083340200000f2fd07706f30f5ed0eb08"
    "f30f1015????????f30f1193d4010000f30f5993d0010000f30f1193d00100000f28c2"
    "f30f59060f28caf30f1106f30f594e04f30f114e04f30f595608f30f115608488b742430"
    "488b5c24384883c4205fc3";
constexpr uint8_t ENTRY_BEFORE[] = {
    0x48, 0x89, 0x5c, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x20,
};
constexpr size_t PATTERN_BYTES = (sizeof(ARRIVAL_PATTERN) - 1) / 2;

// Verified native fields, relative to the objects passed to this callback.
constexpr size_t MOVEMENT_MODE = 0x320; // Chassis u32: 1 is ordinary path movement.
constexpr size_t SPEED = 0x1d0;         // Record f32: desired speed after slowdown.
constexpr size_t SLOWDOWN = 0x1d4;      // Record f32: destination slowdown ratio.
constexpr size_t VELOCITY_X = 0x200;    // Record f32: desired velocity, x component.
constexpr size_t VELOCITY_Y = 0x204;    // Record f32: desired velocity, y component.
constexpr size_t VELOCITY_Z = 0x208;    // Record f32: desired velocity, z component.

using Update = void (*)(void *chassis, void *record);
// The loader publishes into this process-lifetime slot with a release store
// before activating the hook. Its ABI requires naturally aligned pointer storage.
alignas(sizeof(void *)) void *original_update = nullptr;
float braking_multiplier = 1.0f;

void log(const DefianceApi *api, uint32_t level, const char *message) {
    if (api != nullptr && api->log != nullptr) {
        api->log(level, message);
    }
}

bool valid_api(const DefianceApi *api) {
    if (api == nullptr) {
        return false;
    }
    if (api->abi_version != DEFIANCE_ABI_VERSION || api->reserved != 0) {
        return false; // Do not call through a table whose layout is incompatible.
    }
    return true;
}

bool read_percent(const DefianceApi *api, int &percent) {
    if (api->config_get == nullptr) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: host API is missing config_get");
        return false;
    }
    const char *value = api->config_get(PLUGIN_ID, "braking_window_percent");
    if (value == nullptr) {
        log(api, DEFIANCE_LOG_ERROR,
            "vehicle braking: missing braking_window_percent; install the matching manifest");
        return false;
    }
    // The host supplies canonical decimal strings and manifest defaults. Check
    // the whole string and the range so a malformed host value cannot install.
    const char *end = value + std::strlen(value);
    const auto parsed = std::from_chars(value, end, percent);
    if (parsed.ec != std::errc{} || parsed.ptr != end || percent < 50 || percent > 100) {
        log(api, DEFIANCE_LOG_ERROR,
            "vehicle braking: braking_window_percent must be an integer between 50 and 100");
        return false;
    }
    return true;
}

struct Site {
    void *address;
    size_t rva;
};

bool resolve(const DefianceApi *api, Site &site) {
    if (api->module_base == nullptr || api->module_size == nullptr || api->find_pattern == nullptr) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: host API is missing module discovery");
        return false;
    }
    void *base = api->module_base("logic.dll");
    const size_t size = base == nullptr ? 0 : api->module_size(base);
    const auto start = reinterpret_cast<uintptr_t>(base);
    if (size < PATTERN_BYTES || size > std::numeric_limits<uintptr_t>::max() - start) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: logic.dll is unavailable or has invalid bounds");
        return false;
    }
    void *address = api->find_pattern(base, size, ARRIVAL_PATTERN);
    if (address == nullptr) {
        log(api, DEFIANCE_LOG_ERROR,
            "vehicle braking: callback signature is missing or ambiguous; no hook installed");
        return false;
    }
    const auto target = reinterpret_cast<uintptr_t>(address);
    if (target < start || target - start > size - PATTERN_BYTES) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: callback is outside logic.dll; no hook installed");
        return false;
    }
    if (std::memcmp(address, ENTRY_BEFORE, sizeof(ENTRY_BEFORE)) != 0) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: callback entry bytes changed; no hook installed");
        return false;
    }
    site = {address, static_cast<size_t>(target - start)};
    return true;
}

// memcpy supports the game's native object layout without assuming C++ object
// types or alignment for the memory owned by the game.
template <typename T> T read_field(const void *object, size_t offset) {
    T value;
    std::memcpy(&value, static_cast<const uint8_t *>(object) + offset, sizeof(value));
    return value;
}

template <typename T> void write_field(void *object, size_t offset, T value) {
    std::memcpy(static_cast<uint8_t *>(object) + offset, &value, sizeof(value));
}

void tighten(void *chassis, void *record) {
    if (read_field<uint32_t>(chassis, MOVEMENT_MODE) != 1 || braking_multiplier == 1.0f) {
        return;
    }
    const float ratio = read_field<float>(record, SLOWDOWN);
    // Fractional slowdown identifies the final destination. Zero (stationary),
    // one (cruise/corners), NaN, and invalid native outputs remain untouched.
    if (!(0.0f < ratio && ratio < 1.0f)) {
        return;
    }
    const float next_ratio = std::min(ratio * braking_multiplier, 1.0f);
    const float scale = next_ratio / ratio; // Cap at the unbraked native speed.
    constexpr std::array<size_t, 4> offsets = {SPEED, VELOCITY_X, VELOCITY_Y, VELOCITY_Z};
    std::array<float, 4> next{};
    if (read_field<float>(record, SPEED) < 0.0f) {
        return;
    }
    for (size_t i = 0; i < offsets.size(); ++i) {
        next[i] = read_field<float>(record, offsets[i]) * scale;
        if (!std::isfinite(next[i])) {
            return; // Validate every output before changing any field.
        }
    }
    write_field(record, SLOWDOWN, next_ratio);
    for (size_t i = 0; i < offsets.size(); ++i) {
        write_field(record, offsets[i], next[i]);
    }
}

void vehicle_arrival_update(void *chassis, void *record) {
    // The trampoline executes the displaced instructions and the stock body.
    // Calling it exactly once preserves all native work before our adjustment.
    // Interlocked reads atomically with an acquire barrier, matching the host's
    // publication through void** without assuming a std::atomic object layout.
    const auto original = reinterpret_cast<Update>(
        _InterlockedCompareExchangePointer(&original_update, nullptr, nullptr));
    original(chassis, record);
    tighten(chassis, record);
}

int32_t init(const DefianceApi *api) {
    if (!valid_api(api)) {
        return 1;
    }
    int percent = 0;
    if (!read_percent(api, percent)) {
        return 1;
    }
    if (percent == 100) {
        log(api, DEFIANCE_LOG_INFO, "vehicle braking: stock braking; no hook installed");
        return 0;
    }
    Site site{};
    if (!resolve(api, site)) {
        return 1;
    }
    if (api->hook_exact == nullptr) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: host API is missing hook_exact");
        return 1;
    }
    braking_multiplier = 100.0f / static_cast<float>(percent);
    // The loader owns installation, rollback, and removal of this exact 10-byte
    // entry hook. The original pointer must remain writable until plan commit.
    if (api->hook_exact(site.address, reinterpret_cast<void *>(&vehicle_arrival_update),
                        sizeof(ENTRY_BEFORE), &original_update) != 0) {
        log(api, DEFIANCE_LOG_ERROR, "vehicle braking: hook refused; host will finish owned rollback");
        return 1;
    }
    char message[96];
    std::snprintf(message, sizeof(message), "vehicle braking: installed (%d%% braking window)", percent);
    log(api, DEFIANCE_LOG_INFO, message);
    return 0;
}

// The descriptor name is the manifest ID. No stop callback is needed: managed
// hooks belong to the loader and it releases them with the plugin's plan.
const DefiancePlugin PLUGIN = {DEFIANCE_ABI_VERSION, PLUGIN_ID, "0.1.0", init, nullptr};

// All contract storage outlives the loader's use of these pointers. The loader
// reads it after init, before publishing the staged writes into game memory.
DefiancePatchContractEntryV1 entry = {
    "logic.dll", 0, DEFIANCE_PATCH_KIND_ENTRY, ENTRY_BEFORE, sizeof(ENTRY_BEFORE), nullptr, 0,
};
const DefiancePatchContractV1 PATCH_CONTRACT = {1, sizeof(DefiancePatchContractV1), &entry, 1};
const DefiancePatchContractV1 EMPTY_CONTRACT = {1, sizeof(DefiancePatchContractV1), nullptr, 0};

} // namespace

extern "C" __declspec(dllexport) const DefiancePlugin *defiance_plugin() {
    return &PLUGIN;
}

extern "C" __declspec(dllexport) const DefiancePatchContractV1 *
defiance_patch_contract_v1(const DefianceApi *api) {
    if (!valid_api(api)) {
        return nullptr;
    }
    int percent = 0;
    if (!read_percent(api, percent)) {
        return nullptr;
    }
    if (percent == 100) {
        return &EMPTY_CONTRACT;
    }
    Site site{};
    if (!resolve(api, site)) {
        return nullptr;
    }
    entry.rva = site.rva;
    return &PATCH_CONTRACT;
}
