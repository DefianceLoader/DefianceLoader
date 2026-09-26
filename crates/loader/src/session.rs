//! Whether a mission is loaded or being built, as Core reports it, and the gate
//! that keeps hot reload out of those transitions.
//!
//! Core hooks the construction and destruction of the game's tactical state
//! (`TacticalMapGameState`) and reports each through the loader's `session`
//! service ([`API`]): `before_mission` as a state is about to be built (a
//! mission starting, a save loading), `mission(1)` once its constructor has
//! returned, and `mission(-1)` once its destructor has returned. So a state is
//! counted from the moment its construction starts until its destruction has
//! finished, and a slow constructor or destructor never looks like the menu.
//!
//! Hot reload runs only at the menu: tracked, no mission loaded and none being
//! built. The watcher decides and reloads inside [`Gate::while_at_menu`], and
//! `before_mission` marks a construction inside the same lock, so a mission
//! cannot start between the decision and the reload: its construction waits
//! for the reload to finish, then runs the new plugins. Pending reloads are
//! also applied synchronously in `before_mission` (`reload::before_mission`),
//! on the game thread, so a mission started or a save loaded runs them.
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;

/// The mission state and the lock that serializes reload decisions with the
/// start of a construction.
pub struct Gate {
    lock: Mutex<()>,
    tracking: AtomicBool,
    missions: AtomicI32,
    /// States whose construction has started and not yet returned.
    building: AtomicI32,
}

impl Gate {
    pub const fn new() -> Self {
        Gate {
            lock: Mutex::new(()),
            tracking: AtomicBool::new(false),
            missions: AtomicI32::new(0),
            building: AtomicI32::new(0),
        }
    }

    pub fn track(&self) {
        self.tracking.store(true, Ordering::Release);
    }

    pub fn tracking(&self) -> bool {
        self.tracking.load(Ordering::Acquire)
    }

    pub fn in_mission(&self) -> bool {
        self.missions.load(Ordering::Acquire) > 0 || self.building.load(Ordering::Acquire) > 0
    }

    /// Tracked, and no mission loaded or being built.
    pub fn at_menu(&self) -> bool {
        self.tracking() && !self.in_mission()
    }

    /// Run `apply` if it is at the menu, deciding and running inside the lock
    /// a construction's start takes too. Returns whether it ran.
    pub fn while_at_menu(&self, apply: impl FnOnce()) -> bool {
        let _gate = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if !self.at_menu() {
            return false;
        }
        apply();
        true
    }

    /// A state's construction starts: mark it, inside the lock (waiting for a
    /// reload in progress), and run `apply` there first.
    pub fn begin_construction(&self, apply: impl FnOnce()) {
        let _gate = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.building.fetch_add(1, Ordering::AcqRel);
        apply();
    }

    /// A constructor returned (`delta` > 0) or a destructor returned (< 0).
    /// Returns the loaded missions before the change.
    pub fn mission(&self, delta: i32) -> i32 {
        if delta > 0 {
            let _ = self
                .building
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    Some((n - 1).max(0))
                });
        }
        self.missions.fetch_add(delta.signum(), Ordering::AcqRel)
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

pub static GATE: Gate = Gate::new();

/// Whether Core reports missions.
pub fn tracking() -> bool {
    GATE.tracking()
}

/// Whether it is safe to reload now: tracked, and no mission loaded or being
/// built. A decision to reload must be made inside [`Gate::while_at_menu`].
pub fn at_menu() -> bool {
    GATE.at_menu()
}

unsafe extern "C" fn service_tracking() {
    GATE.track();
}

unsafe extern "C" fn service_mission(delta: i32) {
    let before = GATE.mission(delta);
    if before == 0 && delta > 0 {
        // `[trace] when = mission`
        crate::trace::start_deferred();
    }
}

unsafe extern "C" fn service_before_mission() {
    GATE.begin_construction(crate::reload::before_mission);
}

/// `defiance.loader` / `session` v1.
pub static API: defiance_api::SessionV1 = defiance_api::SessionV1 {
    tracking: service_tracking,
    mission: service_mission,
    before_mission: service_before_mission,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn a_mission_counts_from_its_construction_to_its_destruction() {
        let gate = Gate::new();
        assert!(!gate.at_menu(), "untracked means unknown");
        gate.track();
        assert!(gate.at_menu());
        // A slow constructor: from `before_mission` until it returns.
        gate.begin_construction(|| {});
        assert!(!gate.at_menu(), "a state being built is not the menu");
        assert!(!gate.while_at_menu(|| panic!("reloaded during a construction")));
        gate.mission(1);
        assert!(!gate.at_menu());
        // A slow destructor: Core reports only once it has returned.
        assert!(!gate.while_at_menu(|| panic!("reloaded during a mission")));
        gate.mission(-1);
        assert!(gate.at_menu());
    }

    #[test]
    fn a_construction_starting_after_the_menu_check_is_not_reloaded_into() {
        // The watcher saw the menu on its polls; a mission then starts before
        // it applies the queue. The decision is made again inside the lock.
        let gate = Gate::new();
        gate.track();
        assert!(gate.at_menu());
        gate.begin_construction(|| {});
        assert!(!gate.while_at_menu(|| panic!("reloaded into a construction")));
    }

    #[test]
    fn a_construction_waits_for_a_reload_in_progress() {
        let gate = Arc::new(Gate::new());
        gate.track();
        let order = Arc::new(AtomicUsize::new(0));
        let (started, reloading) = mpsc::channel();
        let (finish, finished) = mpsc::channel::<()>();
        let watcher = {
            let (gate, order) = (gate.clone(), order.clone());
            std::thread::spawn(move || {
                gate.while_at_menu(|| {
                    started.send(()).unwrap();
                    finished.recv().unwrap();
                    // The reload finishes before the construction proceeds.
                    order
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .unwrap();
                })
            })
        };
        reloading.recv().unwrap();
        let game = {
            let (gate, order) = (gate.clone(), order.clone());
            std::thread::spawn(move || {
                gate.begin_construction(|| {
                    order
                        .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
                        .unwrap();
                })
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(order.load(Ordering::SeqCst), 0, "the construction waited");
        finish.send(()).unwrap();
        assert!(watcher.join().unwrap());
        game.join().unwrap();
        assert_eq!(order.load(Ordering::SeqCst), 2);
        assert!(!gate.at_menu(), "the new state is being built");
    }
}
