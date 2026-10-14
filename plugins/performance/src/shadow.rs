//! Redraw the shadow map's cascades in turn.
//!
//! `SceneViewImpl`'s render (`world2.dll`, GOG 2026-09-14 `fn_18d7a0`) calls
//! its shadow pass once a frame (`fn_192620`, from `0x18d9e1`; [`Layout::site`],
//! [`Layout::pass`]). The pass renders the shadow cascades (their count at
//! `+0xf40`) into one shadow map (`+0x498`; 8 bytes on from GOG 2026-10-07,
//! [`LAYOUTS`]): it culls and draws the casters
//! (`fn_1916b0`, most of its time) and writes the cascades' matrices and
//! splits, which lit surfaces read, into the `ShadowDataBuffer` (`+0x408`;
//! `+0x410` from GOG 2026-10-07).
//!
//! `[defiance.performance] shadow_cascades` spreads that work over frames ([`Cascades`];
//! `far_half` by default, `all` leaves the pass alone): the map is a texture
//! array, one layer per cascade (created with the cascade count as its array
//! size, `fn_193050`), and the pass redraws only the layers due this frame.
//! Three facts make that work:
//!
//! - The caster pass's loop over cascades (`fn_1916b0`) culls each cascade's
//!   casters and widens each caster's cascade range (`vt+0xa0`/`vt+0xa8` with
//!   `min`/`max` of the cascade index, [`RANGE_PATTERN`]); the pass then draws
//!   each caster into its range only. A cascade the loop skips gets no draws.
//!   A caster found in cascades on both sides of a skipped one still spans
//!   it, and is drawn into its layer through the loop's fit for it
//!   ([`LOOP_FITS`], copied into the `ShadowDataBuffer` after the loop, GOG
//!   2026-09-25 `0x192305`), which the loop did not write this frame: a
//!   stack slot holding whatever an earlier call left there. That painted
//!   stray casters over held layers, most where the cascades overlap with the
//!   camera nearly overhead. The skip path writes a fit there that draws
//!   nothing ([`head_stub`]).
//! - Lit surfaces find their place in a layer through the view's matrices
//!   ([`Layout::matrices`]), which the pass copies into the `ShadowDataBuffer`
//!   after drawing the casters (GOG 2026-09-25 `0x192e30`). The loop writes a
//!   cascade's matrix only when it runs it, so a held layer keeps the matrix
//!   it was drawn with, and its shadows stay where it put them. The plugin
//!   keeps those matrices ([`Held`]) to tell whether a held layer still
//!   covers its band.
//! - The pass clears the map with `DeviceImpl::clearDepthStencilView`
//!   (`fn_e7a10`), whose last argument selects one layer, or all of them when
//!   negative ([`Layout::clear`] passes -1). A layer's clear needs a depth
//!   view of that layer, which the game never makes for the shadow map (only
//!   the whole-array one), and the engine's own per-layer creator cannot make
//!   several: it resizes its list to the layer index, then writes one past
//!   it, which crashed on the empty list (2026-09-25; the per-layer getter
//!   crashed the same way the first time). So the plugin makes its own one-layer
//!   views of the map's D3D11 texture ([`layer_views_ready`]) and clears them
//!   through the game's own immediate context ([`clear_layers`]); when that
//!   cannot be set up, the frame draws every cascade instead.
//!
//! So a stub at the loop's head ([`Layout::head`], [`cascade_due`]) jumps a
//! cascade that is not due to the loop's test ([`Layout::latch`]), and a stub
//! at the clear ([`clear_layers`]) clears only the due layers. A view's first
//! frame, a new or resized map, a changed cascade count and the first frame
//! after shadows come back on draw every cascade, so no layer is left stale.
//!
//! A layer that is not due still has to cover the camera's band for it. Its
//! map holds only the casters the loop culled into the band it was drawn
//! for, though the map itself may reach further, so where the band has moved
//! since, casters are missing until the layer is redrawn (whole buildings'
//! shadows blinking). With the camera nearly overhead the splits cross the
//! ground at a shallow angle, and a small pitch or pan sweeps them far. So
//! each frame the plugin asks the camera for every band as the caster pass
//! does ([`splits`]; the view's camera holder at `+0x90`, `vt+0x60`, then the
//! camera's `vt+0x100`), and draws the layer again unless the band's slice of
//! the camera frustum lies within its slice when the layer was drawn
//! ([`contains`], [`stale`], [`Coverage`]). The loop culls against the slice's
//! planes (`fn_fe210`), not its box: near overhead a pitch reshapes the slice
//! inside an unchanged box. In practice a layer is held only while its band
//! stands still.
use crate::plan::{self, Kind, Plan, Write};
use core::cell::RefCell;
use core::ffi::c_void;
use defiance_api::{Api, LOG_INFO, LOG_WARN};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Diagnostic, never shipped: measure the stock pass without changing its cascades.
#[cfg(feature = "shadow-timing")]
mod timing;

/// The patterns and the scene view's fields that differ between the
/// `SceneViewImpl` layouts. GOG 2026-10-07 moved the view's fields from about
/// `+0xf8` on by 8 (the mesh-sort flag at `+0x451` by 9); the code around them
/// is unchanged. [`LAYOUTS`] holds both; [`redirect`] picks the one whose site
/// and pass both match.
pub struct Layout {
    /// `mov rcx, rsi; call <main-view culling>; mov rcx, rsi; call <shadow
    /// pass>; cmp byte ptr [rsi + 0x438], 0`, in the scene render.
    pub site: &'static str,
    /// The shadow pass's prologue and its first tests: shadows on
    /// ([`Layout::shadows_on`]) and a shadow camera (`[+0x90]` vt+0xa8, a
    /// holder whose first word is the camera).
    pub pass: &'static str,
    /// The head of the caster pass's loop over cascades (`fn_1916b0`, GOG
    /// 2026-09-14 `0x1919da`): `lea rbx, [rsi + 0x330]; mov ecx, 5; lea rax,
    /// [rbp + 0x1a0]`, with the cascade index in `r15d`, the view in `rsi`.
    pub head: &'static str,
    /// The loop's test (`0x19227d`): `mov r15d, [rbp + 0x4c0]` (the next
    /// index), `cmp r15d, [rsi + 0xf40]` (the count), `jb` to
    /// [`Layout::head`].
    pub latch: &'static str,
    /// The pass's clear: `device->vt+0x148(map, 3, 1.0, 0, -1)` (`0x192a2d`),
    /// the depth a rip-relative float at [`CLEAR_DEPTH_AT`].
    pub clear: &'static str,
    /// The view's shadows-on byte, which [`Layout::pass`] tests.
    pub shadows_on: usize,
    /// The view's shadow map, which [`Layout::clear`] passes.
    pub shadow_map: usize,
    /// The view's cascade count, which [`Layout::latch`] compares.
    pub cascade_count: usize,
    /// The camera's near and far planes (floats) and the shadow distance (an
    /// int), from which the caster pass splits the cascades.
    pub near: usize,
    pub far: usize,
    pub distance: usize,
    /// The cascades' matrices for lit surfaces, `0x40` bytes each (row
    /// vectors), from world space to the cascade's layer.
    pub matrices: usize,
}

/// The two `SceneViewImpl` layouts: every build up to GOG 2026-09-25, then
/// GOG 2026-10-07 on.
pub const LAYOUTS: [Layout; 2] = [
    Layout {
        site: "48 8b ce e8 ?? ?? ?? ?? 48 8b ce e8 ?? ?? ?? ?? 80 be 38 04 00 00 00",
        pass: "48 8b c4 48 89 58 10 48 89 70 18 48 89 78 20 55 41 54 41 55 41 56 \
     41 57 48 8d 68 a1 48 81 ec 00 01 00 00 0f 29 70 c8 0f 29 78 b8 44 0f 29 40 a8 \
     44 0f 29 48 98 44 0f 29 50 88 44 0f 29 98 78 ff ff ff 44 0f 29 a0 68 ff ff ff \
     48 8b f1 80 b9 43 04 00 00 00 0f 84 ?? ?? ?? ?? \
     48 8b 89 90 00 00 00 48 8b 01 ff 90 a8 00 00 00 4c 8b 38 4d 85 ff 0f 84 ?? ?? ?? ??",
        head: "48 8d 9e 30 03 00 00 b9 05 00 00 00 48 8d 85 a0 01 00 00",
        latch: "44 8b bd c0 04 00 00 44 3b be 40 0f 00 00 0f 82 ?? ?? ?? ??",
        clear: "48 8b 8e 98 00 00 00 48 8b 01 c7 44 24 28 ff ff ff ff c6 44 24 20 00 \
     f3 0f 10 1d ?? ?? ?? ?? 41 b8 03 00 00 00 48 8b 96 98 04 00 00 ff 90 48 01 00 00",
        shadows_on: 0x443,
        shadow_map: 0x498,
        cascade_count: 0xf40,
        near: 0xfa8,
        far: 0xfac,
        distance: 0xf44,
        matrices: 0x7a8,
    },
    Layout {
        site: "48 8b ce e8 ?? ?? ?? ?? 48 8b ce e8 ?? ?? ?? ?? 80 be 40 04 00 00 00",
        pass: "48 8b c4 48 89 58 10 48 89 70 18 48 89 78 20 55 41 54 41 55 41 56 \
     41 57 48 8d 68 a1 48 81 ec 00 01 00 00 0f 29 70 c8 0f 29 78 b8 44 0f 29 40 a8 \
     44 0f 29 48 98 44 0f 29 50 88 44 0f 29 98 78 ff ff ff 44 0f 29 a0 68 ff ff ff \
     48 8b f1 80 b9 4b 04 00 00 00 0f 84 ?? ?? ?? ?? \
     48 8b 89 90 00 00 00 48 8b 01 ff 90 a8 00 00 00 4c 8b 38 4d 85 ff 0f 84 ?? ?? ?? ??",
        head: "48 8d 9e 38 03 00 00 b9 05 00 00 00 48 8d 85 a0 01 00 00",
        latch: "44 8b bd c0 04 00 00 44 3b be 48 0f 00 00 0f 82 ?? ?? ?? ??",
        clear: "48 8b 8e 98 00 00 00 48 8b 01 c7 44 24 28 ff ff ff ff c6 44 24 20 00 \
     f3 0f 10 1d ?? ?? ?? ?? 41 b8 03 00 00 00 48 8b 96 a0 04 00 00 ff 90 48 01 00 00",
        shadows_on: 0x44b,
        shadow_map: 0x4a0,
        cascade_count: 0xf48,
        near: 0xfb0,
        far: 0xfb4,
        distance: 0xf4c,
        matrices: 0x7b0,
    },
];
/// The index in [`LAYOUTS`] of the running build's layout, set by
/// [`redirect`] before the pass is hooked.
static LAYOUT: AtomicUsize = AtomicUsize::new(0);

fn layout() -> &'static Layout {
    &LAYOUTS[LAYOUT.load(Ordering::Relaxed)]
}

/// Offset of the shadow pass's call in [`Layout::site`].
const SITE_CALL_AT: usize = 11;
/// Offset in the pass of its call to the caster pass (`fn_1916b0`).
const PASS_CASTERS_CALL_AT: usize = 0x145;
/// The caster pass's size, within which the loop's sites lie.
pub(crate) const CASTERS_SIZE: usize = 0xe34;
/// Offset of the `jb` in [`Layout::latch`].
const LATCH_JUMP_AT: usize = 14;
/// Each visible caster's cascade range: `vt+0xa0(min(vt+0xb0(), i))`, then
/// `vt+0xa8(max(vt+0xb8(), i))`, with `i` in `r15d`.
pub const RANGE_PATTERN: &str =
    "48 8b 3e 48 8b 07 48 8b 98 a0 00 00 00 48 8b cf ff 90 b0 00 00 00 \
     41 8b d7 41 3b c7 0f 42 d0 48 8b cf ff d3 48 8b 07 48 8b 98 a8 00 00 00 48 8b cf \
     ff 90 b8 00 00 00 41 8b d7 44 3b f8 0f 42 d0 48 8b cf ff d3";
/// Offset of [`Layout::clear`] in the pass, of its `movss xmm3` and its end.
const PASS_CLEAR_AT: usize = 0x40d;
const CLEAR_DEPTH_AT: usize = 0x17;
const CLEAR_SIZE: usize = 0x32;
/// The loop's slot for the next cascade index, relative to its `rbp`.
const LOOP_NEXT_INDEX: usize = 0x4c0;
/// The loop's slots for each cascade's fit (`fn_194c50`'s world-to-clip
/// matrix, `0x40` bytes each, row vectors), relative to its `rbp`. After the
/// loop the caster pass copies them into the `ShadowDataBuffer`
/// ([`FIT_COPY_PATTERN`]), and the pass draws each caster into its cascade
/// range's layers through them, before it puts [`Layout::matrices`] there for
/// lit surfaces.
const LOOP_FITS: usize = 0x220;
/// The copy's first loads after the loop (GOG 2026-09-25 `0x192305`):
/// `movsd xmm2, [rbp + rbx + 0x230]; movsd xmm10, [rbp + rbx + 0x220]`, at
/// [`LOOP_FITS`].
pub const FIT_COPY_PATTERN: &str = "f2 0f 10 94 1d 30 02 00 00 f2 44 0f 10 94 1d 20 02 00 00";
/// In the cascade loop, after the receivers are culled against the band and
/// before the casters are picked against them (GOG 2026-10-07 `0x1960eb`):
/// `r13` becomes cascade `r15d`'s receiver box (`rbp + 0x190 + i * 0x1c`), then
/// `VisibleMgr` vt+0x30 takes the band (`[rbp + 0x38]`), that box and the
/// light direction (`rax`). [`pad_stub`] goes [`PAD_AT`] bytes in, once `r13`
/// is set.
pub const PAD_PATTERN: &str = "49 6b cd 1c 4c 8d ad 90 01 00 00 4c 03 e9 48 8d 4c 24 60 \
     48 89 4c 24 28 48 8d 8d 30 01 00 00 48 89 4c 24 20 4c 8b c8 4d 8b c5 48 8b 55 38 \
     48 8b cf ff d3";
const PAD_AT: usize = 14;
/// The loop's band `shared_ptr` and its splits (`count + 1` floats), relative
/// to its `rbp`.
pub(crate) const LOOP_BAND: usize = 0x38;
const LOOP_SPLITS: usize = 0x48;
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
    let (device, map) = (field(view, VIEW_DEVICE), field(view, layout().shadow_map));
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

/// The plugin's one-layer depth views of one shadow map texture. Each view holds a
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

/// Whether `view`'s shadow map has the plugin's one-layer depth views for its
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

/// The scene view's device, which [`Layout::clear`] reads; the same in every
/// layout. The other fields the rotation reads are the [`Layout`]'s.
const VIEW_DEVICE: usize = 0x98;

/// Diagnostic, never shipped (feature `shadow-toggle`), kept privately.
#[cfg(feature = "shadow-toggle")]
#[path = "shadow_toggle.rs"]
mod toggle;

type Pass = unsafe extern "C" fn(*mut c_void);
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// `[defiance.performance] shadow_cascades`: which cascades a frame redraws.
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
static PAD_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
/// The margin a held layer is drawn with, as a fraction of its band's depth
/// ([`margin`]); 0 when the loop is not padded.
static MARGIN: AtomicU32 = AtomicU32::new(0);

/// `shadow_margin`'s fraction of a band's depth.
pub fn margin(text: &str) -> f32 {
    match text {
        "none" => 0.0,
        "small" => 0.05,
        "medium" => 0.1,
        _ => 0.2,
    }
}

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

/// How far (in world units) a corner of a band's slice may pass a plane of
/// its slice as drawn before the layer is redrawn: the corners are solved
/// from the planes, so rounding separates the two while the camera stands
/// still.
pub const SLACK: f64 = 0.01;

/// The cascade splits, `count + 1` of them, as the caster pass (`fn_1916b0`)
/// computes them: halfway between linear and logarithmic, from the near
/// plane to the far plane or the shadow distance, whichever is nearer.
pub fn splits(near: f32, far: f32, distance: i32, count: u32) -> Vec<f32> {
    let f = far.min(distance as f32).max(near);
    let c = count as f32;
    let mut s: Vec<f32> = (0..=count)
        .map(|i| {
            let t = i as f32 / c;
            0.5 * ((f - near) * t + near + near * (f / near).powf(t))
        })
        .collect();
    s[0] = near;
    s[count as usize] = f;
    s
}

/// A band's slice of the camera frustum: six planes `(n, d)`, a point `p`
/// inside each when `n . p + d >= 0`, as `fn_fe210` tests them.
pub type Slice = [[f32; 4]; 6];

/// The corners of `slice`: every point on three of its planes and inside the
/// rest.
pub fn corners(slice: &Slice) -> Vec<[f64; 3]> {
    let p: Vec<[f64; 4]> = slice.iter().map(|q| q.map(f64::from)).collect();
    let mut out = Vec::new();
    for a in 0..6 {
        for b in a + 1..6 {
            for c in b + 1..6 {
                let Some(at) = solve(p[a], p[b], p[c]) else {
                    continue;
                };
                if p.iter().all(|q| distance(*q, at) >= -SLACK) {
                    out.push(at);
                }
            }
        }
    }
    out
}

/// The point on planes `x`, `y` and `z`, if they meet in one.
fn solve(x: [f64; 4], y: [f64; 4], z: [f64; 4]) -> Option<[f64; 3]> {
    let cross = |u: [f64; 4], v: [f64; 4]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let (yz, zx, xy) = (cross(y, z), cross(z, x), cross(x, y));
    let det = x[0] * yz[0] + x[1] * yz[1] + x[2] * yz[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let at: [f64; 3] =
        core::array::from_fn(|k| -(x[3] * yz[k] + y[3] * zx[k] + z[3] * xy[k]) / det);
    at.iter().all(|v| v.is_finite()).then_some(at)
}

/// A band (`Galileo::KdTreeMath::Frustum`, built by `fn_fcc40`) as the cascade
/// loop fits its layer to it: its planes, its world box and its eight
/// corners. Its other fields (which planes bound each axis, the planes'
/// normals and its edge axes) do not change with [`pad`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub planes: Slice,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub corners: [[f32; 3]; 8],
}

/// `band` grown by `margin` world units on every side: each plane moved
/// outward, each corner solved again on the three nearest planes that meet in
/// a point, and the box grown to hold the corners. `None` when no three
/// planes meet.
pub fn pad(band: &Band, margin: f32) -> Option<Band> {
    let planes: Slice = band.planes.map(|q| {
        let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
        [q[0], q[1], q[2], q[3] + margin * len]
    });
    let (old, new) = (
        band.planes.map(|q| q.map(f64::from)),
        planes.map(|q| q.map(f64::from)),
    );
    let mut out = Band {
        planes,
        min: band.min.map(|v| v - margin),
        max: band.max.map(|v| v + margin),
        corners: band.corners,
    };
    for corner in out.corners.iter_mut() {
        let at = corner.map(f64::from);
        // The three nearest planes that meet in a point.
        let mut order: [usize; 6] = core::array::from_fn(|i| i);
        order.sort_by(|&a, &b| {
            distance(old[a], at)
                .abs()
                .total_cmp(&distance(old[b], at).abs())
        });
        let mut triples = (0..6)
            .flat_map(|a| (a + 1..6).flat_map(move |b| (b + 1..6).map(move |c| (a, b, c))))
            .map(|(a, b, c)| (order[a], order[b], order[c]));
        let moved = triples.find_map(|(a, b, c)| solve(new[a], new[b], new[c]))?;
        *corner = moved.map(|v| v as f32);
        for k in 0..3 {
            out.min[k] = out.min[k].min(corner[k]);
            out.max[k] = out.max[k].max(corner[k]);
        }
    }
    Some(out)
}

/// The signed distance of `at` from plane `q` (positive inside).
fn distance(q: [f64; 4], at: [f64; 3]) -> f64 {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    (q[0] * at[0] + q[1] * at[1] + q[2] * at[2] + q[3]) / len
}

/// Whether the slice `now` lies within the slice `drawn`; an unknown or
/// degenerate slice does not.
pub fn contains(drawn: Option<&Slice>, now: &Slice) -> bool {
    let Some(drawn) = drawn else {
        return false;
    };
    let corners = corners(now);
    corners.len() >= 4
        && corners.iter().all(|at| {
            drawn
                .iter()
                .all(|q| distance(q.map(f64::from), *at) >= -SLACK)
        })
}

/// A view's cascades' band slices as their layers were last drawn.
pub type Slices = [Option<Slice>; 32];

/// Per-view band slices of each cascade when its layer was last drawn, the
/// most recent views first, as [`Rotations`] keeps them.
pub struct Coverage {
    seen: Vec<(usize, Slices)>,
}

impl Coverage {
    const CAPACITY: usize = 8;

    pub const fn new() -> Self {
        Coverage { seen: Vec::new() }
    }

    /// `view`'s drawn slices; all unknown for a view not seen.
    pub fn slices(&self, view: usize) -> Slices {
        self.seen
            .iter()
            .find(|(v, _)| *v == view)
            .map_or([None; 32], |(_, slices)| *slices)
    }

    /// Records `view`'s slices for the cascades in `drawn`.
    pub fn record(&mut self, view: usize, drawn: u32, fresh: &Slices) {
        let mut slices = self.slices(view);
        for (i, slice) in fresh.iter().enumerate() {
            if drawn >> i & 1 != 0 {
                slices[i] = *slice;
            }
        }
        self.seen.retain(|(v, _)| *v != view);
        self.seen.insert(0, (view, slices));
        self.seen.truncate(Self::CAPACITY);
    }
}

impl Default for Coverage {
    fn default() -> Self {
        Self::new()
    }
}

/// The view's camera holder; its vt+0x60 answers the camera, whose vt+0x100
/// answers the frustum between two distances (a new one each call,
/// `CameraImpl` vt+0x100, GOG 2026-10-07 `0xd6860`), its world box at +0x80
/// (an empty flag, then min and max) and its eight corners at +0xa4.
const VIEW_CAMERAS: usize = 0x90;
const HOLDER_CAMERA: usize = 0x60;
const CAMERA_BAND: usize = 0x100;
const BAND_PLANES: usize = 8;
pub(crate) const BAND_BOX: usize = 0x80;
const BAND_CORNERS: usize = 0xa4;
/// A box as the loop keeps one per cascade for its receivers: an empty flag,
/// then min and max.
pub(crate) const BOX_MIN: usize = 4;
pub(crate) const BOX_MAX: usize = 0x10;

/// Drops an MSVC `shared_ptr`'s reference through its control block: uses
/// at +8, weak references at +0xc, `_Destroy` and `_Delete_this` its first
/// two methods.
unsafe fn drop_shared(control: usize) {
    use std::sync::atomic::AtomicI32;
    if control == 0 {
        return;
    }
    let count = |at: usize| unsafe { &*((control + at) as *const AtomicI32) };
    if count(8).fetch_sub(1, Ordering::AcqRel) == 1 {
        let object = control as *mut c_void;
        let destroy: unsafe extern "C" fn(*mut c_void) = unsafe { method(object, 0) };
        unsafe { destroy(object) };
        if count(0xc).fetch_sub(1, Ordering::AcqRel) == 1 {
            let delete: unsafe extern "C" fn(*mut c_void) = unsafe { method(object, 8) };
            unsafe { delete(object) };
        }
    }
}

/// Each cascade's band this frame, asked of the view's camera as the caster
/// pass asks it; `None` for a band that is empty. `None` overall when the
/// view has no camera.
unsafe fn bands(view: *const u8, layout: &Layout, count: u32) -> Option<Slices> {
    type Shared = [usize; 2];
    let holder = unsafe { *(view.add(VIEW_CAMERAS) as *const *mut c_void) };
    if holder.is_null() {
        return None;
    }
    let get: unsafe extern "C" fn(*mut c_void, *mut Shared) -> *mut Shared =
        unsafe { method(holder, HOLDER_CAMERA) };
    let mut camera: Shared = [0; 2];
    unsafe { get(holder, &mut camera) };
    if camera[0] == 0 {
        unsafe { drop_shared(camera[1]) };
        return None;
    }
    let read = |at: usize| unsafe { *(view.add(at) as *const f32) };
    let distance = unsafe { *(view.add(layout.distance) as *const i32) };
    let near = read(layout.near);
    let splits = splits(near, read(layout.far), distance, count);
    let mut out = [None; 32];
    if near > 0.0 && splits.iter().all(|s| s.is_finite()) {
        let cam = camera[0] as *mut c_void;
        let band: unsafe extern "C" fn(*mut c_void, *mut Shared, f32, f32) -> *mut Shared =
            unsafe { method(cam, CAMERA_BAND) };
        for (i, slot) in out.iter_mut().enumerate().take(count as usize) {
            let mut frustum: Shared = [0; 2];
            unsafe { band(cam, &mut frustum, splits[i], splits[i + 1]) };
            let at = frustum[0] as *const u8;
            if !at.is_null() && unsafe { *at.add(BAND_BOX) } == 0 {
                let plane = |i: usize| unsafe {
                    (at.add(BAND_PLANES + 0x10 * i) as *const [f32; 4]).read_unaligned()
                };
                let slice: Slice = core::array::from_fn(plane);
                if slice.iter().flatten().all(|v| v.is_finite()) {
                    *slot = Some(slice);
                }
            }
            unsafe { drop_shared(frustum[1]) };
        }
    }
    unsafe { drop_shared(camera[1]) };
    Some(out)
}

/// Cascade `i`'s matrix for lit surfaces in the view.
unsafe fn matrix(view: *const u8, layout: &Layout, i: usize) -> [f32; 16] {
    unsafe { (view.add(layout.matrices + 0x40 * i) as *const [f32; 16]).read_unaligned() }
}

/// Per-view matrices for lit surfaces ([`Layout::matrices`]) of each cascade
/// as its layer was last drawn, the most recent views first, as [`Rotations`] keeps them. A view
/// whose map, count or shadow switch changed has none.
pub struct Held {
    seen: Vec<(usize, Seen, HeldMatrices)>,
}

/// One view's cascades' matrices, `None` where unknown.
pub type HeldMatrices = [Option<[f32; 16]>; Held::CASCADES];

impl Held {
    const CAPACITY: usize = 8;
    /// The most cascades a view may have and still hold any of them.
    pub const CASCADES: usize = 8;

    pub const fn new() -> Self {
        Held { seen: Vec::new() }
    }

    /// `view`'s matrices, if its map is still `now`.
    pub fn matrices(&self, view: usize, now: Seen) -> HeldMatrices {
        self.seen
            .iter()
            .find(|(v, seen, _)| *v == view && *seen == now)
            .map_or([None; Held::CASCADES], |(_, _, m)| *m)
    }

    /// Records `view`'s matrices for the cascades in `drawn`.
    pub fn record(&mut self, view: usize, now: Seen, drawn: u32, fresh: &HeldMatrices) {
        let mut held = self.matrices(view, now);
        for (i, m) in fresh.iter().enumerate() {
            if drawn >> i & 1 != 0 {
                held[i] = *m;
            }
        }
        self.seen.retain(|(v, _, _)| *v != view);
        self.seen.insert(0, (view, now, held));
        self.seen.truncate(Self::CAPACITY);
    }
}

impl Default for Held {
    fn default() -> Self {
        Self::new()
    }
}

/// The first `count` cascades not in `mask` whose bands are unknown or have
/// left their bands as drawn.
fn stale(mask: u32, count: u32, bands: &Slices, drawn: &Slices) -> u32 {
    let mut stale = 0;
    for (i, band) in bands.iter().enumerate().take(count as usize) {
        let held = band
            .as_ref()
            .is_some_and(|band| contains(drawn[i].as_ref(), band));
        if mask >> i & 1 == 0 && !held {
            stale |= 1 << i;
        }
    }
    stale
}

/// Called by [`head_stub`] for each cascade: whether the loop runs it.
unsafe extern "C" fn cascade_due(view: usize, index: u32) -> u8 {
    if view == 0 || PARTIAL_VIEW.load(Ordering::Relaxed) != view {
        return 1;
    }
    (index < 32 && PARTIAL_MASK.load(Ordering::Relaxed) >> index & 1 != 0) as u8
}

/// Called by [`clear_stub`] in place of the pass's clear: clears only the due
/// layers, with the plugin's layer views ([`layer_views_ready`] made them this
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
/// cascade when [`cascade_due`] says so, else empties its fit slot
/// ([`LOOP_FITS`]), stores the next index where the loop keeps it and jumps to
/// the loop's test. A caster whose range spans a skipped cascade (found in
/// cascades on both sides of it) is still drawn into that layer through the
/// slot, which would otherwise hold whatever an earlier call left on the
/// stack; the empty fit sends every vertex to one point past the far plane, so
/// the held layer keeps its map. The volatile registers and flags are dead
/// here.
/// Inline hook sites do not promise Windows x64 call alignment. The stubs save
/// and restore the game stack around each Rust callback.
#[unsafe(naked)]
unsafe extern "C" fn head_stub() {
    core::arch::naked_asm!(
        "push rbx",
        "mov rbx, rsp",
        "and rsp, -16",
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "mov edx, r15d",
        "call {due}",
        "test al, al",
        "jnz 2f",
        "mov rsp, rbx",
        "pop rbx",
        "mov eax, r15d",
        "shl rax, 6",
        "lea rax, [rbp + rax + {fits}]",
        "xor ecx, ecx",
        "mov qword ptr [rax], rcx",
        "mov qword ptr [rax + 0x08], rcx",
        "mov qword ptr [rax + 0x10], rcx",
        "mov qword ptr [rax + 0x18], rcx",
        "mov qword ptr [rax + 0x20], rcx",
        "mov qword ptr [rax + 0x28], rcx",
        "mov qword ptr [rax + 0x30], rcx",
        // The last row (0, 0, 2, 1): z = 2w, past the far plane.
        "mov rcx, 0x3f80000040000000",
        "mov qword ptr [rax + 0x38], rcx",
        "lea eax, [r15 + 1]",
        "mov dword ptr [rbp + {next}], eax",
        "jmp qword ptr [rip + {latch}]",
        "2:",
        "mov rsp, rbx",
        "pop rbx",
        "jmp qword ptr [rip + {head}]",
        due = sym cascade_due,
        next = const LOOP_NEXT_INDEX,
        fits = const LOOP_FITS,
        latch = sym LATCH,
        head = sym HEAD_TRAMPOLINE,
    );
}

/// At the pass's clear (in place of its first instruction): [`clear_layers`]
/// clears the due layers and the pass resumes after its clear, or the pass
/// clears them all itself. Volatile registers and flags are dead here too; the
/// inline hook site does not promise call-aligned `rsp`.
#[unsafe(naked)]
unsafe extern "C" fn clear_stub() {
    core::arch::naked_asm!(
        "push rbx",
        "mov rbx, rsp",
        "and rsp, -16",
        "sub rsp, 0x20",
        "mov rcx, rsi",
        "call {clear}",
        "mov rsp, rbx",
        "pop rbx",
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

/// The pass running with its holdable layers padded: its view, the
/// cascades it pads, and each padded cascade's slice as drawn.
struct Padding {
    view: usize,
    cascades: u32,
    slices: Slices,
}

/// Called by [`pad_stub`] for each cascade the loop runs: grows a cascade in
/// [`Padding::cascades`] by [`MARGIN`] of its depth, its band (`[frame +
/// LOOP_BAND]`) and its receivers' box alike, so its layer covers the camera
/// for a while as it moves and is held for longer. The casters are then
/// picked, and the layer fitted, for the grown band.
unsafe extern "C" fn pad_band(view: usize, index: u32, frame: *const u8, receivers: *mut u8) {
    let i = index as usize;
    let fraction = f32::from_bits(MARGIN.load(Ordering::Relaxed));
    PADDING.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let Some(state) = state.as_mut() else {
            return;
        };
        if view == 0 || state.view != view || i >= 32 || state.cascades >> i & 1 == 0 {
            return;
        }
        let split = |k: usize| unsafe { *(frame.add(LOOP_SPLITS + 4 * k) as *const f32) };
        let margin = fraction * (split(i + 1) - split(i));
        let at = unsafe { *(frame.add(LOOP_BAND) as *const *mut u8) };
        if !(margin.is_finite() && margin > 0.0)
            || at.is_null()
            || unsafe { *at.add(BAND_BOX) } != 0
            || unsafe { *receivers } != 0
        {
            return;
        }
        let read3 = |p: *const u8| unsafe { (p as *const [f32; 3]).read_unaligned() };
        let band = Band {
            planes: core::array::from_fn(|q| unsafe {
                (at.add(BAND_PLANES + 0x10 * q) as *const [f32; 4]).read_unaligned()
            }),
            min: read3(unsafe { at.add(BAND_BOX + BOX_MIN) }),
            max: read3(unsafe { at.add(BAND_BOX + BOX_MAX) }),
            corners: core::array::from_fn(|c| read3(unsafe { at.add(BAND_CORNERS + 12 * c) })),
        };
        let finite = |b: &Band| {
            b.planes.iter().flatten().all(|v| v.is_finite())
                && b.corners.iter().flatten().all(|v| v.is_finite())
                && b.min.iter().chain(&b.max).all(|v| v.is_finite())
        };
        if !finite(&band) {
            return;
        }
        let Some(padded) = pad(&band, margin).filter(finite) else {
            return;
        };
        let write3 = |p: *mut u8, v: [f32; 3]| unsafe { (p as *mut [f32; 3]).write_unaligned(v) };
        unsafe {
            for (q, plane) in padded.planes.iter().enumerate() {
                (at.add(BAND_PLANES + 0x10 * q) as *mut [f32; 4]).write_unaligned(*plane);
            }
            for (c, corner) in padded.corners.iter().enumerate() {
                write3(at.add(BAND_CORNERS + 12 * c), *corner);
            }
            write3(at.add(BAND_BOX + BOX_MIN), padded.min);
            write3(at.add(BAND_BOX + BOX_MAX), padded.max);
            let (min, max) = (read3(receivers.add(BOX_MIN)), read3(receivers.add(BOX_MAX)));
            write3(receivers.add(BOX_MIN), min.map(|v| v - margin));
            write3(receivers.add(BOX_MAX), max.map(|v| v + margin));
        }
        state.slices[i] = Some(padded.planes);
    });
}

/// In the cascade loop at [`PAD_PATTERN`] + [`PAD_AT`]: [`pad_band`], then
/// the loop's own instructions. `rax` (the light direction) is live and kept;
/// the other volatile registers and flags are dead.
#[unsafe(naked)]
unsafe extern "C" fn pad_stub() {
    core::arch::naked_asm!(
        "push rbx",
        "mov rbx, rsp",
        "and rsp, -16",
        "sub rsp, 0x30",
        "mov qword ptr [rsp + 0x20], rax",
        "mov rcx, rsi",
        "mov edx, r15d",
        "mov r8, rbp",
        "mov r9, r13",
        "call {pad}",
        "mov rax, qword ptr [rsp + 0x20]",
        "mov rsp, rbx",
        "pop rbx",
        "jmp qword ptr [rip + {original}]",
        pad = sym pad_band,
        original = sym PAD_TRAMPOLINE,
    );
}

thread_local! {
    static PADDING: RefCell<Option<Padding>> = const { RefCell::new(None) };
    static ROTATIONS: RefCell<Rotations> = const { RefCell::new(Rotations::new()) };
    static COVERAGE: RefCell<Coverage> = const { RefCell::new(Coverage::new()) };
    static HELD: RefCell<Held> = const { RefCell::new(Held::new()) };
}

/// The world2 call can arrive with a misaligned stack; this adapter restores
/// the Windows x64 alignment before Rust enters [`pass`].
#[unsafe(naked)]
unsafe extern "C" fn pass_stub() {
    core::arch::naked_asm!(
        "push rbx",
        "mov rbx, rsp",
        "and rsp, -16",
        "sub rsp, 0x20",
        "call {pass}",
        "mov rsp, rbx",
        "pop rbx",
        "ret",
        pass = sym pass,
    );
}

/// The function in slot `offset` of the vtable of the object at `object`.
pub(crate) unsafe fn method<F: Copy>(object: *mut c_void, offset: usize) -> F {
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
    #[cfg(all(feature = "shadow-timing", feature = "render-profile-toggle"))]
    let timing = if crate::render_profile::gpu_enabled() {
        timing::begin(view)
    } else {
        None
    };
    #[cfg(all(feature = "shadow-timing", not(feature = "render-profile-toggle")))]
    let timing = timing::begin(view);
    if !ROTATING.load(Ordering::Relaxed) {
        #[cfg(feature = "render-profile")]
        let mask = if view.is_null() {
            0
        } else {
            let count = unsafe { *(view.cast::<u8>().add(layout().cascade_count) as *const u32) };
            Cascades::All.due(0, count)
        };
        #[cfg(feature = "render-profile")]
        let started = crate::render_profile::shadow_begin(view);
        unsafe { original(view) };
        #[cfg(feature = "render-profile")]
        crate::render_profile::shadow_end(view, mask, started);
        #[cfg(feature = "shadow-timing")]
        timing::end(timing);
        #[cfg(feature = "render-profile")]
        crate::render_profile::flush();
        return;
    }
    let (bytes, layout) = (view as *const u8, layout());
    let now = Seen {
        map: unsafe { *(bytes.add(layout.shadow_map) as *const usize) },
        count: unsafe { *(bytes.add(layout.cascade_count) as *const u32) },
        on: unsafe { *bytes.add(layout.shadows_on) } != 0,
    };
    let mode = Cascades::parse(match ROTATION.load(Ordering::Relaxed) {
        1 => "rotate",
        2 => "near",
        3 => "far_half",
        _ => "all",
    });
    let mut mask = ROTATIONS.with(|r| match r.try_borrow_mut() {
        Ok(mut r) => r.due(view as usize, now, mode),
        Err(_) => Cascades::All.due(0, now.count),
    });
    let all = Cascades::All.due(0, now.count);
    let holdable = now.on && (1..=Held::CASCADES as u32).contains(&now.count);
    // A held layer needs the matrix it was drawn with to check its coverage;
    // without it every layer is drawn.
    let held = HELD.with(|h| {
        h.try_borrow()
            .map_or([None; Held::CASCADES], |h| h.matrices(view as usize, now))
    });
    if !holdable || (0..now.count as usize).any(|i| mask >> i & 1 == 0 && held[i].is_none()) {
        mask = all;
    }
    // A layer that is not due keeps last frame's map; redraw it anyway when
    // the camera's band for it has moved past the band it was drawn for.
    let checked = holdable && mask != all;
    let bands = if checked {
        unsafe { bands(bytes, layout, now.count) }
    } else {
        None
    };
    // Without bands no held layer can be checked, so every layer is drawn.
    match &bands {
        Some(bands) => {
            let drawn = COVERAGE.with(|c| {
                c.try_borrow()
                    .map_or([None; 32], |c| c.slices(view as usize))
            });
            mask |= stale(mask, now.count, bands, &drawn);
        }
        None if checked => mask = all,
        None => {}
    }
    let effective_mask = if mask != all && unsafe { layer_views_ready(view, now.count) } {
        PARTIAL_MASK.store(mask, Ordering::Relaxed);
        PARTIAL_VIEW.store(view as usize, Ordering::Relaxed);
        mask
    } else {
        all
    };
    // Layers that may be held are drawn for a padded band, so they cover the
    // camera's band for longer as it moves; those drawn every frame are not.
    let padded = if holdable && MARGIN.load(Ordering::Relaxed) != 0 {
        let always = (0..2 * now.count).fold(all, |m, f| m & mode.due(f, now.count));
        all & !always
    } else {
        0
    };
    PADDING.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            *state = (padded != 0).then_some(Padding {
                view: view as usize,
                cascades: padded,
                slices: [None; 32],
            });
        }
    });
    #[cfg(feature = "render-profile")]
    let started = crate::render_profile::shadow_begin(view);
    unsafe { original(view) };
    let padded = PADDING.with(|state| {
        state
            .try_borrow_mut()
            .ok()
            .and_then(|mut state| state.take())
            .map_or([None; 32], |state| state.slices)
    });
    if holdable {
        let fresh: HeldMatrices = core::array::from_fn(|i| {
            (i < now.count as usize).then(|| unsafe { matrix(bytes, layout, i) })
        });
        HELD.with(|h| {
            if let Ok(mut h) = h.try_borrow_mut() {
                h.record(view as usize, now, effective_mask, &fresh);
            }
        });
        // Without bands the slices are unknown, so the next frame draws every
        // layer.
        // A padded layer covers its padded slice.
        let bands = bands.or_else(|| unsafe { self::bands(bytes, layout, now.count) });
        let slices: Slices = match bands {
            Some(bands) => core::array::from_fn(|i| padded[i].or(bands[i])),
            None => [None; 32],
        };
        COVERAGE.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                c.record(view as usize, effective_mask, &slices);
            }
        });
    }
    #[cfg(feature = "render-profile")]
    crate::render_profile::shadow_end(view, effective_mask, started);
    #[cfg(feature = "shadow-timing")]
    timing::end(timing);
    PARTIAL_VIEW.store(0, Ordering::Relaxed);
    #[cfg(feature = "render-profile")]
    crate::render_profile::flush();
}

/// Discards samples from the prior diagnostic mode before a new GPU window.
#[cfg(feature = "render-profile-toggle")]
pub(crate) fn reset_profile_timing() {
    timing::reset();
}

fn find(api: &Api, base: *mut c_void, size: usize, pattern: &str) -> *mut u8 {
    let pattern = std::ffi::CString::new(pattern).unwrap_or_default();
    unsafe { (api.find_pattern)(base, size, pattern.as_ptr()) as *mut u8 }
}

/// The D3D11 methods used to validate the game's private view fields: the
/// device's clear and the texture's depth-view creator.
fn d3d_signatures(api: &Api) -> Result<(usize, usize), String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let (context_at, resource_at) = (
        find(api, base, size, DEVICE_CONTEXT_PATTERN) as usize,
        find(api, base, size, TEXTURE_RESOURCE_PATTERN) as usize,
    );
    if context_at == 0 || resource_at == 0 {
        return Err("the device's context or the texture's resource was not found".into());
    }
    Ok((
        context_at - DEVICE_CONTEXT_AT,
        resource_at - TEXTURE_RESOURCE_AT,
    ))
}

fn set_d3d_signatures((clear, depth_view): (usize, usize)) {
    DEVICE_CLEAR_FN.store(clear, Ordering::Relaxed);
    TEXTURE_DEPTH_VIEW_FN.store(depth_view, Ordering::Relaxed);
}

/// The pass's redirect, each address vouched for by its pattern and its place
/// in the scene render or the pass.
struct Redirect {
    layout: usize,
    pass: usize,
    casters: usize,
    write: Write,
}

/// The cascade loop's stubs and the values they read.
struct Rotation {
    signatures: (usize, usize),
    depth: f32,
    latch: usize,
    resume: usize,
    /// Whether [`Rotation::writes`] pads the held layers' bands.
    padded: bool,
    writes: Vec<Write>,
}

fn redirect(api: &Api) -> Result<Redirect, String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    if base.is_null() {
        return Err("world2.dll is not loaded".into());
    }
    let size = unsafe { (api.module_size)(base) };
    let sites = LAYOUTS.each_ref().map(|layout| layout.site);
    let order = crate::matching_first(base, size, &sites);
    let Some((layout, site, pass)) = order.into_iter().find_map(|i| {
        let layout = &LAYOUTS[i];
        let site = find(api, base, size, layout.site) as usize;
        let pass = find(api, base, size, layout.pass) as usize;
        (site != 0 && pass != 0).then_some((i, site, pass))
    }) else {
        return Err("the scene render's shadow pass was not found".into());
    };
    let call = site + SITE_CALL_AT;
    if plan::call_target(call) != Some(pass) {
        return Err("the scene render calls a different shadow pass".into());
    }
    let casters = plan::call_target(pass + PASS_CASTERS_CALL_AT).unwrap_or(0);
    Ok(Redirect {
        layout,
        pass,
        casters,
        write: Write {
            target: call,
            kind: Kind::Call,
            detour: pass_stub as unsafe extern "C" fn() as *mut c_void,
            original: Some(&ORIGINAL),
        },
    })
}

/// The loop-head and clear stubs for [`Cascades`], inside the pass that
/// [`redirect`] found.
fn rotation(api: &Api, redirect: &Redirect, margin: f32) -> Result<Rotation, String> {
    let base = unsafe { (api.module_base)(c"world2.dll".as_ptr()) };
    let size = unsafe { (api.module_size)(base) };
    let layout = &LAYOUTS[redirect.layout];
    let (head, latch, range, copy, clear, pad) = (
        find(api, base, size, layout.head) as usize,
        find(api, base, size, layout.latch) as usize,
        find(api, base, size, RANGE_PATTERN) as usize,
        find(api, base, size, FIT_COPY_PATTERN) as usize,
        find(api, base, size, layout.clear) as usize,
        find(api, base, size, PAD_PATTERN) as usize,
    );
    let signatures = d3d_signatures(api)?;
    let casters = redirect.casters;
    let inside = |at: usize| at > casters && at < casters + CASTERS_SIZE;
    if !(inside(head) && inside(latch) && inside(range) && copy > latch && inside(copy))
        || jb_target(latch + LATCH_JUMP_AT) != Some(head)
        || clear != redirect.pass + PASS_CLEAR_AT
    {
        return Err("the cascade loop or the clear is laid out differently".into());
    }
    let depth_at = clear + CLEAR_DEPTH_AT;
    let load = plan::original(depth_at, 8).ok_or("the clear is unreadable")?;
    let disp = i32::from_le_bytes([load[4], load[5], load[6], load[7]]);
    let depth = unsafe { *(((depth_at + 8) as isize + disp as isize) as *const f32) };
    let stub = |target, detour, original| Write {
        target,
        kind: Kind::Entry(None),
        detour,
        original: Some(original),
    };
    let mut writes = vec![
        stub(
            head,
            head_stub as unsafe extern "C" fn() as *mut c_void,
            &HEAD_TRAMPOLINE,
        ),
        stub(
            clear,
            clear_stub as unsafe extern "C" fn() as *mut c_void,
            &CLEAR_TRAMPOLINE,
        ),
    ];
    // Without its site the layers are held for their bands as they are.
    let padded = margin > 0.0 && inside(pad) && head < pad && pad < latch;
    if padded {
        writes.push(stub(
            pad + PAD_AT,
            pad_stub as unsafe extern "C" fn() as *mut c_void,
            &PAD_TRAMPOLINE,
        ));
    }
    Ok(Rotation {
        signatures,
        depth,
        latch,
        resume: clear + CLEAR_SIZE,
        padded,
        writes,
    })
}

/// The target of the original `0f 82 rel32` (`jb`) at `site`.
fn jb_target(site: usize) -> Option<usize> {
    let bytes = plan::original(site, 6)?;
    if bytes[..2] != [0x0f, 0x82] {
        return None;
    }
    let rel = i32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    Some((site as isize + 6 + rel as isize) as usize)
}

/// Whether `install` redirects the pass for `mode`.
fn redirects(api: &Api, mode: Cascades) -> bool {
    let _ = api;
    #[cfg(feature = "shadow-timing")]
    if mode == Cascades::All {
        return cfg!(feature = "render-profile-toggle") || d3d_signatures(api).is_ok();
    }
    mode != Cascades::All
}

/// The writes `install` makes for `cascades`: the pass's redirect, and the
/// loop's stubs when the cascades rotate and the loop resolves.
pub fn plan(api: &Api, cascades: &str, margin: &str) -> Result<Plan, String> {
    let mode = Cascades::parse(cascades);
    if !redirects(api, mode) {
        return Ok(Plan::default());
    }
    let redirect = redirect(api)?;
    let rotation = (mode != Cascades::All)
        .then(|| rotation(api, &redirect, self::margin(margin)).ok())
        .flatten();
    let mut writes = vec![redirect.write];
    writes.extend(rotation.into_iter().flat_map(|rotation| rotation.writes));
    Ok(Plan { writes })
}

/// `cascades` is `shadow_cascades`, `all` leaving the pass alone; `margin` is
/// `shadow_margin`.
pub fn install(api: &Api, cascades: &str, margin: &str) {
    let margin = self::margin(margin);
    let mode = Cascades::parse(cascades);
    #[cfg(feature = "shadow-timing")]
    timing::install(api.log, mode);
    #[cfg(feature = "shadow-timing")]
    if mode == Cascades::All {
        match d3d_signatures(api) {
            Ok(signatures) => set_d3d_signatures(signatures),
            Err(e) => super::say(
                api,
                LOG_WARN,
                &format!("shadow timing: {e}; GPU timestamps are unavailable"),
            ),
        }
    }
    if !redirects(api, mode) {
        return;
    }
    let result = redirect(api).and_then(|redirect| {
        LAYOUT.store(redirect.layout, Ordering::Relaxed);
        let rotation = (mode != Cascades::All).then(|| rotation(api, &redirect, margin));
        let pass = Plan {
            writes: vec![redirect.write],
        };
        plan::apply(api, &pass).map_err(|_| "its call could not be redirected".to_string())?;
        Ok(rotation)
    });
    let rotation = match result {
        Ok(Some(rotation)) => rotation,
        Ok(None) => {
            #[cfg(feature = "shadow-timing")]
            super::say(
                api,
                LOG_INFO,
                "shadow timing: measuring the stock all-cascade pass",
            );
            return;
        }
        Err(e) => {
            super::say(
                api,
                LOG_WARN,
                &format!("shadow map: {e}; every cascade is redrawn each frame"),
            );
            return;
        }
    };
    let result = rotation.and_then(|rotation| {
        set_d3d_signatures(rotation.signatures);
        CLEAR_DEPTH.store(rotation.depth.to_bits(), Ordering::Relaxed);
        LATCH.store(rotation.latch, Ordering::Release);
        CLEAR_RESUME.store(rotation.resume, Ordering::Release);
        if rotation.padded {
            MARGIN.store(margin.to_bits(), Ordering::Relaxed);
        } else if margin > 0.0 {
            super::say(
                api,
                LOG_WARN,
                "shadow map: the cascade loop's bands were not found; held parts are not padded",
            );
        }
        let stubs = Plan {
            writes: rotation.writes,
        };
        plan::apply(api, &stubs).map_err(|_| "the cascade loop could not be hooked".to_string())
    });
    match result {
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
    fn held_matrices_merge_and_drop_with_the_map() {
        let map = Seen {
            map: 0x5000,
            count: 4,
            on: true,
        };
        let one = |v: f32| -> HeldMatrices { core::array::from_fn(|_| Some([v; 16])) };
        let mut h = Held::new();
        assert_eq!(h.matrices(0x1000, map), [None; Held::CASCADES]);
        h.record(0x1000, map, 0b1111, &one(1.0));
        h.record(0x1000, map, 0b0010, &one(2.0));
        let m = h.matrices(0x1000, map);
        assert_eq!(m[0], Some([1.0; 16]));
        assert_eq!(m[1], Some([2.0; 16]));
        assert_eq!(m[4], None);
        // A changed map forgets the view's matrices.
        let resized = Seen { map: 0x6000, ..map };
        assert_eq!(h.matrices(0x1000, resized), [None; Held::CASCADES]);
        h.record(0x1000, resized, 0b0001, &one(3.0));
        let m = h.matrices(0x1000, resized);
        assert_eq!((m[0], m[1]), (Some([3.0; 16]), None));
        // The least recent view drops past capacity.
        for view in 1..=Held::CAPACITY {
            h.record(view, map, 1, &one(4.0));
        }
        assert_eq!(h.matrices(0x1000, resized), [None; Held::CASCADES]);
        assert_eq!(h.matrices(1, map)[0], Some([4.0; 16]));
    }

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
        let (count, map_at) = (layout().cascade_count, layout().shadow_map);
        view[count..count + 4].copy_from_slice(&4u32.to_le_bytes());
        view[VIEW_DEVICE..VIEW_DEVICE + 8]
            .copy_from_slice(&(device.as_ptr() as usize).to_le_bytes());
        view[map_at..map_at + 8].copy_from_slice(&(map.as_ptr() as usize).to_le_bytes());
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

    #[test]
    fn splits_run_from_near_to_the_nearer_far() {
        let s = splits(1.0, 1000.0, 200, 4);
        assert_eq!(s.len(), 5);
        assert_eq!((s[0], s[4]), (1.0, 200.0));
        assert!(s.windows(2).all(|w| w[0] < w[1]));
        // Halfway between linear (50.75) and logarithmic (200^0.25).
        let want = 0.5 * (199.0 * 0.25 + 1.0 + 200f32.powf(0.25));
        assert!((s[1] - want).abs() < 1e-4);
        assert_eq!(*splits(1.0, 100.0, 200, 2).last().unwrap(), 100.0);
    }

    /// The box `min..max` as six planes.
    fn boxed(min: [f32; 3], max: [f32; 3]) -> Slice {
        let mut s = [[0.0; 4]; 6];
        for k in 0..3 {
            s[2 * k][k] = 1.0;
            s[2 * k][3] = -min[k];
            s[2 * k + 1][k] = -1.0;
            s[2 * k + 1][3] = max[k];
        }
        s
    }

    #[test]
    fn a_slice_has_its_corners() {
        let c = corners(&boxed([0.0; 3], [1.0, 2.0, 3.0]));
        assert_eq!(c.len(), 8);
        assert!(c.iter().any(|p| p == &[1.0, 2.0, 3.0]));
    }

    #[test]
    fn a_padded_band_grows_by_its_margin() {
        let planes = boxed([0.0; 3], [1.0, 2.0, 3.0]);
        let c = corners(&planes);
        let band = Band {
            planes,
            min: [0.0; 3],
            max: [1.0, 2.0, 3.0],
            corners: core::array::from_fn(|i| c[i].map(|v| v as f32)),
        };
        let padded = pad(&band, 0.5).unwrap();
        assert_eq!(padded.planes, boxed([-0.5; 3], [1.5, 2.5, 3.5]));
        assert_eq!((padded.min, padded.max), ([-0.5; 3], [1.5, 2.5, 3.5]));
        for (old, new) in band.corners.iter().zip(&padded.corners) {
            for k in 0..3 {
                let moved = if old[k] == 0.0 { -0.5 } else { 0.5 };
                assert!((new[k] - old[k] - moved).abs() < 1e-5, "{old:?} -> {new:?}");
            }
        }
        // The padded band holds the drawn one, and a band moved by less
        // than the margin.
        assert!(contains(Some(&padded.planes), &planes));
        assert!(contains(
            Some(&padded.planes),
            &boxed([0.4, 0.0, 0.0], [1.4, 2.0, 3.0])
        ));
        assert_eq!(pad(&band, 0.0).unwrap(), band);
    }

    #[test]
    fn a_band_that_moves_past_its_drawn_band_redraws_its_layer() {
        let drawn = boxed([2.0, 2.0, 0.0], [8.0, 8.0, 1.0]);
        assert!(contains(Some(&drawn), &drawn));
        // Within the slice as drawn: held.
        assert!(contains(
            Some(&drawn),
            &boxed([3.0, 2.0, 0.0], [8.0, 7.0, 1.0])
        ));
        // Past it: its casters were never drawn.
        assert!(!contains(
            Some(&drawn),
            &boxed([2.1, 2.0, 0.0], [8.1, 8.0, 1.0])
        ));
        assert!(!contains(None, &drawn));
        // A slice turned within its old box: the box still holds it, the
        // slice does not.
        let wedge = |tilt: f32| {
            let mut s = boxed([0.0; 3], [10.0; 3]);
            s[0] = [1.0, -tilt, 0.0, 0.0];
            s
        };
        assert!(!contains(Some(&wedge(0.5)), &wedge(0.0)));
        assert!(contains(Some(&wedge(0.0)), &wedge(0.5)));
    }

    #[test]
    fn a_band_not_known_redraws_its_layer() {
        let slice = Some(boxed([0.0; 3], [1.0; 3]));
        let mut bands = [None; 32];
        bands[0] = slice;
        bands[2] = slice;
        let drawn = [slice; 32];
        // Cascade 1 is due; 3 has no band; 4 is past the count.
        assert_eq!(stale(0b0010, 4, &bands, &drawn), 0b1000);
        assert_eq!(stale(0, 3, &bands, &[None; 32]), 0b0111);
    }

    #[test]
    fn coverage_keeps_each_views_drawn_slices() {
        let mut c = Coverage::new();
        assert_eq!(c.slices(1), [None; 32]);
        let (a, b) = (
            Some(boxed([0.0; 3], [1.0; 3])),
            Some(boxed([0.0; 3], [2.0; 3])),
        );
        let mut slices = [None; 32];
        slices[0] = a;
        slices[1] = b;
        c.record(1, 0b01, &slices);
        assert_eq!(c.slices(1)[..2], [a, None]);
        c.record(1, 0b10, &[b; 32]);
        assert_eq!(c.slices(1)[..3], [a, b, None]);
        for view in 2..20 {
            c.record(view, 1, &slices);
        }
        assert_eq!(c.slices(1), [None; 32]);
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
        let layout = LAYOUTS
            .iter()
            .find(|layout| !find(layout.site).is_empty())
            .expect("a layout's site matches");
        let (sites, passes) = (find(layout.site), find(layout.pass));
        assert_eq!((sites.len(), passes.len()), (1, 1));
        let target = |call: usize| {
            assert_eq!(image[call], 0xe8);
            let rel = i32::from_le_bytes(image[call + 1..call + 5].try_into().unwrap());
            (call as isize + 5 + rel as isize) as usize
        };
        assert_eq!(target(sites[0] + SITE_CALL_AT), passes[0]);
        let casters = target(passes[0] + PASS_CASTERS_CALL_AT);
        // The cascade rotation's sites.
        let (heads, latches, ranges, copies, clears) = (
            find(layout.head),
            find(layout.latch),
            find(RANGE_PATTERN),
            find(FIT_COPY_PATTERN),
            find(layout.clear),
        );
        assert_eq!(
            (
                heads.len(),
                latches.len(),
                ranges.len(),
                copies.len(),
                clears.len()
            ),
            (1, 1, 1, 1, 1)
        );
        let inside = |at: usize| at > casters && at < casters + CASTERS_SIZE;
        assert!(inside(heads[0]) && inside(latches[0]) && inside(ranges[0]));
        // The fits are copied after the loop, from the slots the skip path
        // empties.
        assert!(copies[0] > latches[0] && inside(copies[0]));
        // The band call that the padding hooks: once, inside the loop.
        let pads = find(PAD_PATTERN);
        assert_eq!(pads.len(), 1);
        assert!(heads[0] < pads[0] && pads[0] < latches[0]);
        // The caster pass reads the splits' inputs and writes the matrices
        // at the layout's offsets.
        let body = &image[casters..casters + CASTERS_SIZE];
        for offset in [layout.near, layout.far, layout.distance, layout.matrices] {
            let disp = (offset as u32).to_le_bytes();
            assert!(
                body.windows(4).any(|w| w == disp),
                "the caster pass uses {offset:#x}"
            );
        }
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
