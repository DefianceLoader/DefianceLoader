//! Preview individual move positions and show the positions after an order.
//!
//! Supported on the GOG and Steam September 2026 builds; [`sites`] finds the
//! code by signature and RTTI and refuses any build where a site does not
//! resolve uniquely. The move cursor owns one
//! arrow per squad. Its renderer positions supply the order's current base
//! point, and the logic DLL's formation solver supplies one offset per member.
//! Cover selection occurs later in `AiCoverState`; the preview uses the
//! formation positions until a selected cover destination can be read.

mod sites;

use core::ffi::c_void;
use defiance_api::{
    Api, GameAccessV1, MissionFrameV1, MovePreviewHandlerV1, MovePreviewV1, PatchContractV1,
    Plugin, ABI_VERSION, LOG_DEBUG, LOG_ERROR, LOG_INFO, LOG_WARN, MISSION_FRAME, PATCH_KIND_CALL,
    PATCH_KIND_ENTRY,
};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

defiance_feature_sdk::service_handshake!();
defiance_feature_sdk::crash_handshake!();

// Each entry hook steals these whole instructions; the site's signature
// begins with them in every supported build.
const CURSOR_BEFORE: &[u8] = &[0x48, 0x83, 0xec, 0x58, 0x0f, 0x29, 0x74, 0x24, 0x40];
const CURSOR_DESTRUCTOR_BEFORE: &[u8] =
    &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20];
const ISSUE_ORDER_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x20];
const FACET_UPDATE_BEFORE: &[u8] = &[0x48, 0x89, 0x5c, 0x24, 0x10];
const MISSION_DELETING_DESTRUCTOR_BEFORE: &[u8] =
    &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20];
const SQUAD_ORDER_DISPATCH_BEFORE: &[u8] = &[
    0x48, 0x8b, 0xc4, 0x4c, 0x89, 0x40, 0x18, 0x48, 0x89, 0x50, 0x10,
];
/// A hooked direct call: `e8` and its rel32.
const CALL_LENGTH: usize = 5;
const MAX_MEMBERS: usize = 32;
const MAX_MARKERS: usize = 64;
/// Move-preview candidates the handler may give beyond the assigned slots.
const MAX_CANDIDATES: usize = 128;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(80);
const ARRIVAL_INTERVAL: Duration = Duration::from_millis(250);
const ARRIVAL_SETTLE: Duration = Duration::from_millis(1500);
const COVER_SETTLE: Duration = Duration::from_secs(4);
const UNTRACKED_LIFETIME: Duration = Duration::from_secs(180);
const ARRIVAL_RADIUS_SQUARED: f32 = 3.5 * 3.5;
const CLOSE_ORDER_SQUARED: f32 = 1.25 * 1.25;
const REQUIRED_MOVEMENT_SQUARED: f32 = 0.75 * 0.75;
const STILL_DISTANCE_SQUARED: f32 = 0.18 * 0.18;
const ARROW_SCALE: Vec3 = Vec3 {
    x: 0.3,
    y: 0.3,
    z: 0.3,
};
// Move-preview candidates: smaller than the destination arrows.
const CANDIDATE_SCALE: Vec3 = Vec3 {
    x: 0.15,
    y: 0.15,
    z: 0.15,
};
const HIDDEN_SCALE: Vec3 = Vec3 {
    x: 0.001,
    y: 0.001,
    z: 0.001,
};
const UNIT_SCALE: Vec3 = Vec3 {
    x: 1.0,
    y: 1.0,
    z: 1.0,
};
const MARKER_HEIGHT: f32 = 0.8;
const GROUNDED_MARKER_HEIGHT: f32 = 0.4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C)]
#[derive(Default)]
struct HeightSample {
    value: f32,
    valid: u8,
    padding: [u8; 3],
}

impl Vec3 {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    fn horizontal_distance_squared(self, other: Self) -> f32 {
        (self.x - other.x).powi(2) + (self.y - other.y).powi(2)
    }

    fn plus(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

#[repr(C)]
struct VectorHeader {
    begin: *mut c_void,
    end: *mut c_void,
    capacity: *mut c_void,
}

type CursorFn = unsafe extern "C" fn(*mut c_void, *const f32);
type IssueFn = unsafe extern "C" fn(*mut c_void);
type FacetUpdateFn = unsafe extern "C" fn(*mut c_void, f32);
type LocationFn = unsafe extern "C" fn(*mut c_void, *const Vec3) -> *mut c_void;
type SquadOrderFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
type StopFactoryFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
type SoldierOrderFn = unsafe extern "C" fn(*mut c_void, *mut c_void, f32);
type ConstructorFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
type FactoryFn = unsafe extern "C" fn(*mut c_void);
type RenderUpdateFn = unsafe extern "C" fn(*mut c_void);
type DestructorFn = unsafe extern "C" fn(*mut c_void, u32) -> *mut c_void;
type SolverFn = unsafe extern "C" fn(usize, usize, *mut VectorHeader, *const [f32; 4], u32);
type HeightFn =
    unsafe extern "C" fn(*mut c_void, *mut HeightSample, *const [f32; 2]) -> *mut HeightSample;

static CURSOR_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static CURSOR_DESTRUCTOR_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static ISSUE_ORDER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static FACET_UPDATE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static MOVE_COMMAND_VTABLE: AtomicUsize = AtomicUsize::new(0);
static MISSION_DELETING_DESTRUCTOR_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static MOVE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static SQUAD_ORDER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static STOP_FACTORY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static GARRISON_HANDOFF_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static BOARDING_HANDOFF_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static CONSTRUCTOR: AtomicUsize = AtomicUsize::new(0);
static FACTORY: AtomicUsize = AtomicUsize::new(0);
static RENDER_UPDATE: AtomicUsize = AtomicUsize::new(0);
static NEXT_ORDER_ID: AtomicUsize = AtomicUsize::new(1);
static SOLVER: AtomicUsize = AtomicUsize::new(0);
static LOG: AtomicUsize = AtomicUsize::new(0);
// Resolved addresses (not rvas), set once by `install`.
static TERRAIN_RENDERER_VTABLE: AtomicUsize = AtomicUsize::new(0);
static MAP_IMPL_VTABLE: AtomicUsize = AtomicUsize::new(0);
static MAP_SAMPLE_HEIGHT: AtomicUsize = AtomicUsize::new(0);
static RENDERER_SCALE_SETTER: AtomicUsize = AtomicUsize::new(0);
static STOP_ORDER_VTABLE: AtomicUsize = AtomicUsize::new(0);
static TERRAIN_SCENE: AtomicUsize = AtomicUsize::new(0);
/// The last dragged move cursor's state, which hover markers are drawn with.
static DRAG_SNAPSHOT: Mutex<Option<CursorSnapshot>> = Mutex::new(None);
static GAME_ACCESS: OnceLock<&'static GameAccessV1> = OnceLock::new();
static CACHE: OnceLock<Mutex<Markers>> = OnceLock::new();
static VISUAL: OnceLock<Mutex<Visual>> = OnceLock::new();
static ORDER_VISUALS: OnceLock<Mutex<Vec<OrderVisual>>> = OnceLock::new();
static VEHICLE_CACHE: OnceLock<Mutex<Markers>> = OnceLock::new();
static VEHICLE_VISUAL: OnceLock<Mutex<Visual>> = OnceLock::new();
static CANDIDATE_VISUAL: OnceLock<Mutex<Visual>> = OnceLock::new();
static MOVE_HANDLER: OnceLock<MoveHandler> = OnceLock::new();
/// Set while the handler's markers follow the cursor without a drag.
static HOVERING: AtomicBool = AtomicBool::new(false);
// diagnostic: how many hover draws and hover removals were logged.
#[cfg(feature = "diagnostics")]
static HOVER_TRACE: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "diagnostics")]
static DRAG_TRACE: AtomicUsize = AtomicUsize::new(0);
// diagnostic: set at the first cursor update without a drag.
#[cfg(feature = "diagnostics")]
static HOVER_SEEN: AtomicBool = AtomicBool::new(false);
// diagnostic: the facet's commands as last logged, and how many changes were logged.
#[cfg(feature = "diagnostics")]
static FACET_SEEN: Mutex<Option<(usize, usize, i32)>> = Mutex::new(None);
#[cfg(feature = "diagnostics")]
static FACET_LOGGED: AtomicUsize = AtomicUsize::new(0);
// diagnostic: which hover outcomes were logged (bit 0 no targets, 1 no preview,
// 2 drawn, 3 scene, 4 built).
#[cfg(feature = "diagnostics")]
static HOVER_OUTCOMES: AtomicUsize = AtomicUsize::new(0);

/// diagnostic: logs a [`hover`] outcome the first time it occurs.
#[cfg(feature = "diagnostics")]
fn hover_outcome(bit: usize, message: impl FnOnce() -> String) {
    if HOVER_OUTCOMES.fetch_or(1 << bit, Ordering::AcqRel) & (1 << bit) == 0 {
        log(LOG_DEBUG, &message());
    }
}
static MOVE_PREVIEW: MovePreviewV1 = MovePreviewV1 {
    set_handler: set_move_handler,
};

/// The plugin's [`MovePreviewHandlerV1`]. Its owner promises the context is
/// usable from the game thread for the process lifetime.
struct MoveHandler(MovePreviewHandlerV1);

unsafe impl Send for MoveHandler {}
unsafe impl Sync for MoveHandler {}
static BOARDING_PENDING: OnceLock<Mutex<Vec<PositionTrack>>> = OnceLock::new();
static VEHICLE_STOP_PENDING: OnceLock<Mutex<Vec<PositionTrack>>> = OnceLock::new();

#[derive(Default)]
struct Markers {
    preview_points: Vec<Vec3>,
    preview_tracks: Vec<PositionTrack>,
    preview_at: Option<Instant>,
    preview_attempt_at: Option<Instant>,
    ordered_preview_points: Vec<Vec3>,
    order_id: usize,
    order_scene: usize,
    order_snapshot: CursorSnapshot,
    moved: Vec<Vec3>,
    moved_tracks: Vec<PositionTrack>,
    owners: Vec<Option<PositionTrack>>,
    starting_positions: Vec<Option<Vec3>>,
    moved_at: Option<Instant>,
    // The cursor begins a new order before issuing its individual move points.
    next_move_point_starts_order: bool,
    arrival_check_at: Option<Instant>,
    previous_positions: Vec<Option<Vec3>>,
    assigned_tracks: Vec<Option<usize>>,
    settled_at: Vec<Option<Instant>>,
    cleared_by: Vec<Option<usize>>,
    retained: Vec<Box<Markers>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PositionTrack {
    facet: usize,
    vtable: usize,
    position: usize,
}

struct Preview {
    points: Vec<Vec3>,
    tracks: Vec<PositionTrack>,
}

struct OrderDisplay {
    id: usize,
    scene: usize,
    snapshot: CursorSnapshot,
    points: Vec<Vec3>,
}

struct OrderVisual {
    id: usize,
    visual: Visual,
}

#[derive(Clone, Copy)]
struct SquadArrow {
    squad: *mut c_void,
    renderer: *mut c_void,
    infantry: bool,
}

// A separate stock cursor owns the extra arrow renderers. Its Vec3 vector is
// borrowed only during a native refresh; the game never reads a Rust allocation
// when it later issues the real squad order.
#[repr(align(16))]
struct CursorImage([u8; 0x100]);

#[derive(Clone, Copy)]
struct CursorSnapshot {
    movement: [u8; 0x24],
    rotating: u8,
}

impl Default for CursorSnapshot {
    fn default() -> Self {
        Self {
            movement: [0; 0x24],
            rotating: 0,
        }
    }
}

impl CursorSnapshot {
    /// An arrow command's state pointing along +x: the base constructor
    /// (steam 2026-09-25 game.dll 0x32d250) zeroes +0x18..+0x3c except the
    /// float 1.0 at +0x30 (the direction's x). The state is rotating (+0xa8)
    /// because the render update (steam 2026-09-25 game.dll 0x32d5b0) gives
    /// non-rotating list markers the constant (1, 0, 0, 0) as their local
    /// rotation (world2.dll NodeImpl vfunc 22), and markers drawn that way
    /// do not show; rotating ones take the world rotation built from
    /// +0x30/+0x34, as a dragged arrow does.
    fn arrow() -> Self {
        let mut snapshot = Self::default();
        snapshot.movement[0x18..0x1c].copy_from_slice(&1.0f32.to_le_bytes());
        snapshot.rotating = 1;
        snapshot
    }
}

#[derive(Default)]
struct Visual {
    cursor: Option<Box<CursorImage>>,
    source: usize,
    scene: usize,
    count: usize,
    snapshot: CursorSnapshot,
    points: Vec<Vec3>,
    source_points: Vec<Vec3>,
    scale: Vec3,
    height: f32,
    terrain: bool,
    /// The most points [`Visual::update`] draws; more dispose the markers.
    capacity: usize,
}

impl Visual {
    /// diagnostic: the fake cursor's address, render list length and first
    /// point, for logging.
    #[cfg(feature = "diagnostics")]
    fn describe(&self) -> String {
        let Some(cursor) = self.cursor.as_ref() else {
            return "no cursor".into();
        };
        let bytes = cursor.0.as_ptr();
        let (begin, end) = unsafe {
            (
                *bytes.add(0x60).cast::<usize>(),
                *bytes.add(0x68).cast::<usize>(),
            )
        };
        format!(
            "cursor {bytes:p} scene {:#x} renderers {} points {} first {:?}",
            self.scene,
            end.wrapping_sub(begin) / 16,
            self.count,
            self.points.first().map(|p| (p.x, p.y, p.z))
        )
    }

    unsafe fn refresh(&mut self, source: *mut c_void) {
        if self.source != source as usize
            || self.cursor.is_none()
            || self.points.is_empty()
            || self.scene != unsafe { ptr(source, 0x10) } as usize
        {
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.cast::<u8>().add(0x18),
                self.snapshot.movement.as_mut_ptr(),
                self.snapshot.movement.len(),
            );
            self.snapshot.rotating = *source.cast::<u8>().add(0xa8);
            self.draw(false);
        }
    }

    unsafe fn dispose(&mut self) {
        if let Some(mut cursor) = self.cursor.take() {
            let object = cursor.0.as_mut_ptr().cast::<c_void>();
            unsafe { core::ptr::write_bytes(cursor.0.as_mut_ptr().add(0x78), 0, 24) };
            let destructor = CURSOR_DESTRUCTOR_ORIGINAL.load(Ordering::Acquire);
            if destructor != 0 {
                let destructor: DestructorFn = unsafe { core::mem::transmute(destructor) };
                unsafe { destructor(object, 0) };
            }
        }
        self.source = 0;
        self.scene = 0;
        self.count = 0;
        self.points.clear();
        self.source_points.clear();
    }

    unsafe fn update(&mut self, source: Option<*mut c_void>, points: &[Vec3]) {
        if points.is_empty() || points.len() > self.capacity {
            unsafe { self.dispose() };
            return;
        }
        if source.is_none() && self.cursor.is_some() && self.source_points == points {
            return;
        }
        let mut scene_changed = false;
        if let Some(source) = source {
            let scene = unsafe { ptr(source, 0x10) } as usize;
            if scene == 0 {
                unsafe { self.dispose() };
                return;
            }
            scene_changed = self.scene != 0 && self.scene != scene;
            self.source = source as usize;
            self.scene = scene;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.cast::<u8>().add(0x18),
                    self.snapshot.movement.as_mut_ptr(),
                    self.snapshot.movement.len(),
                );
                self.snapshot.rotating = *source.cast::<u8>().add(0xa8);
            }
        }
        if self.scene == 0 {
            return;
        }
        let create = self.cursor.is_none() || self.count != points.len() || scene_changed;
        if create {
            let scene = self.scene;
            let source = self.source;
            unsafe { self.dispose() };
            self.scene = scene;
            self.source = source;
            let constructor = CONSTRUCTOR.load(Ordering::Acquire);
            if constructor == 0 {
                return;
            }
            let constructor: ConstructorFn = unsafe { core::mem::transmute(constructor) };
            let mut cursor = Box::new(CursorImage([0; 0x100]));
            let object = cursor.0.as_mut_ptr().cast::<c_void>();
            unsafe { constructor(object, self.scene as *mut c_void) };
            self.cursor = Some(cursor);
            self.count = points.len();
        }
        self.source_points.clear();
        self.source_points.extend_from_slice(points);
        let map = self.terrain.then(terrain_map).flatten();
        self.points.clear();
        self.points.extend(points.iter().map(|point| {
            let sampled = map.and_then(|map| unsafe { ground_height(map, *point) });
            Vec3 {
                x: point.x,
                y: point.y,
                z: sampled.map_or(point.z + self.height, |z| z + GROUNDED_MARKER_HEIGHT),
            }
        }));
        unsafe { self.draw(create) };
    }

    unsafe fn draw(&mut self, create: bool) {
        let Some(cursor) = self.cursor.as_mut() else {
            return;
        };
        let bytes = cursor.0.as_mut_ptr();
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.snapshot.movement.as_ptr(),
                bytes.add(0x18),
                self.snapshot.movement.len(),
            );
            // Stock arrow points are local to the cursor's origin. Our points
            // are already in world space, so the fake cursor starts at zero.
            core::ptr::write_bytes(bytes.add(0x1c), 0, 12);
            // The preview points already incorporate either stock placement
            // mode. Always render them through the local-point path.
            *bytes.add(0x18) = 0;
            *bytes.add(0xa8) = self.snapshot.rotating;
            *bytes.add(0xb0) = 0;
            let begin = self.points.as_mut_ptr();
            *bytes.add(0x78).cast::<*mut Vec3>() = begin;
            *bytes.add(0x80).cast::<*mut Vec3>() = begin.add(self.points.len());
            *bytes.add(0x88).cast::<*mut Vec3>() = begin.add(self.points.capacity());
            if create {
                let factory = FACTORY.load(Ordering::Acquire);
                if factory != 0 {
                    let factory: FactoryFn = core::mem::transmute(factory);
                    factory(bytes.cast());
                }
            }
            let update = RENDER_UPDATE.load(Ordering::Acquire);
            if update != 0 {
                let update: RenderUpdateFn = core::mem::transmute(update);
                update(bytes.cast());
            }
            let render_begin = *bytes.add(0x60).cast::<usize>();
            let render_end = *bytes.add(0x68).cast::<usize>();
            if create && render_end.checked_sub(render_begin) == Some(self.count * 16) {
                for index in 0..self.count {
                    let renderer = *((render_begin + index * 16) as *const *mut c_void);
                    if !renderer.is_null() {
                        // The native renderer stores its transform scale at
                        // +0x3ec; virtual slot +0xc8 updates the same value.
                        let set_scale: unsafe extern "C" fn(*mut c_void, *const Vec3) =
                            core::mem::transmute(method(renderer, 0xc8));
                        set_scale(renderer, &self.scale);
                    }
                }
            }
            core::ptr::write_bytes(bytes.add(0x78), 0, 24);
        }
    }
}

fn infantry_visual() -> Visual {
    Visual {
        scale: ARROW_SCALE,
        height: MARKER_HEIGHT,
        terrain: true,
        capacity: MAX_MARKERS,
        ..Visual::default()
    }
}

fn visual() -> &'static Mutex<Visual> {
    VISUAL.get_or_init(|| Mutex::new(infantry_visual()))
}

fn cache() -> &'static Mutex<Markers> {
    CACHE.get_or_init(|| Mutex::new(Markers::default()))
}

fn order_visuals() -> &'static Mutex<Vec<OrderVisual>> {
    ORDER_VISUALS.get_or_init(|| Mutex::new(Vec::new()))
}

fn candidate_visual() -> &'static Mutex<Visual> {
    CANDIDATE_VISUAL.get_or_init(|| {
        Mutex::new(Visual {
            scale: CANDIDATE_SCALE,
            capacity: MAX_CANDIDATES,
            ..infantry_visual()
        })
    })
}

fn vehicle_visual() -> &'static Mutex<Visual> {
    VEHICLE_VISUAL.get_or_init(|| {
        Mutex::new(Visual {
            scale: UNIT_SCALE,
            capacity: MAX_MARKERS,
            ..Visual::default()
        })
    })
}

fn vehicle_cache() -> &'static Mutex<Markers> {
    VEHICLE_CACHE.get_or_init(|| Mutex::new(Markers::default()))
}

fn boarding_pending() -> &'static Mutex<Vec<PositionTrack>> {
    BOARDING_PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

fn vehicle_stop_pending() -> &'static Mutex<Vec<PositionTrack>> {
    VEHICLE_STOP_PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

impl PositionTrack {
    fn read(self) -> Option<Vec3> {
        let vtable: usize = read_process(self.facet)?;
        if vtable != self.vtable {
            return None;
        }
        read_process::<Vec3>(self.position).filter(|point| point.finite())
    }
}

fn read_process<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    let mut value = MaybeUninit::<T>::uninit();
    let mut read = 0;
    let length = core::mem::size_of::<T>();
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            value.as_mut_ptr().cast(),
            length,
            &mut read,
        )
    };
    (ok != 0 && read == length).then(|| unsafe { value.assume_init() })
}

fn terrain_map() -> Option<usize> {
    let renderer_vtable = TERRAIN_RENDERER_VTABLE.load(Ordering::Acquire);
    let scene = TERRAIN_SCENE.load(Ordering::Acquire);
    if renderer_vtable == 0 || scene == 0 {
        return None;
    }
    let renderer = read_process::<usize>(scene + 0x228)?;
    if renderer == 0 || read_process::<usize>(renderer)? != renderer_vtable {
        return None;
    }
    let map = read_process::<usize>(renderer + 0xb8)?;
    (map != 0 && read_process::<usize>(map)? == MAP_IMPL_VTABLE.load(Ordering::Acquire))
        .then_some(map)
}

unsafe fn ground_height(map: usize, point: Vec3) -> Option<f32> {
    let sample = MAP_SAMPLE_HEIGHT.load(Ordering::Acquire);
    if sample == 0 {
        return None;
    }
    let sample: HeightFn = unsafe { core::mem::transmute(sample) };
    let mut result = HeightSample::default();
    let position = [point.x, point.y];
    unsafe { sample(map as *mut c_void, &mut result, &position) };
    (result.valid != 0 && result.value.is_finite()).then_some(result.value)
}

// Match whole formations at once so a locally closest pair cannot force a
// worse assignment for the remaining soldiers.
fn minimum_distance_assignment(rows: &[Vec3], columns: &[Vec3]) -> Vec<usize> {
    if rows.is_empty() || rows.len() > columns.len() {
        return Vec::new();
    }
    let mut row_cost = vec![0.0_f64; rows.len() + 1];
    let mut column_cost = vec![0.0_f64; columns.len() + 1];
    let mut column_row = vec![0_usize; columns.len() + 1];
    let mut path = vec![0_usize; columns.len() + 1];
    for row in 1..=rows.len() {
        column_row[0] = row;
        let mut current_column = 0;
        let mut best = vec![f64::INFINITY; columns.len() + 1];
        let mut used = vec![false; columns.len() + 1];
        loop {
            used[current_column] = true;
            let current_row = column_row[current_column];
            let mut step = f64::INFINITY;
            let mut next_column = 0;
            for column in 1..=columns.len() {
                if used[column] {
                    continue;
                }
                let dx = f64::from(rows[current_row - 1].x) - f64::from(columns[column - 1].x);
                let dy = f64::from(rows[current_row - 1].y) - f64::from(columns[column - 1].y);
                let distance = dx * dx + dy * dy;
                let candidate = distance - row_cost[current_row] - column_cost[column];
                if candidate < best[column] {
                    best[column] = candidate;
                    path[column] = current_column;
                }
                if best[column] < step {
                    step = best[column];
                    next_column = column;
                }
            }
            for column in 0..=columns.len() {
                if used[column] {
                    row_cost[column_row[column]] += step;
                    column_cost[column] -= step;
                } else {
                    best[column] -= step;
                }
            }
            current_column = next_column;
            if column_row[current_column] == 0 {
                break;
            }
        }
        loop {
            let previous = path[current_column];
            column_row[current_column] = column_row[previous];
            current_column = previous;
            if current_column == 0 {
                break;
            }
        }
    }
    let mut assignment = vec![0; rows.len()];
    for (column, &row) in column_row.iter().enumerate().skip(1) {
        if row != 0 {
            assignment[row - 1] = column - 1;
        }
    }
    assignment
}

impl Markers {
    fn start_vehicle_order(&mut self, preview: Preview, now: Instant) {
        self.clear_order();
        let Preview { points, tracks } = preview;
        if tracks.len() == points.len() {
            self.owners = tracks.iter().copied().map(Some).collect();
        }
        self.starting_positions = tracks.iter().map(|track| track.read()).collect();
        self.moved_tracks = tracks;
        self.moved = points;
        self.moved_at = Some(now);
    }

    fn clear_order(&mut self) {
        self.ordered_preview_points.clear();
        self.order_id = 0;
        self.order_scene = 0;
        self.order_snapshot = CursorSnapshot::default();
        self.moved.clear();
        self.moved_tracks.clear();
        self.owners.clear();
        self.starting_positions.clear();
        self.moved_at = None;
        self.arrival_check_at = None;
        self.previous_positions.clear();
        self.assigned_tracks.clear();
        self.settled_at.clear();
        self.cleared_by.clear();
    }

    fn match_owners(&self) -> Vec<Option<PositionTrack>> {
        let mut owners = vec![None; self.moved.len()];
        if self.moved_tracks.is_empty()
            || self.ordered_preview_points.len() != self.moved_tracks.len()
        {
            return owners;
        }
        if self.moved.len() <= self.moved_tracks.len() {
            for (marker, soldier) in
                minimum_distance_assignment(&self.moved, &self.ordered_preview_points)
                    .into_iter()
                    .enumerate()
            {
                owners[marker] = Some(self.moved_tracks[soldier]);
            }
        } else {
            for (soldier, marker) in
                minimum_distance_assignment(&self.ordered_preview_points, &self.moved)
                    .into_iter()
                    .enumerate()
            {
                owners[marker] = Some(self.moved_tracks[soldier]);
            }
            for (marker, &target) in self.moved.iter().enumerate() {
                if owners[marker].is_some() {
                    continue;
                }
                let closest = self
                    .ordered_preview_points
                    .iter()
                    .enumerate()
                    .min_by(|left, right| {
                        target
                            .horizontal_distance_squared(*left.1)
                            .total_cmp(&target.horizontal_distance_squared(*right.1))
                    })
                    .map(|(soldier, _)| soldier);
                owners[marker] = closest.map(|soldier| self.moved_tracks[soldier]);
            }
        }
        owners
    }

    fn retain_unordered(&mut self, selected: &[PositionTrack]) {
        let owners = if self.owners.len() == self.moved.len() {
            self.owners.clone()
        } else {
            self.match_owners()
        };
        let mut points = Vec::new();
        let mut remaining_owners = Vec::new();
        let mut seen = Vec::new();
        for (index, &point) in self.moved.iter().enumerate() {
            if self.cleared_by.get(index).copied().flatten().is_some() {
                continue;
            }
            let owner = owners[index];
            if owner.is_none_or(|owner| !selected.contains(&owner)) {
                if owner.is_some_and(|owner| seen.contains(&owner)) {
                    continue;
                }
                if let Some(owner) = owner {
                    seen.push(owner);
                }
                points.push(point);
                remaining_owners.push(owner);
            }
        }
        self.moved = points;
        self.owners = remaining_owners;
        let tracks = std::mem::take(&mut self.moved_tracks);
        let starts = std::mem::take(&mut self.starting_positions);
        for (index, track) in tracks.into_iter().enumerate() {
            if !selected.contains(&track) {
                self.moved_tracks.push(track);
                self.starting_positions
                    .push(starts.get(index).copied().flatten());
            }
        }
        if self.moved_tracks.is_empty() {
            self.moved.clear();
            self.owners.clear();
        }
        self.previous_positions.clear();
        self.assigned_tracks.clear();
        self.settled_at.clear();
        self.cleared_by.clear();
        self.arrival_check_at = None;
    }

    fn retire_tracks(&mut self, selected: &[PositionTrack]) -> usize {
        if selected.is_empty() {
            return 0;
        }
        let before = self.moved.len()
            + self
                .retained
                .iter()
                .map(|order| order.moved.len())
                .sum::<usize>();
        for order in &mut self.retained {
            order.retain_unordered(selected);
        }
        self.retained.retain(|order| !order.moved.is_empty());
        self.retain_unordered(selected);
        if self.moved.is_empty() {
            self.clear_order();
        }
        before
            - self.moved.len()
            - self
                .retained
                .iter()
                .map(|order| order.moved.len())
                .sum::<usize>()
    }

    fn start_order(
        &mut self,
        selected: Vec<PositionTrack>,
        now: Instant,
        scene: usize,
        snapshot: CursorSnapshot,
    ) {
        let preview_points = std::mem::take(&mut self.preview_points);
        let preview_tracks = std::mem::take(&mut self.preview_tracks);
        let preview_at = self.preview_at.take();
        let preview_attempt_at = self.preview_attempt_at.take();
        let retained = std::mem::take(&mut self.retained);
        let mut previous = std::mem::take(self);
        self.preview_points = preview_points;
        self.preview_tracks = preview_tracks;
        self.preview_at = preview_at;
        self.preview_attempt_at = preview_attempt_at;
        self.retained = retained;
        for order in &mut self.retained {
            order.retain_unordered(&selected);
        }
        self.retained.retain(|order| !order.moved.is_empty());
        if !previous.moved.is_empty() && !selected.is_empty() {
            previous.retain_unordered(&selected);
            if !previous.moved.is_empty() {
                self.retained.push(Box::new(previous));
            }
        }
        self.ordered_preview_points = self.preview_points.clone();
        self.order_id = NEXT_ORDER_ID.fetch_add(1, Ordering::Relaxed);
        self.order_scene = scene;
        self.order_snapshot = snapshot;
        self.starting_positions = selected.iter().map(|track| track.read()).collect();
        self.moved_tracks = selected;
        self.moved_at = Some(now);
        while self
            .retained
            .iter()
            .map(|order| order.moved.len())
            .sum::<usize>()
            >= MAX_MARKERS
        {
            self.retained.remove(0);
        }
    }

    fn record_move_point(
        &mut self,
        point: Vec3,
        now: Instant,
        scene: usize,
        snapshot: CursorSnapshot,
    ) {
        if !self
            .preview_at
            .is_some_and(|previous| now.duration_since(previous) < Duration::from_secs(1))
        {
            return;
        }
        if self.next_move_point_starts_order
            || self
                .moved_at
                .is_none_or(|previous| now.duration_since(previous) > Duration::from_millis(500))
            || self.moved_tracks != self.preview_tracks
        {
            let selected = self.preview_tracks.clone();
            self.start_order(selected, now, scene, snapshot);
        }
        self.next_move_point_starts_order = false;
        if self.moved.len() < MAX_MARKERS {
            self.moved.push(point);
        }
        self.moved_at = Some(now);
    }

    fn visible_points(&self) -> Vec<Vec3> {
        let mut points = self.current_visible_points();
        for order in &self.retained {
            points.extend(order.current_visible_points());
        }
        points.truncate(MAX_MARKERS);
        points
    }

    fn visible_orders(&self) -> Vec<OrderDisplay> {
        let mut displays = Vec::new();
        let mut remaining = MAX_MARKERS;
        for order in std::iter::once(self).chain(self.retained.iter().map(Box::as_ref)) {
            if order.order_id == 0 || order.order_scene == 0 || remaining == 0 {
                continue;
            }
            let mut points = order.current_visible_points();
            points.truncate(remaining);
            remaining -= points.len();
            if !points.is_empty() {
                displays.push(OrderDisplay {
                    id: order.order_id,
                    scene: order.order_scene,
                    snapshot: order.order_snapshot,
                    points,
                });
            }
        }
        displays
    }

    fn current_visible_points(&self) -> Vec<Vec3> {
        self.moved
            .iter()
            .enumerate()
            .filter_map(|(index, &point)| {
                self.cleared_by
                    .get(index)
                    .copied()
                    .flatten()
                    .is_none()
                    .then_some(point)
            })
            .collect()
    }

    fn arrived(&mut self, now: Instant) -> bool {
        let Some(ordered_at) = self.moved_at else {
            return false;
        };
        if self.moved_tracks.is_empty() {
            return now.duration_since(ordered_at) >= UNTRACKED_LIFETIME;
        }
        if now.duration_since(ordered_at) < Duration::from_secs(1)
            || self
                .arrival_check_at
                .is_some_and(|previous| now.duration_since(previous) < ARRIVAL_INTERVAL)
        {
            return false;
        }
        self.arrival_check_at = Some(now);
        let positions: Vec<_> = self.moved_tracks.iter().map(|track| track.read()).collect();
        if positions.iter().all(Option::is_none) {
            return now.duration_since(ordered_at) >= UNTRACKED_LIFETIME;
        }
        self.assigned_tracks.resize(self.moved.len(), None);
        self.settled_at.resize(self.moved.len(), None);
        self.cleared_by.resize(self.moved.len(), None);
        // Destinations without a preview owner use nearest unique arrivals;
        // one soldier can retire at most one arrow in this order.
        let mut candidates = Vec::new();
        for (marker, &target) in self.moved.iter().enumerate() {
            if self.cleared_by[marker].is_some() {
                continue;
            }
            for (soldier, current) in positions.iter().enumerate() {
                if self
                    .owners
                    .get(marker)
                    .copied()
                    .flatten()
                    .is_some_and(|owner| owner != self.moved_tracks[soldier])
                {
                    continue;
                }
                let Some(current) = current else {
                    continue;
                };
                let distance = current.horizontal_distance_squared(target);
                if distance > ARRIVAL_RADIUS_SQUARED {
                    continue;
                }
                if let Some(Some(start)) = self.starting_positions.get(soldier) {
                    if start.horizontal_distance_squared(target) > CLOSE_ORDER_SQUARED
                        && current.horizontal_distance_squared(*start) < REQUIRED_MOVEMENT_SQUARED
                    {
                        continue;
                    }
                }
                candidates.push((distance, marker, soldier));
            }
        }
        candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut used_soldiers = vec![false; positions.len()];
        for soldier in self.cleared_by.iter().flatten() {
            used_soldiers[*soldier] = true;
        }
        let mut matched = vec![false; self.moved.len()];
        for (_, marker, soldier) in candidates {
            if matched[marker] || used_soldiers[soldier] {
                continue;
            }
            matched[marker] = true;
            used_soldiers[soldier] = true;
            let still = matches!(
                (positions[soldier], self.previous_positions.get(soldier).copied().flatten()),
                (Some(current), Some(previous))
                    if current.horizontal_distance_squared(previous) <= STILL_DISTANCE_SQUARED
            );
            if self.assigned_tracks[marker] != Some(soldier) || !still {
                self.assigned_tracks[marker] = Some(soldier);
                self.settled_at[marker] = None;
            } else if let Some(since) = self.settled_at[marker] {
                if now.duration_since(since) >= ARRIVAL_SETTLE {
                    self.cleared_by[marker] = Some(soldier);
                }
            } else {
                self.settled_at[marker] = Some(now);
            }
        }
        // Cover can move the final position away from the issued formation
        // point. A known owner who has moved and then held still has settled.
        for marker in 0..self.moved.len() {
            if self.cleared_by[marker].is_some() || matched[marker] {
                continue;
            }
            let Some(owner) = self.owners.get(marker).copied().flatten() else {
                continue;
            };
            let Some(soldier) = self.moved_tracks.iter().position(|track| *track == owner) else {
                continue;
            };
            if used_soldiers[soldier] {
                continue;
            }
            let (Some(current), Some(previous), Some(Some(start))) = (
                positions[soldier],
                self.previous_positions.get(soldier).copied().flatten(),
                self.starting_positions.get(soldier),
            ) else {
                continue;
            };
            if current.horizontal_distance_squared(*start) < REQUIRED_MOVEMENT_SQUARED {
                continue;
            }
            let still = current.horizontal_distance_squared(previous) <= STILL_DISTANCE_SQUARED;
            if self.assigned_tracks[marker] != Some(soldier) || !still {
                self.assigned_tracks[marker] = Some(soldier);
                self.settled_at[marker] = None;
            } else if let Some(since) = self.settled_at[marker] {
                if now.duration_since(since) >= COVER_SETTLE {
                    self.cleared_by[marker] = Some(soldier);
                }
            } else {
                self.settled_at[marker] = Some(now);
            }
            matched[marker] = true;
            used_soldiers[soldier] = true;
        }
        for marker in 0..self.moved.len() {
            if self.cleared_by[marker].is_none() && !matched[marker] {
                self.assigned_tracks[marker] = None;
                self.settled_at[marker] = None;
            }
        }
        self.previous_positions = positions;
        self.cleared_by.iter().all(Option::is_some)
    }
}

unsafe fn ptr(object: *mut c_void, offset: usize) -> *mut c_void {
    unsafe { *object.cast::<u8>().add(offset).cast::<*mut c_void>() }
}

unsafe fn method(object: *mut c_void, slot: usize) -> usize {
    unsafe { *ptr(object, 0).cast::<usize>().add(slot / 8) }
}

unsafe fn get(object: *mut c_void, slot: usize) -> *mut c_void {
    if object.is_null() {
        return core::ptr::null_mut();
    }
    let method: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
        unsafe { core::mem::transmute(method(object, slot)) };
    unsafe { method(object) }
}

unsafe fn cursor_arrows(cursor: *mut c_void) -> Option<Vec<SquadArrow>> {
    let render_begin = unsafe { ptr(cursor, 0x60) } as usize;
    let render_end = unsafe { ptr(cursor, 0x68) } as usize;
    let selection_begin = unsafe { ptr(cursor, 0xd8) } as usize;
    let selection_end = unsafe { ptr(cursor, 0xe0) } as usize;
    let render_bytes = render_end.checked_sub(render_begin)?;
    let selection_bytes = selection_end.checked_sub(selection_begin)?;
    if render_begin == 0
        || selection_begin == 0
        || render_bytes == 0
        || render_bytes % 16 != 0
        || selection_bytes != render_bytes / 2
        || render_bytes / 16 > MAX_MARKERS
    {
        return None;
    }
    let mut arrows = Vec::with_capacity(render_bytes / 16);
    for index in 0..render_bytes / 16 {
        let entry = unsafe { *((selection_begin + index * 8) as *const *mut c_void) };
        let renderer = unsafe { *((render_begin + index * 16) as *const *mut c_void) };
        if entry.is_null() || renderer.is_null() {
            return None;
        }
        let squad = unsafe { ptr(entry, 0x10) };
        if squad.is_null() {
            return None;
        }
        let kind = unsafe { method(squad, 0x98) };
        if kind == 0 {
            return None;
        }
        let kind: unsafe extern "C" fn(*mut c_void, u32) -> u8 =
            unsafe { core::mem::transmute(kind) };
        arrows.push(SquadArrow {
            squad,
            renderer,
            infantry: unsafe { kind(squad, 0x10) } != 0,
        });
    }
    Some(arrows)
}

unsafe fn hide_stock_infantry_arrows(arrows: &[SquadArrow]) {
    let setter = RENDERER_SCALE_SETTER.load(Ordering::Acquire);
    for arrow in arrows {
        let renderer = arrow.renderer;
        if unsafe { method(renderer, 0xc8) } != setter {
            continue;
        }
        let scale = unsafe { *renderer.cast::<u8>().add(0x3ec).cast::<Vec3>() };
        if !scale.finite() {
            continue;
        }
        let wanted = if arrow.infantry {
            if scale == HIDDEN_SCALE {
                continue;
            }
            HIDDEN_SCALE
        } else {
            if scale != HIDDEN_SCALE {
                continue;
            }
            UNIT_SCALE
        };
        let set_scale: unsafe extern "C" fn(*mut c_void, *const Vec3) =
            unsafe { core::mem::transmute(setter) };
        unsafe { set_scale(renderer, &wanted) };
    }
}

fn log(level: u32, message: &str) {
    let callback = LOG.load(Ordering::Acquire);
    if callback != 0 {
        let callback: unsafe extern "C" fn(u32, *const i8) =
            unsafe { core::mem::transmute(callback) };
        if let Ok(message) = std::ffi::CString::new(message) {
            unsafe { callback(level, message.as_ptr()) };
        }
    }
}

unsafe fn selected_members(squad: *mut c_void, game: &GameAccessV1) -> Option<Vec<*mut c_void>> {
    let count = unsafe { (game.copy_members)(squad, core::ptr::null_mut(), 0) };
    if count == 0 || count > MAX_MEMBERS {
        return None;
    }
    unsafe { defiance_feature_sdk::services::recipients(game, squad) }
        .map(|recipients| recipients.members)
}

unsafe fn position_track(member: *mut c_void) -> Option<PositionTrack> {
    if member.is_null() {
        return None;
    }
    let facets = unsafe { get(member, 0xb0) };
    if facets.is_null() {
        return None;
    }
    let facet = unsafe { ptr(facets, 0) };
    if facet.is_null() {
        return None;
    }
    let position = unsafe { get(facet, 0x58) } as *const Vec3;
    if position.is_null() || !unsafe { *position }.finite() {
        return None;
    }
    let vtable = unsafe { *facet.cast::<usize>() };
    (vtable != 0).then_some(PositionTrack {
        facet: facet as usize,
        vtable,
        position: position as usize,
    })
}

unsafe fn formation_mode(squad: *mut c_void) -> u32 {
    let facets = unsafe { get(squad, 0xb0) };
    if facets.is_null() {
        return 1;
    }
    let ai = unsafe { ptr(facets, 0x28) };
    if ai.is_null() {
        return 1;
    }
    let holder = unsafe { ptr(ai, 0x1d0) };
    if holder.is_null() {
        return 1;
    }
    let layout = unsafe { ptr(holder, 0x10) };
    if layout.is_null() {
        return 1;
    }
    let mode = unsafe { *layout.cast::<u8>().add(0x10).cast::<u32>() };
    if (1..=5).contains(&mode) {
        mode
    } else {
        1
    }
}

unsafe fn formation(count: usize, yaw: f32, mode: u32) -> Option<Vec<Vec3>> {
    if count == 0 || count > MAX_MEMBERS || !yaw.is_finite() {
        return None;
    }
    let solver = SOLVER.load(Ordering::Acquire);
    if solver == 0 {
        return None;
    }
    let mut storage: Vec<MaybeUninit<[f32; 4]>> = Vec::with_capacity(count * 8 + 32);
    let begin = storage.as_mut_ptr();
    let mut output = VectorHeader {
        begin: begin.cast(),
        end: begin.cast(),
        capacity: unsafe { begin.add(storage.capacity()) }.cast(),
    };
    let direction = [0.0, 0.0, 0.0, yaw];
    let solver: SolverFn = unsafe { core::mem::transmute(solver) };
    unsafe { solver(0, count, &mut output, &direction, mode) };
    let bytes = (output.end as usize).checked_sub(begin as usize)?;
    if output.begin != begin.cast() || bytes != count * 16 {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for slot in 0..count {
        let values = unsafe { begin.add(slot).read().assume_init() };
        let point = Vec3 {
            x: values[0],
            y: values[1],
            z: values[2],
        };
        if !point.finite() {
            return None;
        }
        points.push(point);
    }
    Some(points)
}

unsafe fn preview(
    cursor: *mut c_void,
    game: &GameAccessV1,
    arrows: &[SquadArrow],
) -> Option<Preview> {
    let mut squads = Vec::with_capacity(arrows.len());
    for arrow in arrows.iter().filter(|arrow| arrow.infantry) {
        let facets = unsafe { get(arrow.squad, 0xb0) };
        if facets.is_null() || unsafe { ptr(facets, 0x28) }.is_null() {
            continue;
        }
        let point = unsafe { get(arrow.renderer, 0x70) } as *const Vec3;
        if point.is_null() {
            return None;
        }
        let point = unsafe { *point };
        if !point.finite() {
            return None;
        }
        squads.push((arrow.squad, point));
    }

    let yaw = if unsafe { *cursor.cast::<u8>().add(0xa8) } != 0 {
        let x = unsafe { *cursor.cast::<u8>().add(0x30).cast::<f32>() };
        let y = unsafe { *cursor.cast::<u8>().add(0x34).cast::<f32>() };
        y.atan2(x)
    } else {
        0.0
    };
    if !yaw.is_finite() {
        return None;
    }

    let mut points = Vec::new();
    let mut tracks = Vec::new();
    let mut tracking_available = true;
    for &(squad, base) in &squads {
        let Some(selected) = (unsafe { selected_members(squad, game) }) else {
            continue;
        };
        if selected.is_empty() {
            continue;
        }
        let mode = unsafe { formation_mode(squad) };
        let Some(offsets) = (unsafe { formation(selected.len(), yaw, mode) }) else {
            continue;
        };
        let before = points.len();
        for offset in offsets {
            let point = base.plus(offset);
            if point.finite() {
                points.push(point);
            }
        }
        if tracking_available && points.len() == before + selected.len() {
            for member in selected {
                if let Some(track) = unsafe { position_track(member) } {
                    tracks.push(track);
                } else {
                    tracking_available = false;
                    tracks.clear();
                    break;
                }
            }
        } else {
            tracking_available = false;
            tracks.clear();
        }
        if points.len() >= MAX_MARKERS {
            break;
        }
    }
    (points.len() <= MAX_MARKERS).then_some(Preview { points, tracks })
}

unsafe fn vehicle_targets(arrows: &[SquadArrow]) -> Option<Preview> {
    let mut points = Vec::new();
    let mut tracks = Vec::new();
    let mut tracking_available = true;
    for arrow in arrows.iter().filter(|arrow| !arrow.infantry) {
        let point = unsafe { get(arrow.renderer, 0x70) } as *const Vec3;
        let point = unsafe { point.as_ref() }?;
        if !point.finite() {
            return None;
        }
        points.push(*point);
        if tracking_available {
            if let Some(track) = unsafe { position_track(arrow.squad) } {
                tracks.push(track);
            } else {
                tracking_available = false;
                tracks.clear();
            }
        }
    }
    (!points.is_empty()).then_some(Preview { points, tracks })
}

unsafe extern "C" fn set_move_handler(handler: *const MovePreviewHandlerV1) -> i32 {
    let Some(handler) = (unsafe { handler.as_ref() }) else {
        return 1;
    };
    let copy = MovePreviewHandlerV1 {
        context: handler.context,
        preview: handler.preview,
        confirm: handler.confirm,
    };
    match MOVE_HANDLER.set(MoveHandler(copy)) {
        Ok(()) => 0,
        Err(_) => 2,
    }
}

/// The cursor's ground point and the infantry members its order would move:
/// the mean of the infantry arrows' bases, and each squad's selected members.
unsafe fn move_targets(
    game: &GameAccessV1,
    arrows: &[SquadArrow],
) -> Option<(Vec3, Vec<*mut c_void>)> {
    let mut sum = Vec3::default();
    let mut squads = 0;
    let mut members = Vec::new();
    for arrow in arrows.iter().filter(|arrow| arrow.infantry) {
        let point = unsafe { get(arrow.renderer, 0x70) } as *const Vec3;
        let Some(point) = (unsafe { point.as_ref() }) else {
            continue;
        };
        if !point.finite() {
            continue;
        }
        let Some(selected) = (unsafe { selected_members(arrow.squad, game) }) else {
            continue;
        };
        sum = sum.plus(*point);
        squads += 1;
        members.extend(selected);
    }
    if squads == 0 || members.is_empty() || members.len() > MAX_MARKERS {
        return None;
    }
    let scale = 1.0 / squads as f32;
    let point = Vec3 {
        x: sum.x * scale,
        y: sum.y * scale,
        z: sum.z * scale,
    };
    Some((point, members))
}

/// The hovered move command's point and its selection's infantry members:
/// the facet builds the command at the mouse's terrain (or entity) point,
/// stored at +0x1c, with its selection at +0xd8.
unsafe fn hover_targets(
    game: &GameAccessV1,
    cursor: *mut c_void,
) -> Option<(Vec3, Vec<*mut c_void>)> {
    let point = unsafe { *cursor.cast::<u8>().add(0x1c).cast::<Vec3>() };
    if !point.finite() {
        return None;
    }
    let begin = unsafe { ptr(cursor, 0xd8) } as usize;
    let end = unsafe { ptr(cursor, 0xe0) } as usize;
    if begin == 0 || end < begin || (end - begin) / 8 > MAX_MARKERS {
        return None;
    }
    let mut members = Vec::new();
    for entry in (begin..end).step_by(8) {
        let entry = unsafe { *(entry as *const *mut c_void) };
        let squad = unsafe { get_field(entry, 0x10) };
        if squad.is_null() {
            continue;
        }
        let kind = unsafe { method(squad, 0x98) };
        if kind == 0 {
            continue;
        }
        let kind: unsafe extern "C" fn(*mut c_void, u32) -> u8 =
            unsafe { core::mem::transmute(kind) };
        if unsafe { kind(squad, 0x10) } == 0 {
            continue;
        }
        if let Some(selected) = unsafe { selected_members(squad, game) } {
            members.extend(selected);
        }
    }
    (!members.is_empty() && members.len() <= MAX_MARKERS).then_some((point, members))
}

unsafe fn get_field(object: *mut c_void, offset: usize) -> *mut c_void {
    if object.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { ptr(object, offset) }
}

/// Ask the move-preview handler for the markers at `cursor` (a ground point)
/// for `members`. `None` when no handler is set or it is inactive, so the
/// stock formation preview draws.
unsafe fn handled_preview(cursor: Vec3, members: &[*mut c_void]) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
    let handler = &MOVE_HANDLER.get()?.0;
    let mut points = vec![Vec3::default(); MAX_MARKERS + MAX_CANDIDATES];
    let mut assigned = 0;
    let written = unsafe {
        (handler.preview)(
            handler.context,
            &cursor.x,
            members.as_ptr(),
            members.len(),
            points.as_mut_ptr().cast::<f32>(),
            points.len(),
            &mut assigned,
        )
    };
    let written = usize::try_from(written).ok()?.min(points.len());
    points.truncate(written);
    points.retain(|point| point.finite());
    let candidates = points.split_off(assigned.min(points.len()).min(MAX_MARKERS));
    Some((
        points,
        candidates.into_iter().take(MAX_CANDIDATES).collect(),
    ))
}

/// Let the move-preview handler give this order. True when it did, so the
/// stock move is skipped.
unsafe fn handled_order(cursor: *mut c_void) -> bool {
    let (Some(handler), Some(game)) = (MOVE_HANDLER.get(), GAME_ACCESS.get()) else {
        return false;
    };
    let Some(arrows) = (unsafe { cursor_arrows(cursor) }) else {
        return false;
    };
    let Some((point, members)) = (unsafe { move_targets(game, &arrows) }) else {
        return false;
    };
    let handler = &handler.0;
    unsafe { (handler.confirm)(handler.context, &point.x, members.as_ptr(), members.len()) != 0 }
}

unsafe extern "C" fn issue_order(cursor: *mut c_void) {
    let original = ISSUE_ORDER_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    if unsafe { handled_order(cursor) } {
        unsafe { visual().lock().unwrap().dispose() };
        unsafe { candidate_visual().lock().unwrap().dispose() };
        return;
    }
    let vehicles =
        unsafe { cursor_arrows(cursor) }.and_then(|arrows| unsafe { vehicle_targets(&arrows) });
    if let Some(Preview { points, .. }) = &vehicles {
        let mut visual = vehicle_visual().lock().unwrap();
        unsafe { visual.update(Some(cursor), points) };
        visual.source = 0;
    }
    let original: IssueFn = unsafe { core::mem::transmute(original) };
    cache().lock().unwrap().next_move_point_starts_order = true;
    unsafe { original(cursor) };
    cache().lock().unwrap().next_move_point_starts_order = false;
    if let Some(preview) = vehicles {
        vehicle_cache()
            .lock()
            .unwrap()
            .start_vehicle_order(preview, Instant::now());
    }
}

/// The move command the facet rebuilds each frame while no drag is under
/// way: while the move-preview handler is active, its markers follow the
/// command's point. When it turns inactive the markers it drew go. The
/// command dies next frame, so the visuals keep no source; they live in the
/// command's scene (+0x10, the object every move command is built with) and
/// take the last drag's cursor state, since a hovered command's is not an
/// arrow's.
unsafe fn hover(cursor: *mut c_void) {
    if MOVE_HANDLER.get().is_none() {
        return;
    }
    let Some(game) = GAME_ACCESS.get() else {
        return;
    };
    // diagnostic: the facet hovers a move command.
    #[cfg(feature = "diagnostics")]
    if !HOVER_SEEN.swap(true, Ordering::AcqRel) {
        log(
            LOG_DEBUG,
            "cover markers: the smart cursor hovers a move command",
        );
    }
    {
        let mut markers = cache().lock().unwrap();
        let now = Instant::now();
        if markers
            .preview_attempt_at
            .is_some_and(|previous| now.duration_since(previous) < PREVIEW_INTERVAL)
        {
            return;
        }
        markers.preview_attempt_at = Some(now);
    }
    let targets = unsafe { hover_targets(game, cursor) };
    // diagnostic: why the hover draws nothing, or what it draws, once each.
    #[cfg(feature = "diagnostics")]
    let had_targets = targets.is_some();
    #[cfg(feature = "diagnostics")]
    if !had_targets {
        hover_outcome(0, || {
            "cover markers: the hovered command has no point or no infantry".into()
        });
    }
    let handled = targets.and_then(|(point, members)| unsafe { handled_preview(point, &members) });
    #[cfg(feature = "diagnostics")]
    match &handled {
        None if had_targets => hover_outcome(1, || {
            "cover markers: the move-preview handler gave no hover preview".into()
        }),
        Some((assigned, candidates)) => hover_outcome(2, || {
            format!(
                "cover markers: hover draws {} assigned and {} other points",
                assigned.len(),
                candidates.len()
            )
        }),
        None => {}
    }
    let scene = unsafe { ptr(cursor, 0x10) } as usize;
    let mut snapshot = DRAG_SNAPSHOT
        .lock()
        .unwrap()
        .unwrap_or_else(CursorSnapshot::arrow);
    // A drag too short to rotate leaves a state whose markers do not show;
    // see [`CursorSnapshot::arrow`].
    snapshot.rotating = 1;
    // diagnostic: the hovered command's scene and cursor state, once.
    #[cfg(feature = "diagnostics")]
    hover_outcome(3, || {
        let movement = unsafe { core::slice::from_raw_parts(cursor.cast::<u8>().add(0x18), 0x24) };
        let rotating = unsafe { *cursor.cast::<u8>().add(0xa8) };
        format!("cover markers: hover scene {scene:#x}, command +0x18 {movement:02x?}, +0xa8 {rotating}")
    });
    match handled {
        Some((assigned, candidates)) if scene != 0 => {
            for (visual, points) in [(visual(), &assigned), (candidate_visual(), &candidates)] {
                let mut visual = visual.lock().unwrap();
                if visual.source != 0 || visual.scene != scene {
                    unsafe { visual.dispose() };
                    visual.scene = scene;
                }
                visual.snapshot = snapshot;
                unsafe { visual.update(None, points) };
                // diagnostic: each hover draw's markers (first 12).
                #[cfg(feature = "diagnostics")]
                if HOVER_TRACE.fetch_add(1, Ordering::AcqRel) < 12 {
                    log(
                        LOG_DEBUG,
                        &format!("cover markers: hover drew {}", visual.describe()),
                    );
                }
                // diagnostic: whether the hover markers exist after drawing, once.
                #[cfg(feature = "diagnostics")]
                hover_outcome(4, || {
                    format!(
                        "cover markers: hover visual built {} for {} points",
                        visual.cursor.is_some(),
                        visual.count
                    )
                });
            }
            HOVERING.store(true, Ordering::Release);
        }
        _ => unsafe { end_hover() },
    }
}

/// Removes the markers [`hover`] drew, if it drew any.
unsafe fn end_hover() {
    if HOVERING.swap(false, Ordering::AcqRel) {
        // diagnostic: each removal of hover markers (within the first 12 traces).
        #[cfg(feature = "diagnostics")]
        if HOVER_TRACE.fetch_add(1, Ordering::AcqRel) < 12 {
            log(LOG_DEBUG, "cover markers: hover markers removed");
        }
        unsafe { visual().lock().unwrap().dispose() };
        unsafe { candidate_visual().lock().unwrap().dispose() };
    }
}

/// The smart cursor's per-frame update. Its right-button command (+0x48) is
/// a move command over open ground; outside a drag (command state 1, where
/// [`cursor_update`] previews) it is hovered.
unsafe extern "C" fn facet_update(facet: *mut c_void, dt: f32) {
    let original = FACET_UPDATE_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let original: FacetUpdateFn = unsafe { core::mem::transmute(original) };
    unsafe { original(facet, dt) };
    let command = unsafe { get_field(facet, 0x48) };
    let vtable = MOVE_COMMAND_VTABLE.load(Ordering::Acquire);
    // diagnostic: the facet's left (+0x38) and right (+0x48) command classes
    // and the right one's state, at each change (first 40), against the move
    // command's vtable and whether a move-preview handler is registered.
    #[cfg(feature = "diagnostics")]
    if FACET_LOGGED.load(Ordering::Acquire) < 40 {
        let class = |field: usize| {
            let command = unsafe { get_field(facet, field) };
            if command.is_null() {
                0
            } else {
                unsafe { *command.cast::<usize>() }
            }
        };
        let (left, right) = (class(0x38), class(0x48));
        let state = if right == vtable && right != 0 {
            let state: unsafe extern "C" fn(*mut c_void) -> i32 =
                unsafe { core::mem::transmute(method(command, 0x38)) };
            unsafe { state(command) }
        } else {
            -1
        };
        let mut seen = FACET_SEEN.lock().unwrap();
        if *seen != Some((left, right, state)) {
            *seen = Some((left, right, state));
            FACET_LOGGED.fetch_add(1, Ordering::AcqRel);
            log(
                LOG_DEBUG,
                &format!(
                    "cover markers: facet {facet:p} left {left:#x} right {right:#x} state {state} (move {vtable:#x}, handler {})",
                    MOVE_HANDLER.get().is_some()
                ),
            );
        }
    }
    if MOVE_HANDLER.get().is_none() {
        return;
    }
    if command.is_null() || vtable == 0 || unsafe { *command.cast::<usize>() } != vtable {
        unsafe { end_hover() };
        return;
    }
    let state: unsafe extern "C" fn(*mut c_void) -> i32 =
        unsafe { core::mem::transmute(method(command, 0x38)) };
    if unsafe { state(command) } != 1 {
        unsafe { hover(command) };
    }
}

unsafe extern "C" fn cursor_update(cursor: *mut c_void, transform: *const f32) {
    let original = CURSOR_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    let original: CursorFn = unsafe { core::mem::transmute(original) };
    unsafe { original(cursor, transform) };
    let arrows = unsafe { cursor_arrows(cursor) };
    let Some(arrows) = arrows else {
        return;
    };
    HOVERING.store(false, Ordering::Release);
    unsafe { hide_stock_infantry_arrows(&arrows) };
    let preview_due = {
        let mut markers = cache().lock().unwrap();
        let now = Instant::now();
        if markers
            .preview_attempt_at
            .is_some_and(|previous| now.duration_since(previous) < PREVIEW_INTERVAL)
        {
            false
        } else {
            markers.preview_attempt_at = Some(now);
            true
        }
    };
    if !preview_due {
        unsafe { visual().lock().unwrap().refresh(cursor) };
        unsafe { candidate_visual().lock().unwrap().refresh(cursor) };
        return;
    }
    if let Some(game) = GAME_ACCESS.get() {
        let handled = unsafe { move_targets(game, &arrows) }
            .and_then(|(point, members)| unsafe { handled_preview(point, &members) });
        if let Some((assigned, candidates)) = handled {
            {
                let mut visual = visual().lock().unwrap();
                unsafe { visual.update(Some(cursor), &assigned) };
                *DRAG_SNAPSHOT.lock().unwrap() = Some(visual.snapshot);
                // diagnostic: the drag's markers, to compare with the hover's (first 3).
                #[cfg(feature = "diagnostics")]
                if DRAG_TRACE.fetch_add(1, Ordering::AcqRel) < 3 {
                    log(
                        LOG_DEBUG,
                        &format!("cover markers: drag drew {}", visual.describe()),
                    );
                }
            }
            unsafe {
                candidate_visual()
                    .lock()
                    .unwrap()
                    .update(Some(cursor), &candidates)
            };
            let mut markers = cache().lock().unwrap();
            markers.preview_points.clear();
            markers.preview_tracks.clear();
            markers.preview_at = None;
            return;
        }
        unsafe { candidate_visual().lock().unwrap().dispose() };
        if let Some(Preview { points, tracks }) = unsafe { preview(cursor, game, &arrows) } {
            unsafe { visual().lock().unwrap().update(Some(cursor), &points) };
            let mut markers = cache().lock().unwrap();
            markers.preview_points = points;
            markers.preview_tracks = tracks;
            markers.preview_at = (!markers.preview_points.is_empty()).then(Instant::now);
        }
    }
}

unsafe extern "C" fn cursor_destructor(cursor: *mut c_void, flags: u32) -> *mut c_void {
    {
        let mut markers = cache().lock().unwrap();
        markers.preview_at = None;
        if !markers.moved.is_empty() && markers.owners.is_empty() {
            markers.owners = markers.match_owners();
        }
    }
    {
        let mut visual = visual().lock().unwrap();
        if visual.source == cursor as usize {
            unsafe { visual.dispose() };
        }
    }
    {
        let mut candidates = candidate_visual().lock().unwrap();
        if candidates.source == cursor as usize {
            unsafe { candidates.dispose() };
        }
    }
    let original = CURSOR_DESTRUCTOR_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return core::ptr::null_mut();
    }
    let original: DestructorFn = unsafe { core::mem::transmute(original) };
    unsafe { original(cursor, flags) }
}

unsafe extern "C" fn mission_deleting_destructor(state: *mut c_void, flags: u32) -> *mut c_void {
    // The fake cursor borrows the tactical scene. Release its renderers while
    // that scene is still alive, before the game's state destructor runs.
    unsafe { visual().lock().unwrap().dispose() };
    unsafe { candidate_visual().lock().unwrap().dispose() };
    let mut visuals = order_visuals().lock().unwrap();
    for order in visuals.iter_mut() {
        unsafe { order.visual.dispose() };
    }
    visuals.clear();
    drop(visuals);
    unsafe { vehicle_visual().lock().unwrap().dispose() };
    TERRAIN_SCENE.store(0, Ordering::Release);
    if let Some(pending) = BOARDING_PENDING.get() {
        pending.lock().unwrap().clear();
    }
    if let Some(pending) = VEHICLE_STOP_PENDING.get() {
        pending.lock().unwrap().clear();
    }
    *cache().lock().unwrap() = Markers::default();
    *vehicle_cache().lock().unwrap() = Markers::default();
    let original = MISSION_DELETING_DESTRUCTOR_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return core::ptr::null_mut();
    }
    let original: DestructorFn = unsafe { core::mem::transmute(original) };
    unsafe { original(state, flags) }
}

unsafe extern "C" fn move_point(location: *mut c_void, point: *const Vec3) -> *mut c_void {
    let original = MOVE_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return core::ptr::null_mut();
    }
    if let Some(&point) = unsafe { point.as_ref() } {
        if point.finite() {
            let (scene, snapshot) = {
                let current = visual().lock().unwrap();
                (current.scene, current.snapshot)
            };
            let mut markers = cache().lock().unwrap();
            let now = Instant::now();
            markers.record_move_point(point, now, scene, snapshot);
        }
    }
    let original: LocationFn = unsafe { core::mem::transmute(original) };
    unsafe { original(location, point) }
}

unsafe fn is_stop_order(handle: *mut c_void) -> bool {
    // R8 holds a reference handle whose +0x10 field points to the order object.
    let stop = STOP_ORDER_VTABLE.load(Ordering::Acquire);
    if stop == 0 || handle.is_null() {
        return false;
    }
    let wrapper = unsafe { ptr(handle, 0) };
    if wrapper.is_null() {
        return false;
    }
    let order = unsafe { ptr(wrapper, 0x10) };
    !order.is_null() && unsafe { ptr(order, 0) } as usize == stop
}

unsafe extern "C" fn squad_order(
    squad_ai: *mut c_void,
    result: *mut c_void,
    handle: *mut c_void,
) -> *mut c_void {
    let original = SQUAD_ORDER_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return core::ptr::null_mut();
    }
    let is_stop = unsafe { is_stop_order(handle) };
    let stopped = if is_stop {
        GAME_ACCESS.get().and_then(|game| {
            let squad = unsafe { (game.entity_from_facet)(squad_ai) };
            if squad.is_null() {
                return None;
            }
            unsafe { selected_members(squad, game) }.map(|members| {
                members
                    .into_iter()
                    .filter_map(|member| unsafe { position_track(member) })
                    .collect::<Vec<_>>()
            })
        })
    } else {
        None
    };
    let original: SquadOrderFn = unsafe { core::mem::transmute(original) };
    let returned = unsafe { original(squad_ai, result, handle) };
    if let Some(stopped) = stopped {
        let mut markers = cache().lock().unwrap();
        let retired = markers.retire_tracks(&stopped);
        drop(markers);
        if retired != 0 {
            log(
                LOG_DEBUG,
                &format!("cover markers Stop retired {retired} arrows"),
            );
        }
    }
    returned
}

unsafe extern "C" fn stop_order_factory(manager: *mut c_void, unit: *mut c_void) -> *mut c_void {
    let original = STOP_FACTORY_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return core::ptr::null_mut();
    }
    let tracked_vehicle = unsafe { position_track(unit) }
        .filter(|track| vehicle_cache().lock().unwrap().moved_tracks.contains(track));
    let original: StopFactoryFn = unsafe { core::mem::transmute(original) };
    let order = unsafe { original(manager, unit) };
    if let Some(track) = tracked_vehicle {
        let mut pending = vehicle_stop_pending().lock().unwrap();
        if !pending.contains(&track) {
            pending.push(track);
        }
    }
    order
}

unsafe fn replacement_handoff(
    soldier_ai: *mut c_void,
    handle: *mut c_void,
    direction: f32,
    original: &AtomicUsize,
    boarding: bool,
) {
    let original = original.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    // Capture the recipient before the native handoff can change or remove its facets.
    let track = GAME_ACCESS.get().and_then(|game| {
        let soldier = unsafe { (game.entity_from_facet)(soldier_ai) };
        unsafe { position_track(soldier) }
    });
    let original: SoldierOrderFn = unsafe { core::mem::transmute(original) };
    unsafe { original(soldier_ai, handle, direction) };
    if let Some(track) = track {
        if boarding {
            // Finish boarding's order-issue call before clearing its soldier markers.
            let mut pending = boarding_pending().lock().unwrap();
            if !pending.contains(&track) {
                pending.push(track);
            }
        } else {
            let retired = cache().lock().unwrap().retire_tracks(&[track]);
            if retired != 0 {
                log(
                    LOG_DEBUG,
                    &format!("cover markers garrison retired {retired} arrows"),
                );
            }
        }
    }
}

unsafe extern "C" fn garrison_handoff(
    soldier_ai: *mut c_void,
    handle: *mut c_void,
    direction: f32,
) {
    unsafe {
        replacement_handoff(
            soldier_ai,
            handle,
            direction,
            &GARRISON_HANDOFF_ORIGINAL,
            false,
        )
    };
}

unsafe extern "C" fn boarding_handoff(
    soldier_ai: *mut c_void,
    handle: *mut c_void,
    direction: f32,
) {
    unsafe {
        replacement_handoff(
            soldier_ai,
            handle,
            direction,
            &BOARDING_HANDOFF_ORIGINAL,
            true,
        )
    };
}

unsafe fn update_order_visuals(displays: Vec<OrderDisplay>) {
    let mut visuals = order_visuals().lock().unwrap();
    visuals.retain_mut(|entry| {
        if displays.iter().any(|display| display.id == entry.id) {
            true
        } else {
            unsafe { entry.visual.dispose() };
            false
        }
    });
    for display in displays {
        let index = if let Some(index) = visuals.iter().position(|entry| entry.id == display.id) {
            index
        } else {
            let mut visual = infantry_visual();
            visual.scene = display.scene;
            visual.snapshot = display.snapshot;
            visuals.push(OrderVisual {
                id: display.id,
                visual,
            });
            visuals.len() - 1
        };
        unsafe { visuals[index].visual.update(None, &display.points) };
    }
}

/// Each mission frame, from the loader's `mission-events` service: retires
/// arrived and handed-off orders and refreshes the markers, before the game
/// draws the frame.
unsafe extern "C" fn mission_frame(_: *mut c_void, _: u32, frame: *const MissionFrameV1) {
    let Some(frame) = (unsafe { frame.as_ref() }) else {
        return;
    };
    TERRAIN_SCENE.store(frame.scene as usize, Ordering::Release);
    let pending = std::mem::take(&mut *boarding_pending().lock().unwrap());
    let stopped_vehicles = std::mem::take(&mut *vehicle_stop_pending().lock().unwrap());
    let (displays, boarding_retired) = {
        let mut markers = cache().lock().unwrap();
        let boarding_retired = markers.retire_tracks(&pending);
        let now = Instant::now();
        if !markers.moved.is_empty() && markers.arrived(now) {
            markers.clear_order();
            log(LOG_DEBUG, "cover markers cleared at squad arrival");
        }
        markers.retained.retain_mut(|order| !order.arrived(now));
        (markers.visible_orders(), boarding_retired)
    };
    if !pending.is_empty() {
        log(
            LOG_DEBUG,
            &format!(
                "cover markers boarding received {} soldiers, retired {boarding_retired} arrows",
                pending.len()
            ),
        );
    }
    unsafe { update_order_visuals(displays) };
    let (next_vehicle, vehicle_stopped) = {
        let mut markers = vehicle_cache().lock().unwrap();
        let vehicle_stopped = markers.retire_tracks(&stopped_vehicles);
        if !markers.moved.is_empty() && markers.arrived(Instant::now()) {
            markers.clear_order();
            log(LOG_DEBUG, "cover markers vehicle cleared at arrival");
        }
        (markers.visible_points(), vehicle_stopped)
    };
    if !stopped_vehicles.is_empty() {
        log(
            LOG_DEBUG,
            &format!(
                "cover markers vehicle Stop received {} vehicles, retired {vehicle_stopped} arrows",
                stopped_vehicles.len()
            ),
        );
    }
    {
        let mut visual = vehicle_visual().lock().unwrap();
        if next_vehicle.is_empty() {
            if visual.cursor.is_some() {
                unsafe { visual.dispose() };
            }
        } else {
            unsafe { visual.update(None, &next_vehicle) };
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn ReadProcessMemory(
        process: *mut c_void,
        address: *const c_void,
        buffer: *mut c_void,
        length: usize,
        read: *mut usize,
    ) -> i32;
}

enum InstallError {
    /// A site did not resolve: this build is not one the plugin supports.
    UnsupportedBuild(String),
    Failed(String),
}

unsafe fn module(api: &Api, name: &std::ffi::CStr) -> Result<(*mut u8, usize), InstallError> {
    let base = unsafe { (api.module_base)(name.as_ptr()) }.cast::<u8>();
    if base.is_null() {
        return Err(InstallError::Failed(format!(
            "{} is not loaded",
            name.to_string_lossy()
        )));
    }
    Ok((base, unsafe { (api.module_size)(base.cast()) }))
}

/// A loaded module's image, readable in place.
unsafe fn image<'a>(base: *mut u8, size: usize) -> sites::Image<'a> {
    sites::Image {
        image: unsafe { core::slice::from_raw_parts(base, size) },
        base: base as usize,
    }
}

/// One hooked site: its module, rva, kind and the bytes the hook replaces.
struct Hooked {
    module: &'static std::ffi::CStr,
    base: *mut u8,
    rva: usize,
    kind: u32,
    length: usize,
}

impl Hooked {
    fn at(&self) -> *mut u8 {
        self.base.wrapping_add(self.rva)
    }

    fn before(&self) -> Vec<u8> {
        unsafe { core::slice::from_raw_parts(self.at(), self.length) }.to_vec()
    }
}

/// Every resolved site, in the order `install` hooks them.
struct Resolved {
    game: *mut u8,
    logic: *mut u8,
    world: *mut u8,
    game_sites: sites::Game,
    logic_sites: sites::Logic,
    world_sites: sites::World,
}

impl Resolved {
    fn entry(module: &'static std::ffi::CStr, base: *mut u8, rva: usize, before: &[u8]) -> Hooked {
        Hooked {
            module,
            base,
            rva,
            kind: PATCH_KIND_ENTRY,
            length: before.len(),
        }
    }

    fn call(module: &'static std::ffi::CStr, base: *mut u8, rva: usize) -> Hooked {
        Hooked {
            module,
            base,
            rva,
            kind: PATCH_KIND_CALL,
            length: CALL_LENGTH,
        }
    }

    /// cursor update, cursor destructor, vehicle order, mission destructor,
    /// move destination, squad order, Stop factory, garrison handoff, boarding
    /// handoff, smart cursor update.
    fn hooks(&self) -> [Hooked; 10] {
        let (game, logic) = (self.game, self.logic);
        let (g, l) = (&self.game_sites, &self.logic_sites);
        [
            Self::entry(c"game.dll", game, g.cursor_update, CURSOR_BEFORE),
            Self::entry(
                c"game.dll",
                game,
                g.cursor_destructor,
                CURSOR_DESTRUCTOR_BEFORE,
            ),
            Self::entry(c"game.dll", game, g.issue_order, ISSUE_ORDER_BEFORE),
            Self::entry(
                c"game.dll",
                game,
                g.mission_deleting_destructor,
                MISSION_DELETING_DESTRUCTOR_BEFORE,
            ),
            Self::call(c"logic.dll", logic, l.move_point_call),
            Self::entry(
                c"logic.dll",
                logic,
                l.squad_order_dispatch,
                SQUAD_ORDER_DISPATCH_BEFORE,
            ),
            Self::call(c"logic.dll", logic, l.stop_order_factory_call),
            Self::call(c"logic.dll", logic, l.garrison_handoff_call),
            Self::call(c"logic.dll", logic, l.boarding_handoff_call),
            Self::entry(c"game.dll", game, g.facet_update, FACET_UPDATE_BEFORE),
        ]
    }
}

/// The instructions each of [`Resolved::hooks`] replaces; `None` for calls.
const ENTRY_BEFORE: [Option<&[u8]>; 10] = [
    Some(CURSOR_BEFORE),
    Some(CURSOR_DESTRUCTOR_BEFORE),
    Some(ISSUE_ORDER_BEFORE),
    Some(MISSION_DELETING_DESTRUCTOR_BEFORE),
    None,
    Some(SQUAD_ORDER_DISPATCH_BEFORE),
    None,
    None,
    None,
    Some(FACET_UPDATE_BEFORE),
];

/// Finds every site and checks each entry hook's stolen instructions.
unsafe fn resolve(api: &Api) -> Result<Resolved, InstallError> {
    let (game, game_size) = unsafe { module(api, c"game.dll") }?;
    let (logic, logic_size) = unsafe { module(api, c"logic.dll") }?;
    let (world, world_size) = unsafe { module(api, c"world2.dll") }?;
    let unsupported = |dll: &'static str| {
        move |error: String| InstallError::UnsupportedBuild(format!("{dll}: {error}"))
    };
    let resolved = Resolved {
        game,
        logic,
        world,
        game_sites: unsafe { image(game, game_size) }
            .game()
            .map_err(unsupported("game.dll"))?,
        logic_sites: unsafe { image(logic, logic_size) }
            .logic()
            .map_err(unsupported("logic.dll"))?,
        world_sites: unsafe { image(world, world_size) }
            .world()
            .map_err(unsupported("world2.dll"))?,
    };
    // Each entry hook relocates exactly these instructions, so a build whose
    // prologue differs is refused rather than half-copied.
    let sizes = [(game, game_size), (logic, logic_size), (world, world_size)];
    for (site, expected) in resolved.hooks().iter().zip(ENTRY_BEFORE) {
        let Some(expected) = expected else { continue };
        let size = sizes
            .iter()
            .find(|(base, _)| *base == site.base)
            .map_or(0, |s| s.1);
        if site
            .rva
            .checked_add(expected.len())
            .is_none_or(|end| end > size)
            || site.before() != expected
        {
            return Err(InstallError::UnsupportedBuild(format!(
                "{}: the hook at {:#x} starts differently",
                site.module.to_string_lossy(),
                site.rva
            )));
        }
    }
    Ok(resolved)
}

unsafe fn hook(
    api: &Api,
    site: &Hooked,
    detour: *mut c_void,
    original: &AtomicUsize,
    what: &str,
) -> Result<(), String> {
    let mut out = core::ptr::null_mut();
    let status = if site.kind == PATCH_KIND_CALL {
        unsafe { (api.hook_call)(site.at().cast(), detour, &mut out) }
    } else {
        unsafe { (api.hook_exact)(site.at().cast(), detour, site.length, &mut out) }
    };
    if status != 0 || out.is_null() {
        return Err(format!("{what} hook failed"));
    }
    original.store(out as usize, Ordering::Release);
    Ok(())
}

/// Subscribe [`mission_frame`] to the loader's mission frames. Without
/// them the markers still draw but are not refreshed or retired.
fn subscribe_frames() {
    let Some(events) = (unsafe { defiance_feature_sdk::services::mission_events() }) else {
        log(
            LOG_WARN,
            "cover markers: the loader has no mission-events service; markers are not refreshed",
        );
        return;
    };
    let id = unsafe { (events.subscribe)(MISSION_FRAME, mission_frame, core::ptr::null_mut()) };
    if id == 0 {
        log(
            LOG_WARN,
            "cover markers: mission frames could not be subscribed; markers are not refreshed",
        );
    } else if unsafe { (events.frames)() } == 0 {
        log(
            LOG_WARN,
            "cover markers: Core feeds no mission frames; markers are not refreshed",
        );
    }
}

unsafe fn install(api: &Api) -> Result<(), InstallError> {
    let resolved = unsafe { resolve(api) }?;
    let Some(game_access) = (unsafe { defiance_feature_sdk::services::game_access() }) else {
        return Err(InstallError::Failed(
            "Core game-access service is unavailable".into(),
        ));
    };
    GAME_ACCESS
        .set(game_access)
        .map_err(|_| InstallError::Failed("already initialized".into()))?;
    let (game, logic, world) = (
        resolved.game as usize,
        resolved.logic as usize,
        resolved.world as usize,
    );
    let (g, l, w) = (
        &resolved.game_sites,
        &resolved.logic_sites,
        &resolved.world_sites,
    );
    SOLVER.store(logic + l.formation_solver, Ordering::Release);
    CONSTRUCTOR.store(game + g.cursor_constructor, Ordering::Release);
    FACTORY.store(game + g.cursor_factory, Ordering::Release);
    RENDER_UPDATE.store(game + g.cursor_render_update, Ordering::Release);
    STOP_ORDER_VTABLE.store(logic + l.stop_order_vtable, Ordering::Release);
    MOVE_COMMAND_VTABLE.store(game + g.move_command_vtable, Ordering::Release);
    TERRAIN_RENDERER_VTABLE.store(world + w.terrain_renderer_vtable, Ordering::Release);
    MAP_IMPL_VTABLE.store(world + w.map_impl_vtable, Ordering::Release);
    MAP_SAMPLE_HEIGHT.store(world + w.map_sample_height, Ordering::Release);
    RENDERER_SCALE_SETTER.store(world + w.renderer_scale_setter, Ordering::Release);

    let hooks = resolved.hooks();
    let detours: [(*mut c_void, &AtomicUsize, &str); 10] = [
        (
            cursor_update as *mut c_void,
            &CURSOR_ORIGINAL,
            "move cursor",
        ),
        (
            cursor_destructor as *mut c_void,
            &CURSOR_DESTRUCTOR_ORIGINAL,
            "move cursor destructor",
        ),
        (
            issue_order as *mut c_void,
            &ISSUE_ORDER_ORIGINAL,
            "vehicle move order",
        ),
        (
            mission_deleting_destructor as *mut c_void,
            &MISSION_DELETING_DESTRUCTOR_ORIGINAL,
            "mission cleanup",
        ),
        (
            move_point as *mut c_void,
            &MOVE_ORIGINAL,
            "move destination",
        ),
        (
            squad_order as *mut c_void,
            &SQUAD_ORDER_ORIGINAL,
            "squad order",
        ),
        (
            stop_order_factory as *mut c_void,
            &STOP_FACTORY_ORIGINAL,
            "Stop order factory",
        ),
        (
            garrison_handoff as *mut c_void,
            &GARRISON_HANDOFF_ORIGINAL,
            "garrison handoff",
        ),
        (
            boarding_handoff as *mut c_void,
            &BOARDING_HANDOFF_ORIGINAL,
            "boarding handoff",
        ),
        (
            facet_update as *mut c_void,
            &FACET_UPDATE_ORIGINAL,
            "smart cursor update",
        ),
    ];
    for (site, (detour, original, what)) in hooks.iter().zip(detours) {
        unsafe { hook(api, site, detour, original, what) }.map_err(InstallError::Failed)?;
    }
    subscribe_frames();
    if let Err(code) =
        unsafe { defiance_feature_sdk::services::register(c"move-preview", 1, &MOVE_PREVIEW) }
    {
        log(
            LOG_WARN,
            &format!("cover markers: the move-preview service was not registered ({code})"),
        );
    }
    Ok(())
}

unsafe extern "C" fn init(api: *const Api) -> i32 {
    let Some(api) = (unsafe { api.as_ref() }) else {
        return 1;
    };
    if api.abi_version != ABI_VERSION || api.reserved != 0 {
        return 1;
    }
    LOG.store(api.log as usize, Ordering::Release);
    match unsafe { install(api) } {
        Ok(()) => {
            log(LOG_INFO, "cover markers installed");
            0
        }
        Err(InstallError::UnsupportedBuild(reason)) => {
            log(
                LOG_WARN,
                &format!("cover markers: not a supported build ({reason}); no writes made"),
            );
            0
        }
        Err(InstallError::Failed(error)) => {
            log(LOG_ERROR, &format!("cover markers refused: {error}"));
            1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn defiance_patch_contract_v1(api: *const Api) -> *const PatchContractV1 {
    let Some(api_ref) = (unsafe { api.as_ref() }) else {
        return core::ptr::null();
    };
    let resolved = match unsafe { resolve(api_ref) } {
        Ok(resolved) => resolved,
        // An unsupported build installs nothing, so it claims nothing.
        Err(InstallError::UnsupportedBuild(_)) => {
            return unsafe { defiance_feature_sdk::contract::build(api, Vec::new()) };
        }
        Err(InstallError::Failed(_)) => return core::ptr::null(),
    };
    let patches = resolved
        .hooks()
        .iter()
        .map(|site| defiance_feature_sdk::contract::Patch {
            module: site.module,
            rva: site.rva,
            kind: site.kind,
            before: site.before(),
            after: None,
        })
        .collect();
    unsafe { defiance_feature_sdk::contract::build(api, patches) }
}

#[no_mangle]
pub extern "C" fn defiance_plugin() -> *const Plugin {
    defiance_api::leak(Plugin {
        abi_version: ABI_VERSION,
        name: c"defiance.cover-markers".as_ptr(),
        version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast(),
        init,
        stop: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use defiance_api::MemberStateV1;

    struct TestMember {
        parent: *mut c_void,
        selected: u8,
    }

    struct TestRoster {
        parent: *mut c_void,
        members: [*mut c_void; 3],
    }

    unsafe extern "C" fn test_null(_value: *mut c_void) -> *mut c_void {
        core::ptr::null_mut()
    }

    unsafe extern "C" fn test_selectable(roster: *mut c_void) -> *mut c_void {
        unsafe { (*(roster as *mut TestRoster)).parent }
    }

    unsafe extern "C" fn test_members(
        roster: *mut c_void,
        output: *mut *mut c_void,
        capacity: usize,
    ) -> usize {
        let members = unsafe { &(*(roster as *mut TestRoster)).members };
        if capacity >= members.len() {
            unsafe { core::ptr::copy_nonoverlapping(members.as_ptr(), output, members.len()) };
        }
        members.len()
    }

    unsafe extern "C" fn test_read_member(
        member: *mut c_void,
        expected: *mut c_void,
        output: *mut MemberStateV1,
    ) -> i32 {
        let member = unsafe { &*(member as *mut TestMember) };
        if member.parent != expected {
            return 1;
        }
        unsafe {
            output.write(MemberStateV1 {
                selected: member.selected,
                enabled: 1,
                ..MemberStateV1::default()
            })
        };
        0
    }

    unsafe extern "C" fn test_set_pin(_facet: *mut c_void, _value: u8, _has_pin: u8) -> i32 {
        0
    }

    unsafe extern "C" fn test_squad_firing(_facet: *mut c_void) -> u8 {
        0
    }

    unsafe extern "C" fn test_set_squad_firing(_facet: *mut c_void, _value: u8) -> i32 {
        0
    }

    #[test]
    fn partial_roster_uses_parent_selectable_and_skips_unselected_members() {
        let parent = 0x1234usize as *mut c_void;
        let mut first = TestMember {
            parent,
            selected: 1,
        };
        let mut second = TestMember {
            parent,
            selected: 0,
        };
        let mut third = TestMember {
            parent,
            selected: 1,
        };
        let mut roster = TestRoster {
            parent,
            members: [
                (&mut first as *mut TestMember).cast(),
                (&mut second as *mut TestMember).cast(),
                (&mut third as *mut TestMember).cast(),
            ],
        };
        let game = GameAccessV1 {
            entity_from_facet: test_null,
            selectable: test_selectable,
            copy_members: test_members,
            read_member: test_read_member,
            set_firing_pin: test_set_pin,
            squad_firing: test_squad_firing,
            set_squad_firing: test_set_squad_firing,
        };
        let selected = unsafe { selected_members((&mut roster as *mut TestRoster).cast(), &game) }
            .expect("valid roster");
        assert_eq!(selected, [roster.members[0], roster.members[2]]);
        unsafe {
            core::ptr::write_volatile(&mut first.selected, 0);
            core::ptr::write_volatile(&mut third.selected, 0);
        }
        let selected = unsafe { selected_members((&mut roster as *mut TestRoster).cast(), &game) }
            .expect("valid roster");
        assert_eq!(selected, roster.members);
        unsafe {
            core::ptr::write_volatile(&mut first.selected, 1);
            core::ptr::write_volatile(&mut second.selected, 1);
            core::ptr::write_volatile(&mut third.selected, 1);
        }
        let selected = unsafe { selected_members((&mut roster as *mut TestRoster).cast(), &game) }
            .expect("valid roster");
        assert_eq!(selected, roster.members);
    }

    unsafe extern "C" fn fill_offsets(
        _context: usize,
        count: usize,
        output: *mut VectorHeader,
        direction: *const [f32; 4],
        mode: u32,
    ) {
        assert_eq!(mode, 3);
        assert_eq!(unsafe { (*direction)[3] }, 0.5);
        let begin = unsafe { (*output).begin.cast::<[f32; 4]>() };
        for index in 0..count {
            unsafe { begin.add(index).write([index as f32, 2.0, 3.0, 0.0]) };
        }
        unsafe { (*output).end = begin.add(count).cast() };
    }

    #[test]
    fn scratch_vector_receives_one_offset_per_member() {
        SOLVER.store(fill_offsets as *const () as usize, Ordering::Release);
        let offsets = unsafe { formation(3, 0.5, 3) }.unwrap();
        assert_eq!(offsets.len(), 3);
        assert_eq!((offsets[0].x, offsets[0].y, offsets[0].z), (0.0, 2.0, 3.0));
        assert_eq!((offsets[2].x, offsets[2].y, offsets[2].z), (2.0, 2.0, 3.0));
        SOLVER.store(0, Ordering::Release);
    }

    #[test]
    fn arrival_uses_horizontal_proximity_even_for_an_elevated_raycast_hit() {
        let facet_vtable = 0x1234usize;
        let mut soldier = Vec3 {
            x: 20.0,
            y: 0.0,
            z: 0.0,
        };
        let start = Instant::now();
        let mut markers = Markers {
            moved: vec![Vec3 {
                x: 0.0,
                y: 0.0,
                z: 40.0,
            }],
            moved_tracks: vec![PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: &soldier as *const Vec3 as usize,
            }],
            starting_positions: vec![Some(soldier)],
            moved_at: Some(start),
            ..Markers::default()
        };
        assert!(!markers.arrived(start + Duration::from_secs(1)));
        unsafe { core::ptr::write_volatile(&mut soldier.x, 2.0) };
        assert!(!markers.arrived(start + Duration::from_millis(1250)));
        assert!(!markers.arrived(start + Duration::from_millis(1500)));
        assert!(markers.arrived(start + Duration::from_millis(3000)));
    }

    #[test]
    fn nearby_order_stays_until_the_soldier_moves() {
        let facet_vtable = 0x1234usize;
        let mut soldier = Vec3 {
            x: 2.0,
            y: 0.0,
            z: 0.0,
        };
        let start = Instant::now();
        let mut markers = Markers {
            moved: vec![Vec3::default()],
            moved_tracks: vec![PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: &soldier as *const Vec3 as usize,
            }],
            starting_positions: vec![Some(soldier)],
            moved_at: Some(start),
            ..Markers::default()
        };
        assert!(!markers.arrived(start + Duration::from_secs(1)));
        assert!(!markers.arrived(start + Duration::from_millis(1250)));
        assert!(!markers.arrived(start + Duration::from_millis(3000)));
        unsafe { core::ptr::write_volatile(&mut soldier.x, 0.5) };
        assert!(!markers.arrived(start + Duration::from_millis(3250)));
        assert!(!markers.arrived(start + Duration::from_millis(3500)));
        assert!(markers.arrived(start + Duration::from_millis(5000)));
    }

    #[test]
    fn each_arrow_clears_when_a_distinct_soldier_settles() {
        let facet_vtable = 0x1234usize;
        let mut first = Vec3 {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };
        let mut second = Vec3 {
            x: 20.0,
            y: 0.0,
            z: 0.0,
        };
        let start = Instant::now();
        let mut markers = Markers {
            moved: vec![
                Vec3::default(),
                Vec3 {
                    x: 30.0,
                    y: 0.0,
                    z: 0.0,
                },
            ],
            moved_tracks: [&first, &second]
                .into_iter()
                .map(|soldier| PositionTrack {
                    facet: &facet_vtable as *const usize as usize,
                    vtable: facet_vtable,
                    position: soldier as *const Vec3 as usize,
                })
                .collect(),
            starting_positions: vec![Some(first), Some(second)],
            moved_at: Some(start),
            ..Markers::default()
        };
        unsafe { core::ptr::write_volatile(&mut first.x, 0.0) };
        assert!(!markers.arrived(start + Duration::from_millis(1250)));
        assert!(!markers.arrived(start + Duration::from_millis(1500)));
        assert!(!markers.arrived(start + Duration::from_millis(3000)));
        assert_eq!(markers.visible_points(), vec![markers.moved[1]]);
        unsafe { core::ptr::write_volatile(&mut second.x, 30.0) };
        assert!(!markers.arrived(start + Duration::from_millis(3250)));
        assert!(!markers.arrived(start + Duration::from_millis(3500)));
        assert!(markers.arrived(start + Duration::from_millis(5000)));
        assert!(markers.visible_points().is_empty());
    }

    #[test]
    fn one_soldier_cannot_clear_two_nearby_arrows() {
        let facet_vtable = 0x1234usize;
        let mut soldier = Vec3 {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };
        let start = Instant::now();
        let mut markers = Markers {
            moved: vec![
                Vec3::default(),
                Vec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
            ],
            moved_tracks: vec![PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: &soldier as *const Vec3 as usize,
            }],
            starting_positions: vec![Some(soldier)],
            moved_at: Some(start),
            ..Markers::default()
        };
        unsafe { core::ptr::write_volatile(&mut soldier.x, 0.25) };
        assert!(!markers.arrived(start + Duration::from_millis(1250)));
        assert!(!markers.arrived(start + Duration::from_millis(1500)));
        assert!(!markers.arrived(start + Duration::from_millis(3000)));
        assert_eq!(markers.visible_points().len(), 1);
        assert!(!markers.arrived(start + Duration::from_millis(5000)));
        assert_eq!(markers.visible_points().len(), 1);
    }

    #[test]
    fn reordering_one_member_keeps_other_pending_destinations() {
        let facet_vtable = 0x1234usize;
        let mut first = Vec3::default();
        let middle = Vec3::default();
        let mut last = Vec3::default();
        let tracks: Vec<_> = [&first, &middle, &last]
            .into_iter()
            .map(|position| PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: position as *const Vec3 as usize,
            })
            .collect();
        let old_points: Vec<_> = [10.0, 20.0, 30.0]
            .map(|x| Vec3 { x, y: 0.0, z: 0.0 })
            .into();
        let next_point = Vec3 {
            x: 50.0,
            y: 0.0,
            z: 0.0,
        };
        let start = Instant::now();
        let mut markers = Markers {
            preview_points: vec![next_point],
            preview_tracks: vec![tracks[1]],
            ordered_preview_points: old_points.clone(),
            order_id: 999,
            order_scene: 1,
            order_snapshot: CursorSnapshot {
                rotating: 1,
                ..CursorSnapshot::default()
            },
            moved: old_points.clone(),
            moved_tracks: tracks.clone(),
            starting_positions: vec![Some(Vec3::default()); 3],
            moved_at: Some(start),
            ..Markers::default()
        };
        markers.start_order(
            vec![tracks[1]],
            start + Duration::from_secs(1),
            1,
            CursorSnapshot {
                rotating: 2,
                ..CursorSnapshot::default()
            },
        );
        markers.moved.push(next_point);
        let displays = markers.visible_orders();
        assert_eq!(displays.len(), 2);
        assert_eq!(displays[0].snapshot.rotating, 2);
        assert_eq!(displays[1].snapshot.rotating, 1);
        assert_eq!(markers.retained.len(), 1);
        assert_eq!(
            markers.retained[0].moved,
            vec![old_points[0], old_points[2]]
        );
        assert_eq!(markers.retained[0].moved_tracks, vec![tracks[0], tracks[2]]);
        assert_eq!(
            markers.visible_points(),
            vec![next_point, old_points[0], old_points[2]]
        );

        unsafe {
            core::ptr::write_volatile(&mut first.x, 10.0);
            core::ptr::write_volatile(&mut last.x, 30.0);
        }
        let old_order = &mut markers.retained[0];
        assert!(!old_order.arrived(start + Duration::from_millis(1250)));
        assert!(!old_order.arrived(start + Duration::from_millis(1500)));
        assert!(old_order.arrived(start + Duration::from_millis(3250)));
        markers.retained.clear();
        assert_eq!(markers.visible_points(), vec![next_point]);
    }

    #[test]
    fn rapid_move_clicks_replace_the_previous_orders_arrows() {
        let facet_vtable = 0x1234usize;
        let soldiers = [Vec3::default(), Vec3::default()];
        let tracks: Vec<_> = soldiers
            .iter()
            .map(|position| PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: position as *const Vec3 as usize,
            })
            .collect();
        let first = [
            Vec3 {
                x: 10.0,
                ..Vec3::default()
            },
            Vec3 {
                x: 12.0,
                ..Vec3::default()
            },
        ];
        let second = [
            Vec3 {
                x: 20.0,
                ..Vec3::default()
            },
            Vec3 {
                x: 22.0,
                ..Vec3::default()
            },
        ];
        let start = Instant::now();
        let mut markers = Markers {
            preview_points: first.into(),
            preview_tracks: tracks.clone(),
            preview_at: Some(start),
            ..Markers::default()
        };
        markers.next_move_point_starts_order = true;
        markers.record_move_point(first[0], start, 1, CursorSnapshot::default());
        markers.record_move_point(
            first[1],
            start + Duration::from_millis(1),
            1,
            CursorSnapshot::default(),
        );
        let first_id = markers.order_id;
        assert_eq!(markers.moved, first);

        markers.preview_points = second.into();
        markers.preview_at = Some(start + Duration::from_millis(100));
        markers.next_move_point_starts_order = true;
        markers.record_move_point(
            second[0],
            start + Duration::from_millis(100),
            1,
            CursorSnapshot::default(),
        );
        markers.record_move_point(
            second[1],
            start + Duration::from_millis(101),
            1,
            CursorSnapshot::default(),
        );
        assert_ne!(markers.order_id, first_id);
        assert_eq!(markers.moved, second);
        assert!(markers.retained.is_empty());
        assert_eq!(markers.visible_points(), second);
    }

    #[test]
    fn stop_retires_only_the_receiving_soldiers_arrows() {
        let tracks = [
            PositionTrack {
                facet: 1,
                vtable: 1,
                position: 1,
            },
            PositionTrack {
                facet: 2,
                vtable: 2,
                position: 2,
            },
            PositionTrack {
                facet: 3,
                vtable: 3,
                position: 3,
            },
        ];
        let points = [
            Vec3::default(),
            Vec3 {
                x: 10.0,
                ..Vec3::default()
            },
            Vec3 {
                x: 20.0,
                ..Vec3::default()
            },
        ];
        let other_order = Markers {
            ordered_preview_points: vec![points[2]],
            moved: vec![points[2]],
            moved_tracks: vec![tracks[2]],
            ..Markers::default()
        };
        let mut markers = Markers {
            ordered_preview_points: points[..2].to_vec(),
            moved: points[..2].to_vec(),
            moved_tracks: tracks[..2].to_vec(),
            retained: vec![Box::new(other_order)],
            ..Markers::default()
        };
        assert_eq!(markers.retire_tracks(&[tracks[0]]), 1);
        assert_eq!(markers.visible_points(), vec![points[1], points[2]]);
        assert_eq!(markers.retire_tracks(&[tracks[1]]), 1);
        assert_eq!(markers.visible_points(), vec![points[2]]);
        assert_eq!(markers.retire_tracks(&[tracks[2]]), 1);
        assert!(markers.visible_points().is_empty());
    }

    #[test]
    fn vehicle_stop_retires_only_the_stopped_vehicles_arrow() {
        let positions = [Vec3::default(), Vec3::default()];
        let tracks = positions
            .iter()
            .enumerate()
            .map(|(index, position)| PositionTrack {
                facet: index + 1,
                vtable: index + 1,
                position: position as *const Vec3 as usize,
            })
            .collect::<Vec<_>>();
        let points = vec![
            Vec3 {
                x: 10.0,
                ..Vec3::default()
            },
            Vec3 {
                x: 20.0,
                ..Vec3::default()
            },
        ];
        let mut markers = Markers::default();
        markers.start_vehicle_order(
            Preview {
                points: points.clone(),
                tracks: tracks.clone(),
            },
            Instant::now(),
        );
        assert_eq!(markers.retire_tracks(&[tracks[0]]), 1);
        assert_eq!(markers.visible_points(), vec![points[1]]);
        assert_eq!(markers.retire_tracks(&[tracks[1]]), 1);
        assert!(markers.visible_points().is_empty());
    }

    #[test]
    fn whole_formation_matching_replaces_the_correct_soldier() {
        let tracks = [
            PositionTrack {
                facet: 1,
                vtable: 1,
                position: 1,
            },
            PositionTrack {
                facet: 2,
                vtable: 2,
                position: 2,
            },
        ];
        let markers = Markers {
            ordered_preview_points: vec![
                Vec3::default(),
                Vec3 {
                    x: 5.0,
                    ..Vec3::default()
                },
            ],
            moved: vec![
                Vec3 {
                    x: 4.0,
                    ..Vec3::default()
                },
                Vec3 {
                    x: 6.0,
                    ..Vec3::default()
                },
            ],
            moved_tracks: tracks.into(),
            ..Markers::default()
        };
        assert_eq!(
            markers.match_owners(),
            vec![Some(tracks[0]), Some(tracks[1])]
        );
    }

    #[test]
    fn cover_shift_still_replaces_a_soldiers_old_arrow() {
        let tracks = [
            PositionTrack {
                facet: 1,
                vtable: 1,
                position: 1,
            },
            PositionTrack {
                facet: 2,
                vtable: 2,
                position: 2,
            },
        ];
        let mut markers = Markers {
            ordered_preview_points: vec![
                Vec3::default(),
                Vec3 {
                    x: 10.0,
                    ..Vec3::default()
                },
            ],
            moved: vec![
                Vec3 {
                    x: 100.0,
                    ..Vec3::default()
                },
                Vec3 {
                    x: 110.0,
                    ..Vec3::default()
                },
            ],
            moved_tracks: tracks.into(),
            ..Markers::default()
        };
        markers.retain_unordered(&[tracks[0]]);
        assert_eq!(markers.moved.len(), 1);
        assert_eq!(markers.moved[0].x, 110.0);
        assert_eq!(markers.moved_tracks, vec![tracks[1]]);
    }

    #[test]
    fn third_order_removes_the_first_orders_superseded_arrow() {
        let facet_vtable = 0x1234usize;
        let first = Vec3::default();
        let second = Vec3::default();
        let tracks: Vec<_> = [&first, &second]
            .into_iter()
            .map(|position| PositionTrack {
                facet: &facet_vtable as *const usize as usize,
                vtable: facet_vtable,
                position: position as *const Vec3 as usize,
            })
            .collect();
        let point = |x| Vec3 {
            x,
            ..Vec3::default()
        };
        let start = Instant::now();
        let mut markers = Markers {
            preview_points: vec![point(30.0)],
            preview_tracks: vec![tracks[1]],
            ordered_preview_points: vec![point(10.0), point(20.0)],
            moved: vec![point(10.0), point(20.0)],
            moved_tracks: tracks.clone(),
            moved_at: Some(start),
            ..Markers::default()
        };
        markers.start_order(
            vec![tracks[1]],
            start + Duration::from_secs(1),
            1,
            CursorSnapshot::default(),
        );
        markers.moved.push(point(30.0));
        assert_eq!(markers.retained[0].moved, vec![point(10.0)]);
        markers.preview_points = vec![point(40.0)];
        markers.preview_tracks = vec![tracks[0]];
        markers.start_order(
            vec![tracks[0]],
            start + Duration::from_secs(2),
            1,
            CursorSnapshot::default(),
        );
        markers.moved.push(point(40.0));
        assert_eq!(markers.retained.len(), 1);
        assert_eq!(markers.retained[0].moved, vec![point(30.0)]);
        assert_eq!(markers.visible_points(), vec![point(40.0), point(30.0)]);
    }

    #[test]
    fn known_soldier_settling_at_cover_clears_an_offset_marker() {
        let facet_vtable = 0x1234usize;
        let mut soldier = Vec3 {
            x: 20.0,
            ..Vec3::default()
        };
        let track = PositionTrack {
            facet: &facet_vtable as *const usize as usize,
            vtable: facet_vtable,
            position: &soldier as *const Vec3 as usize,
        };
        let start = Instant::now();
        let mut markers = Markers {
            moved: vec![Vec3::default()],
            moved_tracks: vec![track],
            owners: vec![Some(track)],
            starting_positions: vec![Some(soldier)],
            moved_at: Some(start),
            ..Markers::default()
        };
        assert!(!markers.arrived(start + Duration::from_secs(1)));
        unsafe { core::ptr::write_volatile(&mut soldier.x, 10.0) };
        assert!(!markers.arrived(start + Duration::from_millis(1250)));
        assert!(!markers.arrived(start + Duration::from_millis(1500)));
        assert!(!markers.arrived(start + Duration::from_secs(4)));
        assert!(markers.arrived(start + Duration::from_secs(6)));
    }
}
