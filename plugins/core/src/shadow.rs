//! Redraw the shadow map's cascades in turn.
//!
//! `SceneViewImpl`'s render (`world2.dll`, GOG 2026-09-14 `fn_18d7a0`) calls
//! its shadow pass once a frame (`fn_192620`, from `0x18d9e1`; [`SITE_PATTERN`],
//! [`PASS_PATTERN`]). The pass renders the shadow cascades (their count at
//! `+0xf40`) into one shadow map (`+0x498`): it culls and draws the casters
//! (`fn_1916b0`, most of its time) and writes the cascades' matrices and
//! splits, which lit surfaces read, into the `ShadowDataBuffer` (`+0x408`).
//!
//! `[loader] shadow_cascades` spreads that work over frames ([`Cascades`];
//! `far_half` by default, `all` leaves the pass alone): the map is a texture
//! array, one layer per cascade (created with the cascade count as its array
//! size, `fn_193050`), and the pass redraws only the layers due this frame.
//! Three facts make that work:
//!
//! - The caster pass's loop over cascades (`fn_1916b0`) culls each cascade's
//!   casters and widens each caster's cascade range (`vt+0xa0`/`vt+0xa8` with
//!   `min`/`max` of the cascade index, [`RANGE_PATTERN`]); the pass then draws
//!   each caster into its range only. A cascade the loop skips gets no draws.
//! - The loop writes each cascade's matrix for lit surfaces into the view
//!   (`+0x7a8 + 0x40·i`), which the pass copies into the `ShadowDataBuffer`, so
//!   a skipped cascade keeps the matrix its layer was drawn with.
//! - The pass clears the map with `DeviceImpl::clearDepthStencilView`
//!   (`fn_e7a10`), whose last argument selects one layer, or all of them when
//!   negative ([`CLEAR_PATTERN`] passes -1). A layer's clear needs a depth
//!   view of that layer, which the game never makes for the shadow map (only
//!   the whole-array one), and the engine's own per-layer creator cannot make
//!   several: it resizes its list to the layer index, then writes one past
//!   it, which crashed on the empty list (2026-09-25; the per-layer getter
//!   crashed the same way the first time). So Core makes its own one-layer
//!   views of the map's D3D11 texture ([`layer_views_ready`]) and clears them
//!   through the game's own immediate context ([`clear_layers`]); when that
//!   cannot be set up, the frame draws every cascade instead.
//!
//! So a stub at the loop's head ([`HEAD_PATTERN`], [`cascade_due`]) jumps a
//! cascade that is not due to the loop's test ([`LATCH_PATTERN`]), and a stub
//! at the clear ([`clear_layers`]) clears only the due layers. A view's first
//! frame, a new or resized map, a changed cascade count and the first frame
//! after shadows come back on draw every cascade, so no layer is left stale.
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// `mov rcx, rsi; call <main-view culling>; mov rcx, rsi; call <shadow pass>;
/// cmp byte ptr [rsi + 0x438], 0`, in the scene render.
pub const SITE_PATTERN: &str =
    "48 8b ce e8 ?? ?? ?? ?? 48 8b ce e8 ?? ?? ?? ?? 80 be 38 04 00 00 00";
/// Offset of the shadow pass's call in [`SITE_PATTERN`].
const SITE_CALL_AT: usize = 11;
/// The shadow pass's prologue and its first tests: shadows on (`+0x443`) and a
/// shadow camera (`[+0x90]` vt+0xa8, a holder whose first word is the camera).
pub const PASS_PATTERN: &str = "48 8b c4 48 89 58 10 48 89 70 18 48 89 78 20 55 41 54 41 55 41 56 \
     41 57 48 8d 68 a1 48 81 ec 00 01 00 00 0f 29 70 c8 0f 29 78 b8 44 0f 29 40 a8 \
     44 0f 29 48 98 44 0f 29 50 88 44 0f 29 98 78 ff ff ff 44 0f 29 a0 68 ff ff ff \
     48 8b f1 80 b9 43 04 00 00 00 0f 84 ?? ?? ?? ?? \
     48 8b 89 90 00 00 00 48 8b 01 ff 90 a8 00 00 00 4c 8b 38 4d 85 ff 0f 84 ?? ?? ?? ??";
/// Offset in the pass of its call to the caster pass (`fn_1916b0`).
const PASS_CASTERS_CALL_AT: usize = 0x145;
/// The caster pass's size, within which the loop's sites lie.
const CASTERS_SIZE: usize = 0xe34;

/// The head of the caster pass's loop over cascades (`fn_1916b0`, GOG
/// 2026-09-14 `0x1919da`): `lea rbx, [rsi + 0x330]; mov ecx, 5; lea rax,
/// [rbp + 0x1a0]`, with the cascade index in `r15d`, the view in `rsi`.
pub const HEAD_PATTERN: &str = "48 8d 9e 30 03 00 00 b9 05 00 00 00 48 8d 85 a0 01 00 00";
/// The loop's test (`0x19227d`): `mov r15d, [rbp + 0x4c0]` (the next index),
/// `cmp r15d, [rsi + 0xf40]` (the count), `jb` to [`HEAD_PATTERN`].
pub const LATCH_PATTERN: &str = "44 8b bd c0 04 00 00 44 3b be 40 0f 00 00 0f 82 ?? ?? ?? ??";
/// Offset of the `jb` in [`LATCH_PATTERN`].
const LATCH_JUMP_AT: usize = 14;
/// Each visible caster's cascade range: `vt+0xa0(min(vt+0xb0(), i))`, then
/// `vt+0xa8(max(vt+0xb8(), i))`, with `i` in `r15d`.
pub const RANGE_PATTERN: &str =
    "48 8b 3e 48 8b 07 48 8b 98 a0 00 00 00 48 8b cf ff 90 b0 00 00 00 \
     41 8b d7 41 3b c7 0f 42 d0 48 8b cf ff d3 48 8b 07 48 8b 98 a8 00 00 00 48 8b cf \
     ff 90 b8 00 00 00 41 8b d7 44 3b f8 0f 42 d0 48 8b cf ff d3";
/// The pass's clear: `device->vt+0x148(map, 3, 1.0, 0, -1)` (`0x192a2d`), the
/// depth a rip-relative float at [`CLEAR_DEPTH_AT`].
pub const CLEAR_PATTERN: &str =
    "48 8b 8e 98 00 00 00 48 8b 01 c7 44 24 28 ff ff ff ff c6 44 24 20 00 \
     f3 0f 10 1d ?? ?? ?? ?? 41 b8 03 00 00 00 48 8b 96 98 04 00 00 ff 90 48 01 00 00";
/// Offset of [`CLEAR_PATTERN`] in the pass, of its `movss xmm3` and its end.
const PASS_CLEAR_AT: usize = 0x40d;
const CLEAR_DEPTH_AT: usize = 0x17;
const CLEAR_SIZE: usize = 0x32;
/// The loop's slot for the next cascade index, relative to its `rbp`.
const LOOP_NEXT_INDEX: usize = 0x4c0;
const VIEW_CASCADE_COUNT: usize = 0xf40;
const DEVICE_CLEAR_DEPTH: usize = 0x148;

/// In `DeviceImpl::clearDepthStencilView` (`fn_e7a10`, the device's
/// vt+0x148): its D3D11 immediate context is `[+0xb8]`, whose
/// `ClearDepthStencilView` (vt+0x1a8) it calls. [`DEVICE_CONTEXT_AT`] bytes
/// into the function.
pub const DEVICE_CONTEXT_PATTERN: &str = "48 8b 8f b8 00 00 00 48 8b d0 0f b6 84 24 c0 00 00 00 \
     0f 28 de 44 8b c3 88 44 24 20 4c 8b 09 41 ff 91 a8 01 00 00";
const DEVICE_CONTEXT_AT: usize = 0xb5;
/// In `Texture::createDepthStencilView` (`fn_1c76e0`, the texture's vt+0x98):
/// its D3D11 texture is `[+0x80]`, passed to `CreateDepthStencilView`.
/// [`TEXTURE_RESOURCE_AT`] bytes into the function.
pub const TEXTURE_RESOURCE_PATTERN: &str =
    "48 8b 4e 40 48 8b 01 ff 50 28 4c 8b f0 48 8b 08 48 8b 41 50 \
     48 89 85 30 02 00 00 49 8b 0f 4a 8d 3c e1 48 8b 0f 48 85 c9 74 10 48 89 1f 48 8b 11 ff 52 10 \
     48 8b 85 30 02 00 00 4c 8b cf 4c 8d 44 24 30 48 8b 96 80 00 00 00 49 8b ce ff d0";
const TEXTURE_RESOURCE_AT: usize = 0x218;
const DEVICE_IMPL_CONTEXT: usize = 0xb8;
const TEXTURE_RESOURCE: usize = 0x80;
const TEXTURE_CREATE_DEPTH_VIEW: usize = 0x98;
/// The two functions above, found at install: a view's device and shadow map
/// are trusted only when their vtables hold them.
static DEVICE_CLEAR_FN: AtomicUsize = AtomicUsize::new(0);
static TEXTURE_DEPTH_VIEW_FN: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
const IID_TEXTURE2D: Guid = Guid(
    0x6f15_aaf2,
    0xd208,
    0x4e89,
    [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
);
const IID_DEVICE_CONTEXT: Guid = Guid(
    0xc0bf_a96c,
    0xe089,
    0x44fb,
    [0x8e, 0xaf, 0x26, 0xf8, 0x79, 0x61, 0x90, 0xda],
);

/// COM vtable slots used: IUnknown, `ID3D11DeviceChild::GetDevice`,
/// `ID3D11Texture2D::GetDesc`, `ID3D11Device::CreateDepthStencilView`,
/// `ID3D11DeviceContext::ClearDepthStencilView`.
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
const GET_DEVICE: usize = 3;
const TEXTURE_GET_DESC: usize = 10;
const DEVICE_CREATE_DEPTH_VIEW: usize = 10;
const CONTEXT_CLEAR_DEPTH_VIEW: usize = 53;
const BIND_DEPTH_STENCIL: u32 = 0x40;
const DSV_DIMENSION_TEXTURE2DARRAY: u32 = 4;
const CLEAR_DEPTH_AND_STENCIL: u32 = 3;

/// `D3D11_TEXTURE2D_DESC`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Texture2dDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc: u32,
}

/// `D3D11_DEPTH_STENCIL_VIEW_DESC` for a `Texture2DArray`.
#[repr(C)]
struct DepthViewDesc {
    format: u32,
    dimension: u32,
    flags: u32,
    mip_slice: u32,
    first_slice: u32,
    array_size: u32,
}

/// The depth format to view a depth texture's format with.
fn depth_view_format(format: u32) -> Option<u32> {
    match format {
        39 | 40 => Some(40), // R32_TYPELESS, D32_FLOAT
        44 | 45 => Some(45), // R24G8_TYPELESS, D24_UNORM_S8_UINT
        53 | 55 => Some(55), // R16_TYPELESS, D16_UNORM
        19 | 20 => Some(20), // R32G8X24_TYPELESS, D32_FLOAT_S8X24_UINT
        _ => None,
    }
}

/// Slot `slot` of the COM object `object`'s vtable.
unsafe fn com<F: Copy>(object: *mut c_void, slot: usize) -> F {
    unsafe { method(object, slot * 8) }
}

unsafe fn release(object: *mut c_void) {
    let release: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { com(object, RELEASE) };
    unsafe { release(object) };
}

/// Whether COM says `object` implements `iid`.
unsafe fn implements(object: *mut c_void, iid: &Guid) -> bool {
    let query: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32 =
        unsafe { com(object, QUERY_INTERFACE) };
    let mut out = core::ptr::null_mut();
    let ok = unsafe { query(object, iid, &mut out) } >= 0 && !out.is_null();
    if !out.is_null() {
        unsafe { release(out) };
    }
    ok
}

/// The address in slot `offset` of the vtable of the object at `object`.
unsafe fn slot_address(object: *mut c_void, offset: usize) -> usize {
    let vtable = unsafe { *(object as *const *const usize) };
    if vtable.is_null() {
        return 0;
    }
    unsafe { *vtable.add(offset / 8) }
}

/// The D3D11 immediate context and the shadow map's D3D11 texture of `view`:
/// only when its device and map are the game's own classes (their vtables
/// hold the functions the patterns found, which vouch for the fields) and
/// COM confirms both interfaces.
unsafe fn d3d(view: *mut c_void) -> Option<(*mut c_void, *mut c_void)> {
    let field = |object: *mut c_void, offset: usize| unsafe {
        *((object as *const u8).add(offset) as *const *mut c_void)
    };
    let (device, map) = (field(view, VIEW_DEVICE), field(view, VIEW_SHADOW_MAP));
    if device.is_null() || map.is_null() {
        return None;
    }
    let (clear_fn, view_fn) = (
        DEVICE_CLEAR_FN.load(Ordering::Relaxed),
        TEXTURE_DEPTH_VIEW_FN.load(Ordering::Relaxed),
    );
    if clear_fn == 0
        || view_fn == 0
        || unsafe { slot_address(device, DEVICE_CLEAR_DEPTH) } != clear_fn
        || unsafe { slot_address(map, TEXTURE_CREATE_DEPTH_VIEW) } != view_fn
    {
        return None;
    }
    let (context, texture) = (
        field(device, DEVICE_IMPL_CONTEXT),
        field(map, TEXTURE_RESOURCE),
    );
    if context.is_null()
        || texture.is_null()
        || !unsafe { implements(context, &IID_DEVICE_CONTEXT) }
        || !unsafe { implements(texture, &IID_TEXTURE2D) }
    {
        return None;
    }
    Some((context, texture))
}

/// Core's one-layer depth views of one shadow map texture. Each view holds a
/// reference to the texture, so its address cannot be reused while cached.
struct LayerViews {
    texture: usize,
    views: Vec<usize>,
}

impl Drop for LayerViews {
    fn drop(&mut self) {
        for view in self.views.drain(..) {
            unsafe { release(view as *mut c_void) };
        }
    }
}

thread_local! {
    /// The most recent textures first; a few views may be drawing at once.
    static LAYER_VIEWS: RefCell<Vec<LayerViews>> = const { RefCell::new(Vec::new()) };
}
const LAYER_VIEW_TEXTURES: usize = 4;

/// One-layer depth views of `texture`'s `count` layers, or `None`.
unsafe fn create_layer_views(texture: *mut c_void, count: u32) -> Option<Vec<usize>> {
    let get_desc: unsafe extern "system" fn(*mut c_void, *mut Texture2dDesc) =
        unsafe { com(texture, TEXTURE_GET_DESC) };
    let mut desc = Texture2dDesc::default();
    unsafe { get_desc(texture, &mut desc) };
    let format = depth_view_format(desc.format)?;
    if desc.array_size != count
        || desc.sample_count != 1
        || desc.bind_flags & BIND_DEPTH_STENCIL == 0
    {
        return None;
    }
    let get_device: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) =
        unsafe { com(texture, GET_DEVICE) };
    let mut device = core::ptr::null_mut();
    unsafe { get_device(texture, &mut device) };
    if device.is_null() {
        return None;
    }
    let create: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const DepthViewDesc,
        *mut *mut c_void,
    ) -> i32 = unsafe { com(device, DEVICE_CREATE_DEPTH_VIEW) };
    let mut views = Vec::new();
    for layer in 0..count {
        let desc = DepthViewDesc {
            format,
            dimension: DSV_DIMENSION_TEXTURE2DARRAY,
            flags: 0,
            mip_slice: 0,
            first_slice: layer,
            array_size: 1,
        };
        let mut view = core::ptr::null_mut();
        if unsafe { create(device, texture, &desc, &mut view) } < 0 || view.is_null() {
            break;
        }
        views.push(view as usize);
    }
    unsafe { release(device) };
    if views.len() != count as usize {
        for view in views {
            unsafe { release(view as *mut c_void) };
        }
        return None;
    }
    Some(views)
}

/// Whether `view`'s shadow map has Core's one-layer depth views for its
/// `count` layers, creating them for a texture not seen before.
unsafe fn layer_views_ready(view: *mut c_void, count: u32) -> bool {
    if count == 0 || count > 32 {
        return false;
    }
    let Some((_, texture)) = (unsafe { d3d(view) }) else {
        return false;
    };
    LAYER_VIEWS.with(|cache| {
        let Ok(mut cache) = cache.try_borrow_mut() else {
            return false;
        };
        if let Some(at) = cache.iter().position(|v| v.texture == texture as usize) {
            if cache[at].views.len() == count as usize {
                let entry = cache.remove(at);
                cache.insert(0, entry);
                return true;
            }
            cache.remove(at);
        }
        match unsafe { create_layer_views(texture, count) } {
            Some(views) => {
                cache.insert(
                    0,
                    LayerViews {
                        texture: texture as usize,
                        views,
                    },
                );
                cache.truncate(LAYER_VIEW_TEXTURES);
                true
            }
            None => false,
        }
    })
}

/// The scene view's fields the rotation reads: shadows on ([`PASS_PATTERN`]
/// tests it), the device and the shadow map ([`CLEAR_PATTERN`] passes both).
const VIEW_SHADOWS_ON: usize = 0x443;
const VIEW_DEVICE: usize = 0x98;
const VIEW_SHADOW_MAP: usize = 0x498;

/// Diagnostic, never shipped (feature `shadow-toggle`), kept privately.
#[cfg(feature = "shadow-toggle")]
#[path = "shadow_toggle.rs"]
mod toggle;

type Pass = unsafe extern "C" fn(*mut c_void);
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// `[loader] shadow_cascades`: which cascades a frame redraws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cascades {
    /// Every cascade every frame, as the game does.
    All,
    /// One cascade a frame, in turn.
    Rotate,
    /// The nearest cascade every frame, and one of the others in turn.
    Near,
    /// One cascade a frame, the farthest half as often as the others: the
    /// others twice in turn, then the farthest. The farthest covers the most
    /// casters and costs the most (8.8 ms against 2.0–4.3 ms for the others
    /// on a busy scene with four), so this evens the frames out.
    FarHalf,
}

impl Cascades {
    pub fn parse(text: &str) -> Cascades {
        match text {
            "rotate" => Cascades::Rotate,
            "near" => Cascades::Near,
            "far_half" => Cascades::FarHalf,
            _ => Cascades::All,
        }
    }

    /// The cascades (bits) frame `frame` of a view redraws, out of `count`.
    pub fn due(self, frame: u32, count: u32) -> u32 {
        let all = if count >= 32 {
            u32::MAX
        } else {
            (1u32 << count) - 1
        };
        match self {
            Cascades::All => all,
            _ if count <= 1 => all,
            Cascades::Rotate => 1 << (frame % count),
            Cascades::Near => 1 | 1 << (1 + frame % (count - 1)),
            Cascades::FarHalf => {
                let at = frame % (2 * count - 1);
                if at == 2 * count - 2 {
                    1 << (count - 1)
                } else {
                    1 << (at % (count - 1))
                }
            }
        }
    }
}

static ROTATION: AtomicU32 = AtomicU32::new(0);
/// The view whose pass is running with only some cascades due, or 0.
static PARTIAL_VIEW: AtomicUsize = AtomicUsize::new(0);
static PARTIAL_MASK: AtomicU32 = AtomicU32::new(0);
static CLEAR_DEPTH: AtomicU32 = AtomicU32::new(0x3f80_0000);
static ROTATING: AtomicBool = AtomicBool::new(false);
// Read by the stubs as plain qwords.
static HEAD_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static LATCH: AtomicUsize = AtomicUsize::new(0);
static CLEAR_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static CLEAR_RESUME: AtomicUsize = AtomicUsize::new(0);

/// What a view's shadow map looked like on its last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub map: usize,
    pub count: u32,
    pub on: bool,
}

/// Per-view rotation state, the most recent views first. A view beyond
/// [`Rotations::CAPACITY`] drops the least recent, which then draws every
/// cascade on its next frame as if new.
pub struct Rotations {
    seen: Vec<(usize, u32, Seen)>,
}

impl Rotations {
    const CAPACITY: usize = 8;

    pub const fn new() -> Self {
        Rotations { seen: Vec::new() }
    }

    /// The cascades `view` redraws this frame, given its map now; every one
    /// when the view is new, or its map, count or shadow switch changed.
    pub fn due(&mut self, view: usize, now: Seen, mode: Cascades) -> u32 {
        let previous = match self.seen.iter().position(|(v, _, _)| *v == view) {
            Some(at) => Some(self.seen.remove(at)),
            None => None,
        };
        let frame = previous.map_or(0, |(_, f, _)| f.wrapping_add(1));
        let unchanged = previous.is_some_and(|(_, _, last)| last == now && last.on);
        self.seen.insert(0, (view, frame, now));
        self.seen.truncate(Self::CAPACITY);
        if unchanged {
            mode.due(frame, now.count)
        } else {
            Cascades::All.due(0, now.count)
        }
    }
}

impl Default for Rotations {
    fn default() -> Self {
        Self::new()
    }
}

/// Called by [`head_stub`] for each cascade: whether the loop runs it.
unsafe extern "C" fn cascade_due(view: usize, index: u32) -> u8 {
    if view == 0 || PARTIAL_VIEW.load(Ordering::Relaxed) != view {
        return 1;
    }
    (index < 32 && PARTIAL_MASK.load(Ordering::Relaxed) >> index & 1 != 0) as u8
}

/// Called by [`clear_stub`] in place of the pass's clear: clears only the due
/// layers, with Core's layer views ([`layer_views_ready`] made them this
/// frame) on the game's own context, and answers 1; or answers 0 for the
/// pass's own clear of them all.
unsafe extern "C" fn clear_layers(view: *mut c_void) -> u8 {
    if view.is_null() || PARTIAL_VIEW.load(Ordering::Relaxed) != view as usize {
        return 0;
    }
    let Some((context, texture)) = (unsafe { d3d(view) }) else {
        return 0;
    };
    let clear: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, f32, u8) =
        unsafe { com(context, CONTEXT_CLEAR_DEPTH_VIEW) };
    let depth = f32::from_bits(CLEAR_DEPTH.load(Ordering::Relaxed));
    let mask = PARTIAL_MASK.load(Ordering::Relaxed);
    LAYER_VIEWS.with(|cache| {
        let Ok(cache) = cache.try_borrow() else {
            return 0;
        };
        let Some(entry) = cache.iter().find(|v| v.texture == texture as usize) else {
            return 0;
        };
        for (layer, view) in entry.views.iter().enumerate() {
            if layer < 32 && mask >> layer & 1 != 0 {
                unsafe {
                    clear(
                        context,
                        *view as *mut c_void,
                        CLEAR_DEPTH_AND_STENCIL,
                        depth,
                        0,
                    )
                };
            }
        }
        1
    })
}

/// At the cascade loop's head (in place of its first instruction): runs the
/// cascade when [`cascade_due`] says so, else stores the next index where the
/// loop keeps it and jumps to the loop's test. The volatile registers and
/// flags are dead here; `rsp` is 16-aligned in the loop.
#[unsafe(naked)]
unsafe extern "C" fn head_stub() {
    core::arch::naked_asm!(
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "mov edx, r15d",
        "call {due}",
        "add rsp, 0x20",
        "test al, al",
        "jnz 2f",
        "lea eax, [r15 + 1]",
        "mov dword ptr [rbp + {next}], eax",
        "jmp qword ptr [rip + {latch}]",
        "2:",
        "jmp qword ptr [rip + {head}]",
        due = sym cascade_due,
        next = const LOOP_NEXT_INDEX,
        latch = sym LATCH,
        head = sym HEAD_TRAMPOLINE,
    );
}

/// At the pass's clear (in place of its first instruction): [`clear_layers`]
/// clears the due layers and the pass resumes after its clear, or the pass
/// clears them all itself. Volatile registers and flags are dead here too.
#[unsafe(naked)]
unsafe extern "C" fn clear_stub() {
    core::arch::naked_asm!(
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "call {clear}",
        "add rsp, 0x20",
        "test al, al",
        "jnz 2f",
        "jmp qword ptr [rip + {original}]",
        "2:",
        "jmp qword ptr [rip + {resume}]",
        clear = sym clear_layers,
        original = sym CLEAR_TRAMPOLINE,
        resume = sym CLEAR_RESUME,
    );
}

thread_local! {
    static ROTATIONS: RefCell<Rotations> = const { RefCell::new(Rotations::new()) };
}

/// The function in slot `offset` of the vtable of the object at `object`.
unsafe fn method<F: Copy>(object: *mut c_void, offset: usize) -> F {
    let vtable = unsafe { *(object as *const *const usize) };
    let entry = unsafe { *vtable.add(offset / 8) };
    unsafe { core::mem::transmute_copy(&entry) }
}

/// The detour for the scene render's shadow-pass call.
unsafe extern "C" fn pass(view: *mut c_void) {
    let original: Pass = unsafe { core::mem::transmute(ORIGINAL.load(Ordering::Acquire)) };
    #[cfg(feature = "shadow-toggle")]
    if ROTATION.load(Ordering::Relaxed) != 0 {
        toggle::poll();
    }
    if !ROTATING.load(Ordering::Relaxed) {
        unsafe { original(view) };
        return;
    }
    let bytes = view as *const u8;
    let now = Seen {
        map: unsafe { *(bytes.add(VIEW_SHADOW_MAP) as *const usize) },
        count: unsafe { *(bytes.add(VIEW_CASCADE_COUNT) as *const u32) },
        on: unsafe { *bytes.add(VIEW_SHADOWS_ON) } != 0,
    };
    let mode = Cascades::parse(match ROTATION.load(Ordering::Relaxed) {
        1 => "rotate",
        2 => "near",
        3 => "far_half",
        _ => "all",
    });
    let mask = ROTATIONS.with(|r| match r.try_borrow_mut() {
        Ok(mut r) => r.due(view as usize, now, mode),
        Err(_) => Cascades::All.due(0, now.count),
    });
    let all = Cascades::All.due(0, now.count);
    if mask != all && unsafe { layer_views_ready(view, now.count) } {
        PARTIAL_MASK.store(mask, Ordering::Relaxed);
        PARTIAL_VIEW.store(view as usize, Ordering::Relaxed);
    }
    unsafe { original(view) };
    PARTIAL_VIEW.store(0, Ordering::Relaxed);
}

fn find(api: &Api, base: *mut c_void, size: usize, pattern: &str) -> *mut u8 {
    let pattern = std::ffi::CString::new(pattern).unwrap_or_default();
    unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) as *mut u8 }
}

/// The target of the `e8 rel32` at `site`.
unsafe fn call_target(site: *const u8) -> usize {
    let rel = unsafe { core::ptr::read_unaligned(site.add(1) as *const i32) };
    (site as isize + 5 + rel as isize) as usize
}

/// The addresses the redirections need, each vouched for by its pattern and
/// its place in the pass or the caster pass.
struct Sites {
    call: *mut u8,
    pass: *mut u8,
    casters: usize,
}

fn redirect(api: &Api) -> Result<Sites, String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let site = find(api, base, size, SITE_PATTERN);
    let pass_fn = find(api, base, size, PASS_PATTERN);
    if site.is_null() || pass_fn.is_null() {
        return Err("the scene render's shadow pass was not found".into());
    }
    let call = unsafe { site.add(SITE_CALL_AT) };
    if unsafe { call_target(call) } != pass_fn as usize {
        return Err("the scene render calls a different shadow pass".into());
    }
    let casters = unsafe { call_target(pass_fn.add(PASS_CASTERS_CALL_AT)) };
    let mut original = core::ptr::null_mut();
    let detour = pass as unsafe extern "C" fn(_) as *mut c_void;
    if unsafe { (api.hook_call)(call as *mut c_void, detour, &mut original) } != 0
        || original.is_null()
    {
        return Err("its call could not be redirected".into());
    }
    ORIGINAL.store(original as usize, Ordering::Release);
    Ok(Sites {
        call,
        pass: pass_fn,
        casters,
    })
}

/// The loop-head and clear stubs for [`Cascades`], after [`redirect`].
fn rotate(api: &Api, sites: &Sites) -> Result<(), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    let size = unsafe { (api.module_size)(base) };
    let (head, latch, range, clear) = (
        find(api, base, size, HEAD_PATTERN) as usize,
        find(api, base, size, LATCH_PATTERN) as usize,
        find(api, base, size, RANGE_PATTERN) as usize,
        find(api, base, size, CLEAR_PATTERN) as usize,
    );
    let (context_at, resource_at) = (
        find(api, base, size, DEVICE_CONTEXT_PATTERN) as usize,
        find(api, base, size, TEXTURE_RESOURCE_PATTERN) as usize,
    );
    if context_at == 0 || resource_at == 0 {
        return Err("the device's context or the texture's resource was not found".into());
    }
    DEVICE_CLEAR_FN.store(context_at - DEVICE_CONTEXT_AT, Ordering::Relaxed);
    TEXTURE_DEPTH_VIEW_FN.store(resource_at - TEXTURE_RESOURCE_AT, Ordering::Relaxed);
    let inside = |at: usize| at > sites.casters && at < sites.casters + CASTERS_SIZE;
    let _ = sites.call;
    if !(inside(head) && inside(latch) && inside(range))
        || unsafe { call_target_jb((latch + LATCH_JUMP_AT) as *const u8) } != head
        || clear != sites.pass as usize + PASS_CLEAR_AT
    {
        return Err("the cascade loop or the clear is laid out differently".into());
    }
    let depth_at = clear + CLEAR_DEPTH_AT;
    let disp = unsafe { core::ptr::read_unaligned((depth_at + 4) as *const i32) };
    let depth = unsafe { *(((depth_at + 8) as isize + disp as isize) as *const f32) };
    CLEAR_DEPTH.store(depth.to_bits(), Ordering::Relaxed);
    LATCH.store(latch, Ordering::Release);
    CLEAR_RESUME.store(clear + CLEAR_SIZE, Ordering::Release);
    for (target, stub, trampoline) in [
        (
            head,
            head_stub as unsafe extern "C" fn() as *mut c_void,
            &HEAD_TRAMPOLINE,
        ),
        (
            clear,
            clear_stub as unsafe extern "C" fn() as *mut c_void,
            &CLEAR_TRAMPOLINE,
        ),
    ] {
        let mut original = core::ptr::null_mut();
        if unsafe { (api.hook)(target as *mut c_void, stub, &mut original) } != 0
            || original.is_null()
        {
            return Err("the cascade loop could not be hooked".into());
        }
        trampoline.store(original as usize, Ordering::Release);
    }
    Ok(())
}

/// The target of the `0f 82 rel32` (`jb`) at `site`.
unsafe fn call_target_jb(site: *const u8) -> usize {
    if unsafe { *site } != 0x0f || unsafe { *site.add(1) } != 0x82 {
        return 0;
    }
    let rel = unsafe { core::ptr::read_unaligned(site.add(2) as *const i32) };
    (site as isize + 6 + rel as isize) as usize
}

/// `cascades` is `shadow_cascades`; `all` leaves the pass alone.
pub fn install(api: &Api, cascades: &str) {
    let mode = Cascades::parse(cascades);
    if mode == Cascades::All {
        return;
    }
    let sites = match redirect(api) {
        Ok(sites) => sites,
        Err(e) => {
            super::say(
                api,
                LOG_WARN,
                &format!("shadow map: {e}; every cascade is redrawn each frame"),
            );
            return;
        }
    };
    match rotate(api, &sites) {
        Ok(()) => {
            let (code, what) = match mode {
                Cascades::Rotate => (1, "one cascade a frame, in turn"),
                Cascades::Near => (2, "the nearest cascade every frame, the others in turn"),
                _ => (3, "one cascade a frame, the farthest half as often"),
            };
            ROTATION.store(code, Ordering::Relaxed);
            ROTATING.store(true, Ordering::Release);
            #[cfg(feature = "shadow-toggle")]
            {
                let _ = toggle::LOG.set(api.log);
                super::say(
                    api,
                    LOG_INFO,
                    "shadow map: diagnostic build, F11 switches the rotation",
                );
            }
            super::say(api, LOG_INFO, &format!("shadow map: redrawing {what}"));
        }
        Err(e) => super::say(
            api,
            LOG_WARN,
            &format!("shadow map: {e}; every cascade is redrawn each frame"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascades_due_each_frame() {
        let frames = |mode: Cascades| (0..6).map(|f| mode.due(f, 4)).collect::<Vec<_>>();
        assert_eq!(frames(Cascades::All), [0xf; 6]);
        assert_eq!(frames(Cascades::Rotate), [1, 2, 4, 8, 1, 2]);
        assert_eq!(frames(Cascades::Near), [3, 5, 9, 3, 5, 9]);
        let far: Vec<u32> = (0..14).map(|f| Cascades::FarHalf.due(f, 4)).collect();
        assert_eq!(far, [1, 2, 4, 1, 2, 4, 8, 1, 2, 4, 1, 2, 4, 8]);
        assert_eq!(
            (0..6)
                .map(|f| Cascades::FarHalf.due(f, 2))
                .collect::<Vec<_>>(),
            [1, 1, 2, 1, 1, 2]
        );
        assert_eq!(Cascades::FarHalf.due(3, 1), 1);
        // A single cascade is always drawn.
        assert_eq!(Cascades::Rotate.due(3, 1), 1);
        assert_eq!(Cascades::Near.due(3, 1), 1);
    }

    #[test]
    fn a_new_or_changed_map_draws_every_cascade() {
        let map = Seen {
            map: 0x5000,
            count: 4,
            on: true,
        };
        let mut r = Rotations::new();
        assert_eq!(r.due(0x1000, map, Cascades::Rotate), 0xf); // new view
        assert_eq!(r.due(0x1000, map, Cascades::Rotate), 2);
        assert_eq!(r.due(0x1000, map, Cascades::Rotate), 4);
        let resized = Seen { map: 0x6000, ..map };
        assert_eq!(r.due(0x1000, resized, Cascades::Rotate), 0xf);
        let fewer = Seen {
            count: 3,
            ..resized
        };
        assert_eq!(r.due(0x1000, fewer, Cascades::Rotate), 0x7);
        // Shadows off, then on again: the first frame back draws them all.
        let off = Seen { on: false, ..fewer };
        r.due(0x1000, off, Cascades::Rotate);
        assert_eq!(r.due(0x1000, fewer, Cascades::Rotate), 0x7);
        assert_ne!(r.due(0x1000, fewer, Cascades::Rotate), 0x7);
    }

    #[test]
    fn rotates_each_view_separately() {
        let map = Seen {
            map: 0x5000,
            count: 4,
            on: true,
        };
        let mut r = Rotations::new();
        let (mut main, mut preview) = (Vec::new(), Vec::new());
        for _ in 0..3 {
            main.push(r.due(0x1000, map, Cascades::Rotate));
            preview.push(r.due(0x2000, Seen { map: 0x7000, ..map }, Cascades::Rotate));
        }
        assert_eq!(main, [0xf, 2, 4]);
        assert_eq!(preview, [0xf, 2, 4]);
    }

    /// A loop laid out as the caster pass's: view in `rsi`, index in `r15d`,
    /// the next index at `[rbp + 0x4c0]`, the count at `[rsi + 0xf40]`. Its
    /// head jumps to [`head_stub`]; the body marks `seen[i]` (`r12`).
    #[unsafe(naked)]
    unsafe extern "C" fn fake_loop(_view: *mut u8, _seen: *mut u32) {
        core::arch::naked_asm!(
            "push rbx", "push rsi", "push rdi", "push rbp", "push r12", "push r15",
            "sub rsp, 0x508",
            "lea rax, [rip + 3f]", "mov qword ptr [rip + {head}], rax",
            "lea rax, [rip + 4f]", "mov qword ptr [rip + {latch}], rax",
            "mov rsi, rcx", "mov r12, rdx", "mov rbp, rsp",
            "xor r15d, r15d",
            "mov dword ptr [rbp + 0x4c0], 0",
            "cmp dword ptr [rsi + 0xf40], 0",
            "je 5f",
            "2:",
            "jmp {stub}",
            "3:",
            "mov dword ptr [r12 + r15 * 4], 1",
            "lea eax, [r15 + 1]",
            "mov dword ptr [rbp + 0x4c0], eax",
            "4:",
            "mov r15d, dword ptr [rbp + 0x4c0]",
            "cmp r15d, dword ptr [rsi + 0xf40]",
            "jb 2b",
            "5:",
            "add rsp, 0x508",
            "pop r15", "pop r12", "pop rbp", "pop rdi", "pop rsi", "pop rbx",
            "ret",
            head = sym HEAD_TRAMPOLINE,
            latch = sym LATCH,
            stub = sym head_stub,
        );
    }

    /// The pass around its clear: view in `rsi`, then [`clear_stub`]; the
    /// pass's own clear (the trampoline) records -1 in `cleared[0]`.
    #[unsafe(naked)]
    unsafe extern "C" fn fake_clear(_view: *mut u8, _cleared: *mut i32) {
        core::arch::naked_asm!(
            "push rsi", "push r12", "push rbx",
            "sub rsp, 0x20",
            "lea rax, [rip + 3f]", "mov qword ptr [rip + {own}], rax",
            "lea rax, [rip + 4f]", "mov qword ptr [rip + {resume}], rax",
            "mov rsi, rcx", "mov r12, rdx",
            "jmp {stub}",
            "3:",
            "mov dword ptr [r12], -1",
            "4:",
            "add rsp, 0x20",
            "pop rbx", "pop r12", "pop rsi",
            "ret",
            own = sym CLEAR_TRAMPOLINE,
            resume = sym CLEAR_RESUME,
            stub = sym clear_stub,
        );
    }

    // Fake COM objects: one vtable of `extern "system"` functions each; the
    // object's first word is its vtable.
    static CLEARED: std::sync::Mutex<Vec<(usize, u32, f32, u8)>> =
        std::sync::Mutex::new(Vec::new());
    static RELEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static CREATED: std::sync::Mutex<Vec<(u32, u32, u32, u32)>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "system" fn fake_query(
        o: *mut c_void,
        _: *const Guid,
        out: *mut *mut c_void,
    ) -> i32 {
        unsafe { *out = o };
        0
    }
    unsafe extern "system" fn fake_release(_: *mut c_void) -> u32 {
        RELEASED.fetch_add(1, Ordering::Relaxed);
        0
    }
    unsafe extern "system" fn fake_clear_view(
        _: *mut c_void,
        view: *mut c_void,
        flags: u32,
        depth: f32,
        stencil: u8,
    ) {
        CLEARED
            .lock()
            .unwrap()
            .push((view as usize, flags, depth, stencil));
    }
    static FAKE_DEVICE: std::sync::OnceLock<Box<[usize; 1]>> = std::sync::OnceLock::new();
    unsafe extern "system" fn fake_get_device(_: *mut c_void, out: *mut *mut c_void) {
        unsafe { *out = FAKE_DEVICE.get().unwrap().as_ptr() as *mut c_void };
    }
    unsafe extern "system" fn fake_get_desc(_: *mut c_void, desc: *mut Texture2dDesc) {
        unsafe {
            *desc = Texture2dDesc {
                width: 2048,
                height: 2048,
                mip_levels: 1,
                array_size: 4,
                format: 39,
                sample_count: 1,
                bind_flags: 0x48,
                ..Default::default()
            }
        };
    }
    unsafe extern "system" fn fake_create_view(
        _: *mut c_void,
        _: *mut c_void,
        desc: *const DepthViewDesc,
        out: *mut *mut c_void,
    ) -> i32 {
        let d = unsafe { &*desc };
        CREATED
            .lock()
            .unwrap()
            .push((d.format, d.dimension, d.first_slice, d.array_size));
        // A view is an object whose vtable can Release it.
        let view = Box::leak(Box::new([
            VIEW_VT.get_or_init(|| vtable(&[])).as_ptr() as usize
        ]));
        MADE.lock().unwrap().push(view.as_ptr() as usize);
        unsafe { *out = view.as_mut_ptr() as *mut c_void };
        0
    }
    static VIEW_VT: std::sync::OnceLock<Box<[usize; 64]>> = std::sync::OnceLock::new();
    static MADE: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    fn vtable(entries: &[(usize, usize)]) -> Box<[usize; 64]> {
        let mut v = Box::new([0usize; 64]);
        v[QUERY_INTERFACE] = fake_query as *const () as usize;
        v[RELEASE] = fake_release as *const () as usize;
        for &(slot, f) in entries {
            v[slot] = f;
        }
        v
    }

    /// A scene view whose device, context, map and texture are fakes that
    /// record the clears and the views made.
    struct FakeScene {
        view: Vec<u8>,
        _parts: Vec<Box<dyn std::any::Any>>,
    }

    fn fake_scene() -> FakeScene {
        let texture_vt = vtable(&[
            (GET_DEVICE, fake_get_device as *const () as usize),
            (TEXTURE_GET_DESC, fake_get_desc as *const () as usize),
        ]);
        let device_vt = vtable(&[(
            DEVICE_CREATE_DEPTH_VIEW,
            fake_create_view as *const () as usize,
        )]);
        let context_vt = vtable(&[(
            CONTEXT_CLEAR_DEPTH_VIEW,
            fake_clear_view as *const () as usize,
        )]);
        let _ = FAKE_DEVICE.set(Box::new([device_vt.as_ptr() as usize]));
        let texture = Box::new([texture_vt.as_ptr() as usize]);
        let context = Box::new([context_vt.as_ptr() as usize]);
        // The game's classes: vtables whose slots hold the found functions.
        let mut impl_vt = Box::new([0usize; 64]);
        impl_vt[DEVICE_CLEAR_DEPTH / 8] = 0x1111;
        let mut map_vt = Box::new([0usize; 64]);
        map_vt[TEXTURE_CREATE_DEPTH_VIEW / 8] = 0x2222;
        DEVICE_CLEAR_FN.store(0x1111, Ordering::Relaxed);
        TEXTURE_DEPTH_VIEW_FN.store(0x2222, Ordering::Relaxed);
        let mut device = Box::new([0usize; 0x20]);
        device[0] = impl_vt.as_ptr() as usize;
        device[DEVICE_IMPL_CONTEXT / 8] = context.as_ptr() as usize;
        let mut map = Box::new([0usize; 0x20]);
        map[0] = map_vt.as_ptr() as usize;
        map[TEXTURE_RESOURCE / 8] = texture.as_ptr() as usize;
        let mut view = vec![0u8; 0x1000];
        view[VIEW_CASCADE_COUNT..VIEW_CASCADE_COUNT + 4].copy_from_slice(&4u32.to_le_bytes());
        view[VIEW_DEVICE..VIEW_DEVICE + 8]
            .copy_from_slice(&(device.as_ptr() as usize).to_le_bytes());
        view[VIEW_SHADOW_MAP..VIEW_SHADOW_MAP + 8]
            .copy_from_slice(&(map.as_ptr() as usize).to_le_bytes());
        let parts: Vec<Box<dyn std::any::Any>> = vec![
            texture_vt, device_vt, context_vt, texture, context, impl_vt, map_vt, device, map,
        ];
        FakeScene {
            view,
            _parts: parts,
        }
    }

    #[test]
    fn the_stubs_skip_cascades_and_clear_only_their_layers() {
        let mut scene = fake_scene();
        let v = scene.view.as_mut_ptr();

        // Not the partial view: every cascade runs; the pass clears them all.
        let mut seen = [0u32; 8];
        PARTIAL_VIEW.store(0, Ordering::Relaxed);
        unsafe { fake_loop(v, seen.as_mut_ptr()) };
        assert_eq!(seen[..4], [1, 1, 1, 1]);
        let mut own = [0i32; 1];
        unsafe { fake_clear(v, own.as_mut_ptr()) };
        assert_eq!(own[0], -1);

        // Layer views: one per layer, D32 of the typeless texture, made once.
        assert!(unsafe { layer_views_ready(v as *mut c_void, 4) });
        assert_eq!(
            *CREATED.lock().unwrap(),
            [(40, 4, 0, 1), (40, 4, 1, 1), (40, 4, 2, 1), (40, 4, 3, 1)]
        );
        assert!(unsafe { layer_views_ready(v as *mut c_void, 4) });
        assert_eq!(CREATED.lock().unwrap().len(), 4);
        // A count that does not match the texture's layers is refused.
        assert!(!unsafe { layer_views_ready(v as *mut c_void, 3) });

        // Cascades 0 and 2 due: only they run, and only their layers clear.
        assert!(unsafe { layer_views_ready(v as *mut c_void, 4) });
        let mut seen = [0u32; 8];
        CLEAR_DEPTH.store(1.0f32.to_bits(), Ordering::Relaxed);
        PARTIAL_MASK.store(0b0101, Ordering::Relaxed);
        PARTIAL_VIEW.store(v as usize, Ordering::Relaxed);
        unsafe { fake_loop(v, seen.as_mut_ptr()) };
        assert_eq!(seen[..4], [1, 0, 1, 0]);
        let mut own = [0i32; 1];
        CLEARED.lock().unwrap().clear();
        unsafe { fake_clear(v, own.as_mut_ptr()) };
        assert_eq!(own[0], 0);
        // The refused count dropped the first views; these are new ones.
        let made = MADE.lock().unwrap().clone();
        assert_eq!(made.len(), 8);
        assert_eq!(
            *CLEARED.lock().unwrap(),
            [(made[4], 3, 1.0, 0), (made[6], 3, 1.0, 0)]
        );

        // The last cascade alone: the loop still ends.
        let mut seen = [0u32; 8];
        PARTIAL_MASK.store(0b1000, Ordering::Relaxed);
        unsafe { fake_loop(v, seen.as_mut_ptr()) };
        assert_eq!(seen[..4], [0, 0, 0, 1]);
        PARTIAL_VIEW.store(0, Ordering::Relaxed);

        // Objects that are not the game's classes are never touched.
        DEVICE_CLEAR_FN.store(0x9999, Ordering::Relaxed);
        assert!(unsafe { d3d(v as *mut c_void) }.is_none());
        assert!(!unsafe { layer_views_ready(v as *mut c_void, 4) });
        LAYER_VIEWS.with(|cache| cache.borrow_mut().clear());
    }

    /// The patterns against the game's `world2.dll`, when it is available.
    #[test]
    fn the_patterns_find_the_pass_in_the_game() {
        let Some(dir) = std::env::var_os("DEFIANCE_GAME_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join("bin").join("world2.dll");
        let Ok(image) = defiance_core::pe::map_file(&path) else {
            return;
        };
        let find = |text: &str| {
            let pattern = defiance_core::pattern::parse(text).unwrap();
            defiance_core::scan::scan(&image, &pattern)
        };
        let (sites, passes) = (find(SITE_PATTERN), find(PASS_PATTERN));
        assert_eq!((sites.len(), passes.len()), (1, 1));
        let target = |call: usize| {
            assert_eq!(image[call], 0xe8);
            let rel = i32::from_le_bytes(image[call + 1..call + 5].try_into().unwrap());
            (call as isize + 5 + rel as isize) as usize
        };
        assert_eq!(target(sites[0] + SITE_CALL_AT), passes[0]);
        let casters = target(passes[0] + PASS_CASTERS_CALL_AT);
        // The cascade rotation's sites.
        let (heads, latches, ranges, clears) = (
            find(HEAD_PATTERN),
            find(LATCH_PATTERN),
            find(RANGE_PATTERN),
            find(CLEAR_PATTERN),
        );
        assert_eq!(
            (heads.len(), latches.len(), ranges.len(), clears.len()),
            (1, 1, 1, 1)
        );
        let inside = |at: usize| at > casters && at < casters + CASTERS_SIZE;
        assert!(inside(heads[0]) && inside(latches[0]) && inside(ranges[0]));
        let jb = latches[0] + LATCH_JUMP_AT;
        assert_eq!(image[jb..jb + 2], [0x0f, 0x82]);
        let rel = i32::from_le_bytes(image[jb + 2..jb + 6].try_into().unwrap());
        assert_eq!((jb as isize + 6 + rel as isize) as usize, heads[0]);
        assert_eq!(clears[0], passes[0] + PASS_CLEAR_AT);
        let depth_at = clears[0] + CLEAR_DEPTH_AT;
        assert_eq!(image[depth_at..depth_at + 4], [0xf3, 0x0f, 0x10, 0x1d]);
        let disp = i32::from_le_bytes(image[depth_at + 4..depth_at + 8].try_into().unwrap());
        let constant = (depth_at as isize + 8 + disp as isize) as usize;
        assert_eq!(
            f32::from_le_bytes(image[constant..constant + 4].try_into().unwrap()),
            1.0
        );
        // The device's clear and the texture's depth-view creator: each once,
        // at its offset into a function (an unwind entry starts there).
        let (contexts, resources) = (find(DEVICE_CONTEXT_PATTERN), find(TEXTURE_RESOURCE_PATTERN));
        assert_eq!((contexts.len(), resources.len()), (1, 1));
        // The functions' starts are 16-aligned, after int3 padding.
        for start in [
            contexts[0] - DEVICE_CONTEXT_AT,
            resources[0] - TEXTURE_RESOURCE_AT,
        ] {
            assert_eq!(start % 16, 0);
            assert_eq!(image[start - 1], 0xcc);
        }
    }
}
