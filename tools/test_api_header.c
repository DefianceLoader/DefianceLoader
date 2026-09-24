#include "../crates/api/include/defiance.h"

#ifdef __cplusplus
#define CHECK static_assert
#else
#define CHECK _Static_assert
#endif

CHECK(DEFIANCE_ABI_VERSION == 5, "base ABI unchanged");
CHECK(sizeof(DefianceApi) == 112, "base API layout");
CHECK(sizeof(DefiancePlugin) == 40, "plugin layout");
CHECK(sizeof(DefianceServiceApiV1) == 24, "service extension layout");
CHECK(offsetof(DefianceServiceApiV1, register_service) == 8, "register offset");
CHECK(offsetof(DefianceServiceApiV1, query_service) == 16, "query offset");
CHECK(sizeof(DefianceSelectionV1) == 8, "selection table layout");
CHECK(sizeof(DefianceCrashRangesV1) == 16, "crash ranges table layout");
CHECK(offsetof(DefianceCrashRangesV1, unmap) == 8, "crash ranges unmap offset");
CHECK(sizeof(DefianceTraceV1) == 16, "trace table layout");
CHECK(offsetof(DefianceTraceV1, stop) == 8, "trace stop offset");
CHECK(sizeof(DefianceMultiplayerV1) == 16, "multiplayer table layout");
CHECK(offsetof(DefianceMultiplayerV1, guard_installed) == 8, "multiplayer guard offset");
CHECK(sizeof(DefianceMemberStateV1) == 24, "member snapshot layout");
CHECK(sizeof(DefianceGameAccessV1) == 56, "game access table layout");
CHECK(offsetof(DefianceMemberStateV1, selected) == 16, "member flags offset");
