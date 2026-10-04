//! Passenger source-gun choice for bound mounts.
//!
//! A mount fires whichever passenger gun is bound to it. `Gun::vfunc_56`
//! refuses shots from a gun whose description has `shot_mode` 1 or 2 while
//! the mount's owner moves, and `Gun::vfunc_59` refuses targets outside the
//! ammunition's `[min, max]` range. So a moving vehicle keeps a sniper's rifle
//! silent, and an explicit Attack inside the rifle's minimum range never
//! fires. This module rebinds the mount to another gun of the same passenger
//! that can fire: unbind (`Gunner` slot `+0x150`) returns the mount's magazine
//! to the old source, and bind (`+0x148`) moves the new source's magazine in.
//!
//! The mount's target (`Gun+0xc8`) is not part of the binding, so a rebind
//! keeps it.

use super::ObjectGetter;
use super::{
    automatic_intent, clone_owned, discard, passenger_source, q, read_u32, set_target,
    virtual_method, weak_object, GUNNER_GUNS_BEGIN, GUNNER_GUNS_END, GUNNER_GUN_LIMIT,
    GUNNER_SOURCES_BEGIN, GUNNER_SOURCES_END, GUN_TARGET,
};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The loader's `Api::log`, stored by [`super::configure`].
pub(super) static LOG: AtomicUsize = AtomicUsize::new(0);

const GUN_OWNER: usize = 0x18;
const GUN_DESCRIPTION: usize = 0x40;
const GUN_AMMO: usize = 0x50;
const DESCRIPTION_SHOT_MODE: usize = 0xc4;
const DESCRIPTION_GRENADE: usize = 0xa9;
const DESCRIPTION_WEAPON_TYPE: usize = 0x108;
const AMMO_MAX_RANGE: usize = 0x1b0;
const AMMO_MIN_RANGE: usize = 0x1b4;
const GUNNER_TARGET: usize = 0x98;
const GUNNER_BIND: usize = 0x148;
const GUNNER_UNBIND: usize = 0x150;
const VEHICLE_GUNNERS_BEGIN: usize = 0x208;
const VEHICLE_GUNNERS_END: usize = 0x210;
/// `Gun::vfunc_56` skips its moving check when the owner reports this flag.
const OWNER_FIRES_WHILE_MOVING: u32 = 0x2000;
const ROSTER_LIMIT: usize = 64;

/// How often one mount re-evaluates its binding.
const CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// The least time between two rebinds of one mount, unless its vehicle
/// changed between moving and stopped since.
const REBIND_DWELL: Duration = Duration::from_millis(1500);
/// How long a stopped vehicle still counts as moving; brief stops in traffic
/// keep the moving gun bound.
const STOP_DWELL: Duration = Duration::from_secs(1);
/// How long a stopped vehicle must keep moving before it counts as moving; a
/// vehicle settling after a stop creeps for a moment.
const START_DWELL: Duration = Duration::from_millis(300);
/// The least time between two changes of a vehicle's moving state, so short
/// hops do not switch guns faster than this.
const MOTION_HOLD: Duration = Duration::from_millis(1500);
/// Vehicles whose motion is tracked at once; more prunes stale entries.
const MOTION_LIMIT: usize = 256;

type FlagTest = unsafe extern "system" fn(usize, u32) -> u8;
type Speed = unsafe extern "system" fn(usize) -> f32;
type Position = unsafe extern "system" fn(usize) -> *const f32;
type Binding = unsafe extern "system" fn(usize, usize) -> u8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Candidate {
    pub gun: usize,
    /// A nonzero `weapon_type`; the vehicle's rebalance prefers these too.
    pub special: bool,
    /// `shot_mode` 1 or 2: `Gun::vfunc_56` refuses it while moving.
    pub stationary_only: bool,
    /// `(min, max)` ammunition range.
    pub range: Option<(f32, f32)>,
}

/// The gun a mount should carry, or `None` to keep `current`. Usable guns can
/// fire now: not stationary-only while moving, and with `distance` (an explicit
/// target's) inside their range. A special gun outranks the current one, which
/// outranks the rest, so an unusable current gun yields to any usable gun.
pub(super) fn choose(
    current: usize,
    candidates: &[Candidate],
    moving: bool,
    distance: Option<f32>,
) -> Option<usize> {
    let usable = |candidate: &Candidate| {
        !(moving && candidate.stationary_only)
            && distance.is_none_or(|distance| {
                candidate
                    .range
                    .is_some_and(|(min, max)| distance >= min && distance <= max)
            })
    };
    let mut best: Option<(&Candidate, (bool, bool))> = None;
    for candidate in candidates.iter().filter(|candidate| usable(candidate)) {
        let rank = (candidate.special, candidate.gun == current);
        if best.is_none_or(|(_, best_rank)| rank > best_rank) {
            best = Some((candidate, rank));
        }
    }
    best.map(|(candidate, _)| candidate.gun)
        .filter(|&gun| gun != current)
}

#[derive(Debug, PartialEq)]
pub(super) enum Exchange {
    /// Unbind refused; the old gun is still bound.
    Unchanged,
    Rebound,
    /// Bind refused the new gun; the old gun is bound again.
    Restored,
    /// Neither gun could be bound; the mount is empty.
    Lost,
}

/// Unbind first, so the mount's magazine returns to `old`, then bind `new`.
/// Never unbind without a source: that discards the magazine.
pub(super) fn exchange(
    old: usize,
    new: usize,
    mut unbind: impl FnMut(usize) -> bool,
    mut bind: impl FnMut(usize) -> bool,
) -> Exchange {
    if old == 0 || new == 0 || !unbind(old) {
        Exchange::Unchanged
    } else if bind(new) {
        Exchange::Rebound
    } else if bind(old) {
        Exchange::Restored
    } else {
        Exchange::Lost
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MountState {
    checked: Option<Instant>,
    rebound: Option<Instant>,
    /// The vehicle's [`Motion::generation`] at the last evaluation.
    seen: u32,
}

impl MountState {
    /// Whether this mount evaluates now; `forced` skips the check interval.
    pub(super) fn due(&mut self, now: Instant, forced: bool) -> bool {
        let due = forced
            || self
                .checked
                .is_none_or(|at| now.saturating_duration_since(at) >= CHECK_INTERVAL);
        if due {
            self.checked = Some(now);
        }
        due
    }

    pub(super) fn may_rebind(&self, now: Instant) -> bool {
        self.rebound
            .is_none_or(|at| now.saturating_duration_since(at) >= REBIND_DWELL)
    }

    pub(super) fn rebound_at(&mut self, now: Instant) {
        self.rebound = Some(now);
    }
}

/// One vehicle's moving state, shared by all its mounts so its passengers
/// switch guns together. Every mount adds a sample each tick.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Motion {
    /// The first sample of the current run of moving samples.
    since: Option<Instant>,
    /// The last moving sample.
    last: Option<Instant>,
    /// The last change of `moving`.
    changed: Option<Instant>,
    moving: bool,
    /// Counts changes of `moving`; a mount that sees it change evaluates at
    /// once.
    generation: u32,
}

impl Motion {
    /// Moving once motion has lasted [`START_DWELL`] without a still sample,
    /// and until [`STOP_DWELL`] after the last moving sample. The state holds
    /// at least [`MOTION_HOLD`] after each change. Returns the state and its
    /// generation.
    pub(super) fn sample(&mut self, now: Instant, moving_now: bool) -> (bool, u32) {
        let held = self
            .changed
            .is_some_and(|at| now.saturating_duration_since(at) < MOTION_HOLD);
        if moving_now {
            let since = *self.since.get_or_insert(now);
            self.last = Some(now);
            if !self.moving && !held && now.saturating_duration_since(since) >= START_DWELL {
                self.change(now);
            }
        } else if !self.moving {
            self.since = None;
        } else if !held
            && self
                .last
                .is_none_or(|last| now.saturating_duration_since(last) >= STOP_DWELL)
        {
            self.change(now);
            self.since = None;
        }
        (self.moving, self.generation)
    }

    fn change(&mut self, now: Instant) {
        self.moving = !self.moving;
        self.changed = Some(now);
        self.generation = self.generation.wrapping_add(1);
    }
}

static MOTION: OnceLock<Mutex<HashMap<usize, Motion>>> = OnceLock::new();

/// Samples `owner`'s motion; see [`Motion::sample`].
fn vehicle_moving(owner: usize, now: Instant, moving_now: bool) -> (bool, u32) {
    let Ok(mut motion) = MOTION.get_or_init(|| Mutex::new(HashMap::new())).lock() else {
        return (moving_now, 0);
    };
    if motion.len() >= MOTION_LIMIT && !motion.contains_key(&owner) {
        motion.retain(|_, state| {
            state
                .last
                .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(10))
        });
        if motion.len() >= MOTION_LIMIT {
            return (moving_now, 0);
        }
    }
    motion.entry(owner).or_default().sample(now, moving_now)
}

static MOUNTS: OnceLock<Mutex<HashMap<usize, MountState>>> = OnceLock::new();

static FORCED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Re-evaluate `gunner` on its next tick, skipping the check interval but not
/// the rebind dwell. A new explicit order calls this.
pub(super) fn force(gunner: usize) {
    if let Ok(mut forced) = FORCED.lock() {
        if !forced.contains(&gunner) && forced.len() < 256 {
            forced.push(gunner);
        }
    }
}

/// Re-evaluate every bound passenger mount of `gunner`.
pub(super) unsafe fn update(gunner: usize) {
    if gunner == 0 {
        return;
    }
    let forced = FORCED.lock().is_ok_and(|mut forced| {
        let before = forced.len();
        forced.retain(|&owner| owner != gunner);
        forced.len() != before
    });
    let begin = unsafe { q(gunner, GUNNER_GUNS_BEGIN) };
    let end = unsafe { q(gunner, GUNNER_GUNS_END) };
    let Some(count) = end
        .checked_sub(begin)
        .filter(|bytes| begin != 0 && bytes % 8 == 0)
        .map(|bytes| bytes / 8)
        .filter(|&count| count <= GUNNER_GUN_LIMIT)
    else {
        return;
    };
    let guns: Vec<usize> = (0..count)
        .map(|index| unsafe { q(begin + index * 8, 0) })
        .collect();
    let now = Instant::now();
    for gun in guns {
        if gun == 0 {
            continue;
        }
        let Some(source) = (unsafe { passenger_source(gunner, gun) }) else {
            continue;
        };
        let Some(mut state) = take_state(gun, now) else {
            continue;
        };
        let owner = unsafe { weak_object(q(gun, GUN_OWNER)) };
        let (moving, generation) = vehicle_moving(owner, now, unsafe { owner_moving(gun) });
        let changed = generation != state.seen;
        state.seen = generation;
        if state.due(now, forced || changed) && (changed || state.may_rebind(now)) {
            unsafe { evaluate(gunner, gun, owner, source, moving, &mut state, now) };
        }
        put_state(gun, state);
    }
}

fn take_state(gun: usize, now: Instant) -> Option<MountState> {
    let mut mounts = MOUNTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()?;
    if mounts.len() >= 1024 {
        mounts.retain(|_, state| {
            state
                .checked
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(10))
        });
    }
    Some(mounts.get(&gun).copied().unwrap_or_default())
}

fn put_state(gun: usize, state: MountState) {
    if let Some(Ok(mut mounts)) = MOUNTS.get().map(Mutex::lock) {
        if mounts.len() < 1024 || mounts.contains_key(&gun) {
            mounts.insert(gun, state);
        }
    }
}

unsafe fn evaluate(
    gunner: usize,
    gun: usize,
    owner: usize,
    source: usize,
    moving: bool,
    state: &mut MountState,
    now: Instant,
) {
    let explicit = !unsafe { automatic_intent(gunner) };
    let distance = if explicit {
        unsafe { target_distance(gun, q(gunner, GUNNER_TARGET)) }
    } else {
        None
    };
    let vehicle_ai = unsafe { facet(owner, 0x28) };
    let candidates = unsafe { candidates(source, vehicle_ai) };
    let Some(new) = choose(source, &candidates, moving, distance) else {
        return;
    };
    let unbind = unsafe { virtual_method(gunner, GUNNER_UNBIND) };
    let bind = unsafe { virtual_method(gunner, GUNNER_BIND) };
    if unbind == 0 || bind == 0 {
        return;
    }
    let unbind = unsafe { std::mem::transmute::<usize, Binding>(unbind) };
    let bind = unsafe { std::mem::transmute::<usize, Binding>(bind) };
    let result = exchange(
        source,
        new,
        |old| unsafe { unbind(gunner, old) } != 0,
        |gun| unsafe { bind(gunner, gun) } != 0,
    );
    if result != Exchange::Unchanged {
        state.rebound_at(now);
    }
    let reason = match (moving, distance) {
        (_, Some(distance)) => format!("explicit target at {distance:.0}"),
        (true, None) => "moving".into(),
        (false, None) => "stopped".into(),
    };
    let (old_name, new_name) = unsafe { (gun_name(source), gun_name(new)) };
    match result {
        Exchange::Unchanged => {}
        Exchange::Rebound => {
            log(
                defiance_api::LOG_DEBUG,
                &format!("passenger gun rebind ({reason}): {old_name} -> {new_name}"),
            );
            if explicit {
                unsafe { keep_explicit_target(gunner, new) };
            }
        }
        Exchange::Restored => log(
            defiance_api::LOG_DEBUG,
            &format!("passenger gun rebind ({reason}) refused {new_name}; kept {old_name}"),
        ),
        Exchange::Lost => log(
            defiance_api::LOG_WARN,
            &format!("passenger gun rebind ({reason}): neither {new_name} nor {old_name} rebound"),
        ),
    }
}

/// `Gun::vfunc_56`'s moving test for the mount's owner.
unsafe fn owner_moving(gun: usize) -> bool {
    let owner = unsafe { weak_object(q(gun, GUN_OWNER)) };
    let flags = unsafe { virtual_method(owner, 0x98) };
    if flags == 0
        || unsafe { std::mem::transmute::<usize, FlagTest>(flags)(owner, OWNER_FIRES_WHILE_MOVING) }
            != 0
    {
        return false;
    }
    let movement = unsafe { facet(owner, 0x38) };
    let speed = unsafe { virtual_method(movement, 0x60) };
    speed != 0 && unsafe { std::mem::transmute::<usize, Speed>(speed)(movement) } > 0.0
}

/// A field of `entity`'s facet block (`vfunc_0xb0`).
unsafe fn facet(entity: usize, offset: usize) -> usize {
    let getter = unsafe { virtual_method(entity, 0xb0) };
    if getter == 0 {
        return 0;
    }
    let facets = unsafe { std::mem::transmute::<usize, ObjectGetter>(getter)(entity) };
    if facets == 0 {
        0
    } else {
        unsafe { q(facets, offset) }
    }
}

/// `Gun::vfunc_59`'s 2D distance from the mount's owner to a target handle.
unsafe fn target_distance(gun: usize, target: usize) -> Option<f32> {
    let from = unsafe { position(weak_object(q(gun, GUN_OWNER))) }?;
    let to = unsafe { position(weak_object(target)) }?;
    let (dx, dy) = (from[0] - to[0], from[1] - to[1]);
    Some((dx * dx + dy * dy).sqrt())
}

unsafe fn position(entity: usize) -> Option<[f32; 2]> {
    let getter = unsafe { virtual_method(entity, 0x28) };
    if getter == 0 {
        return None;
    }
    let at = unsafe { std::mem::transmute::<usize, Position>(getter)(entity) };
    if at.is_null() {
        return None;
    }
    Some(unsafe { [at.read_unaligned(), at.add(1).read_unaligned()] })
}

/// `source`'s passenger guns that a mount can carry: the same class (bind,
/// `Gun::vfunc_11`, dereferences null for a source of another class), with
/// ammunition, no grenades, and not bound to another mount of the vehicle. The roster path
/// matches `vehicle_special_fire_roster_gun` in `patch/vehicle-priority-fire.asm`.
unsafe fn candidates(source: usize, vehicle_ai: usize) -> Vec<Candidate> {
    let passenger = unsafe { weak_object(q(source, GUN_OWNER)) };
    let ai = unsafe { facet(passenger, 0x28) };
    if ai == 0 {
        return Vec::new();
    }
    let inventory = unsafe { q(ai, 0x1f0) };
    let weapons = if inventory == 0 {
        0
    } else {
        unsafe { q(inventory, 0x10) }
    };
    let Some(roster) = (unsafe { pointers(weapons, 0x38, ROSTER_LIMIT) }) else {
        return Vec::new();
    };
    let class = unsafe { q(source, 0) };
    roster
        .into_iter()
        .filter(|&gun| gun != 0 && unsafe { q(gun, 0) } == class)
        .filter(|&gun| gun == source || !unsafe { bound_in_vehicle(vehicle_ai, gun) })
        .filter_map(|gun| unsafe { candidate(gun) })
        .collect()
}

unsafe fn candidate(gun: usize) -> Option<Candidate> {
    let description = unsafe { q(gun, GUN_DESCRIPTION) };
    if description == 0 || unsafe { *((description + DESCRIPTION_GRENADE) as *const u8) } != 0 {
        return None;
    }
    let ammo = unsafe { q(gun, GUN_AMMO) };
    if ammo == 0 {
        return None;
    }
    let range = unsafe {
        Some((
            ((ammo + AMMO_MIN_RANGE) as *const f32).read_unaligned(),
            ((ammo + AMMO_MAX_RANGE) as *const f32).read_unaligned(),
        ))
    };
    Some(Candidate {
        gun,
        special: unsafe { read_u32(description, DESCRIPTION_WEAPON_TYPE) } != 0,
        stationary_only: unsafe { read_u32(description, DESCRIPTION_SHOT_MODE) }.wrapping_sub(1)
            < 2,
        range,
    })
}

unsafe fn bound_in_vehicle(vehicle_ai: usize, gun: usize) -> bool {
    // An unreadable vehicle cannot prove the gun free.
    let Some(gunners) = (unsafe { pointers(vehicle_ai, VEHICLE_GUNNERS_BEGIN, GUNNER_GUN_LIMIT) })
    else {
        return true;
    };
    debug_assert_eq!(VEHICLE_GUNNERS_END, VEHICLE_GUNNERS_BEGIN + 8);
    gunners
        .into_iter()
        .filter(|&gunner| gunner != 0)
        .any(|gunner| {
            unsafe { pointers(gunner, GUNNER_SOURCES_BEGIN, GUNNER_GUN_LIMIT) }
                .is_none_or(|sources| sources.contains(&gun))
        })
}

/// A bounded pointer vector at `owner + offset` (begin, end).
unsafe fn pointers(owner: usize, offset: usize, limit: usize) -> Option<Vec<usize>> {
    if owner == 0 {
        return None;
    }
    let begin = unsafe { q(owner, offset) };
    let bytes = unsafe { q(owner, offset + 8) }.checked_sub(begin)?;
    if bytes % 8 != 0 || bytes / 8 > limit || (begin == 0 && bytes != 0) {
        return None;
    }
    Some(
        (0..bytes / 8)
            .map(|index| unsafe { q(begin + index * 8, 0) })
            .collect(),
    )
}

/// Bind can choose a different empty mount than the one unbound. Give that
/// mount the explicit order's target if it has none.
unsafe fn keep_explicit_target(gunner: usize, source: usize) {
    let shared = unsafe { q(gunner, GUNNER_TARGET) };
    if shared == 0 || unsafe { weak_object(shared) } == 0 {
        return;
    }
    let (Some(guns), Some(sources)) = (unsafe {
        (
            pointers(gunner, GUNNER_GUNS_BEGIN, GUNNER_GUN_LIMIT),
            pointers(gunner, GUNNER_SOURCES_BEGIN, GUNNER_GUN_LIMIT),
        )
    }) else {
        return;
    };
    debug_assert_eq!(GUNNER_GUNS_END, GUNNER_GUNS_BEGIN + 8);
    debug_assert_eq!(GUNNER_SOURCES_END, GUNNER_SOURCES_BEGIN + 8);
    let Some(mount) = guns
        .iter()
        .zip(&sources)
        .find(|(_, &bound)| bound == source)
        .map(|(&gun, _)| gun)
    else {
        return;
    };
    if mount == 0 || unsafe { q(mount, GUN_TARGET) } != 0 {
        return;
    }
    let mut target = unsafe { clone_owned(shared) };
    if !unsafe { set_target(mount, &mut target) } {
        unsafe { discard(&mut target) };
    }
}

/// The description's name (an MSVC `std::string` at `+8`).
unsafe fn gun_name(gun: usize) -> String {
    let description = unsafe { q(gun, GUN_DESCRIPTION) };
    if description == 0 {
        return format!("{gun:#x}");
    }
    let length = unsafe { q(description, 0x18) };
    let capacity = unsafe { q(description, 0x20) };
    if length == 0 || length > 256 || capacity < length {
        return format!("{gun:#x}");
    }
    let data = if capacity > 15 {
        unsafe { q(description, 8) }
    } else {
        description + 8
    };
    if data == 0 {
        return format!("{gun:#x}");
    }
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, length) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn log(level: u32, message: &str) {
    let log = LOG.load(Ordering::Acquire);
    if log == 0 {
        return;
    }
    if let Ok(text) = CString::new(message) {
        let log = unsafe {
            std::mem::transmute::<usize, unsafe extern "C" fn(u32, *const std::ffi::c_char)>(log)
        };
        unsafe { log(level, text.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gun(gun: usize, special: bool, stationary_only: bool, range: (f32, f32)) -> Candidate {
        Candidate {
            gun,
            special,
            stationary_only,
            range: Some(range),
        }
    }

    const RIFLE: usize = 1;
    const SMG: usize = 2;

    fn sniper() -> [Candidate; 2] {
        [
            gun(RIFLE, true, true, (60.0, 400.0)),
            gun(SMG, false, false, (0.0, 120.0)),
        ]
    }

    #[test]
    fn moving_swaps_a_stationary_rifle_for_the_smg_and_stopping_restores_it() {
        let guns = sniper();
        assert_eq!(choose(RIFLE, &guns, true, None), Some(SMG));
        assert_eq!(choose(SMG, &guns, true, None), None);
        assert_eq!(choose(SMG, &guns, false, None), Some(RIFLE));
        assert_eq!(choose(RIFLE, &guns, false, None), None);
    }

    #[test]
    fn explicit_targets_choose_the_gun_whose_range_includes_them() {
        let guns = sniper();
        // Inside the rifle's minimum range.
        assert_eq!(choose(RIFLE, &guns, false, Some(30.0)), Some(SMG));
        // Both reach it: the special wins.
        assert_eq!(choose(SMG, &guns, false, Some(100.0)), Some(RIFLE));
        // Only the rifle reaches it, but it cannot fire while moving.
        assert_eq!(choose(SMG, &guns, true, Some(300.0)), None);
        // Nothing reaches it: keep the binding.
        assert_eq!(choose(RIFLE, &guns, false, Some(900.0)), None);
    }

    #[test]
    fn equal_guns_keep_the_current_binding() {
        let guns = [
            gun(SMG, false, false, (0.0, 100.0)),
            gun(3, false, false, (0.0, 100.0)),
        ];
        assert_eq!(choose(3, &guns, true, None), None);
        assert_eq!(choose(SMG, &guns, false, Some(50.0)), None);
        // A passenger with only stationary guns keeps its binding.
        let rifles = [gun(RIFLE, true, true, (60.0, 400.0))];
        assert_eq!(choose(RIFLE, &rifles, true, None), None);
    }

    #[test]
    fn exchange_unbinds_before_binding_and_restores_on_refusal() {
        let calls = std::cell::RefCell::new(Vec::new());
        let result = exchange(
            RIFLE,
            SMG,
            |gun| {
                calls.borrow_mut().push(("unbind", gun));
                true
            },
            |gun| {
                calls.borrow_mut().push(("bind", gun));
                true
            },
        );
        assert_eq!(result, Exchange::Rebound);
        assert_eq!(*calls.borrow(), [("unbind", RIFLE), ("bind", SMG)]);

        let mut binds = Vec::new();
        let result = exchange(
            RIFLE,
            SMG,
            |_| true,
            |gun| {
                binds.push(gun);
                gun == RIFLE
            },
        );
        assert_eq!(result, Exchange::Restored);
        assert_eq!(binds, [SMG, RIFLE]);

        assert_eq!(exchange(RIFLE, SMG, |_| true, |_| false), Exchange::Lost);
        let mut bound = false;
        let refused = exchange(
            RIFLE,
            SMG,
            |_| false,
            |_| {
                bound = true;
                true
            },
        );
        assert_eq!(refused, Exchange::Unchanged);
        assert!(!bound);
        // Never unbind without a source.
        assert_eq!(
            exchange(0, SMG, |_| panic!("null unbind"), |_| true),
            Exchange::Unchanged
        );
    }

    #[test]
    fn mount_state_throttles_and_dwells() {
        let start = Instant::now();
        let mut state = MountState::default();
        assert!(state.due(start, false));
        assert!(!state.due(start + Duration::from_millis(100), false));
        assert!(state.due(start + Duration::from_millis(100), true));
        assert!(state.due(start + Duration::from_millis(400), false));

        assert!(state.may_rebind(start));
        state.rebound_at(start);
        assert!(!state.may_rebind(start + Duration::from_secs(1)));
        assert!(state.may_rebind(start + Duration::from_secs(2)));
    }

    #[test]
    fn motion_ignores_creeps_holds_through_brief_stops_and_limits_changes() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut motion = Motion::default();
        assert_eq!(motion.sample(at(0), false), (false, 0));

        // A creep that a still sample interrupts never counts.
        assert_eq!(motion.sample(at(100), true), (false, 0));
        assert_eq!(motion.sample(at(200), false), (false, 0));
        assert_eq!(motion.sample(at(450), true), (false, 0));

        // Motion that lasts the start dwell counts, and holds through a stop
        // shorter than the stop dwell.
        assert_eq!(motion.sample(at(750), true), (true, 1));
        assert_eq!(motion.sample(at(1000), false), (true, 1));
        assert_eq!(motion.sample(at(1700), false), (true, 1));

        // The stop dwell has passed, but the state holds after a change.
        assert_eq!(motion.sample(at(2000), false), (true, 1));
        assert_eq!(motion.sample(at(2250), false), (false, 2));

        // A hop during the hold does not count, even past the start dwell.
        assert_eq!(motion.sample(at(2400), true), (false, 2));
        assert_eq!(motion.sample(at(2800), true), (false, 2));
        assert_eq!(motion.sample(at(3000), false), (false, 2));

        // After the hold, motion must last the start dwell again.
        assert_eq!(motion.sample(at(3500), true), (false, 2));
        assert_eq!(motion.sample(at(3800), true), (true, 3));
    }

    #[repr(C)]
    struct Owner {
        table: *const usize,
        flag: u8,
        facets: *const usize,
    }

    unsafe extern "system" fn owner_flags(owner: usize, flag: u32) -> u8 {
        assert_eq!(flag, OWNER_FIRES_WHILE_MOVING);
        unsafe { (*(owner as *const Owner)).flag }
    }

    unsafe extern "system" fn owner_facets(owner: usize) -> usize {
        unsafe { (*(owner as *const Owner)).facets as usize }
    }

    unsafe extern "system" fn movement_speed(movement: usize) -> f32 {
        unsafe { *((movement + 8) as *const f32) }
    }

    #[test]
    fn owner_moving_mirrors_the_native_shot_gate() {
        let mut owner_table = [0usize; 0xb8 / 8];
        owner_table[0x98 / 8] = owner_flags as *const () as usize;
        owner_table[0xb0 / 8] = owner_facets as *const () as usize;
        let mut movement_table = [0usize; 0x68 / 8];
        movement_table[0x60 / 8] = movement_speed as *const () as usize;
        let mut movement = [movement_table.as_ptr() as usize, 0];
        let movement = movement.as_mut_ptr();
        let mut facets = [0usize; 8];
        facets[0x38 / 8] = movement as usize;
        let mut owner = Owner {
            table: owner_table.as_ptr(),
            flag: 0,
            facets: facets.as_ptr(),
        };
        let owner = &mut owner as *mut Owner;
        let handle = [0usize, 0, owner as usize];
        let mut mount = [0usize; 4];
        mount[GUN_OWNER / 8] = handle.as_ptr() as usize;
        let gun = mount.as_ptr() as usize;

        let set_speed = |speed: f32| unsafe { movement.add(1).write(speed.to_bits() as usize) };
        set_speed(3.0);
        assert!(unsafe { owner_moving(gun) });
        set_speed(0.0);
        assert!(!unsafe { owner_moving(gun) });
        set_speed(3.0);
        unsafe { (*owner).flag = 1 };
        assert!(!unsafe { owner_moving(gun) });
    }
}
