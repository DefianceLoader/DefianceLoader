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
CHECK(sizeof(DefianceMovePreviewHandlerV1) == 24, "move preview handler layout");
CHECK(offsetof(DefianceMovePreviewHandlerV1, confirm) == 16, "move preview confirm offset");
CHECK(sizeof(DefianceMovePreviewV1) == 8, "move preview table layout");
CHECK(sizeof(DefianceCrashRangesV1) == 16, "crash ranges table layout");
CHECK(offsetof(DefianceCrashRangesV1, unmap) == 8, "crash ranges unmap offset");
CHECK(sizeof(DefianceNearMemoryV1) == 8, "near memory table layout");
CHECK(sizeof(DefianceTraceV1) == 16, "trace table layout");
CHECK(offsetof(DefianceTraceV1, stop) == 8, "trace stop offset");
CHECK(sizeof(DefianceTraceFieldV1) == 32, "trace field layout");
CHECK(sizeof(DefianceTraceRequestV1) == 568, "trace request layout");
CHECK(offsetof(DefianceTraceRequestV1, fields) == 56, "trace fields offset");
CHECK(sizeof(DefianceTraceEventV1) == 432, "trace event layout");
CHECK(offsetof(DefianceTraceEventV1, frames) == 304, "trace frames offset");
CHECK(sizeof(DefianceTraceStatsV1) == 32, "trace statistics layout");
CHECK(sizeof(DefianceTraceCaptureV1) == 40, "trace capture table layout");
CHECK(sizeof(DefianceMultiplayerV1) == 16, "multiplayer table layout");
CHECK(sizeof(DefianceSessionV1) == 24, "session table layout");
CHECK(offsetof(DefianceSessionV1, before_mission) == 16, "session before_mission offset");
CHECK(sizeof(DefianceMissionFrameV1) == 16, "mission frame layout");
CHECK(offsetof(DefianceMissionFrameV1, dt) == 8, "mission frame dt offset");
CHECK(sizeof(DefianceMissionEventsV1) == 16, "mission events table layout");
CHECK(offsetof(DefianceMissionEventsV1, frames) == 8, "mission events frames offset");
CHECK(sizeof(DefianceMissionFeedV1) == 16, "mission feed table layout");
CHECK(offsetof(DefianceMissionFeedV1, frame) == 8, "mission feed frame offset");
CHECK(offsetof(DefianceMultiplayerV1, guard_installed) == 8, "multiplayer guard offset");
CHECK(sizeof(DefianceMemberStateV1) == 24, "member snapshot layout");
CHECK(sizeof(DefianceGameAccessV1) == 56, "game access table layout");
CHECK(sizeof(DefianceRelationV1) == 16, "relation table layout");
CHECK(offsetof(DefianceRelationV1, relation) == 8, "relation query offset");
CHECK(sizeof(DefianceFacetsV1) == 8, "facets table layout");
CHECK(sizeof(DefianceSelectionSnapshotV1) == 16, "selection snapshot table layout");
CHECK(offsetof(DefianceSelectionSnapshotV1, copy_command_targets) == 8,
      "selection snapshot command targets offset");
CHECK(sizeof(DefianceNativeReplacementV1) == 16, "native replacement layout");
CHECK(sizeof(DefiancePatchUnitV1) == 32, "patch unit layout");
CHECK(offsetof(DefiancePatchUnitV1, code) == 16, "patch unit code offset");
CHECK(sizeof(DefiancePatchV1) == 32, "patch table layout");
CHECK(offsetof(DefiancePatchV1, install) == 16, "patch install offset");
CHECK(offsetof(DefiancePatchV1, contract) == 24, "patch contract offset");
CHECK(sizeof(DefianceBuildV1) == 8, "build table layout");
CHECK(offsetof(DefianceMemberStateV1, selected) == 16, "member flags offset");
