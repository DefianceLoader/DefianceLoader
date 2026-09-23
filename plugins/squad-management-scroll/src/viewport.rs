//! UI-independent viewport over a fixed number of widgets. Never retains
//! pointers into game vectors.
//!
//! The weapon and ammunition rows each show six cards; the upgrade columns and
//! the perk pick row show five widgets. `visible` is per instance so one type
//! serves all of them.
pub const VISIBLE: usize = 6;
pub const UPGRADE_VISIBLE: usize = 5;
pub const PERK_VISIBLE: usize = 5;

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub offset: usize,
    pub count: usize,
    pub visible: usize,
    remainder: i32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            offset: 0,
            count: 0,
            visible: VISIBLE,
            remainder: 0,
        }
    }
}

impl Viewport {
    pub fn new(visible: usize) -> Self {
        Self {
            visible,
            ..Self::default()
        }
    }
    pub fn maximum(self) -> usize {
        self.count.saturating_sub(self.visible)
    }
    pub fn resize(&mut self, count: usize) {
        self.count = count;
        self.offset = self.offset.min(self.maximum());
        if self.maximum() == 0 {
            self.remainder = 0;
        }
    }
    pub fn seek(&mut self, value: i32) {
        self.offset = (value.max(0) as usize).min(self.maximum());
        self.remainder = 0;
    }
    pub fn wheel(&mut self, delta: i16) {
        if self.maximum() == 0 {
            return;
        }
        // High-resolution wheels may send less than one Windows wheel tick.
        self.remainder += i32::from(delta);
        let steps = self.remainder / 120;
        self.remainder %= 120;
        self.offset = (self.offset as i32 - steps).clamp(0, self.maximum() as i32) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_and_shrink() {
        let mut v = Viewport::default();
        for count in [0, 1, 6] {
            v.resize(count);
            v.wheel(-120);
            assert_eq!(v.offset, 0);
        }
        v.resize(7);
        v.wheel(-120);
        assert_eq!(v.offset, 1);
        v.wheel(-120);
        assert_eq!(v.offset, 1);
        v.resize(80);
        v.seek(74);
        assert_eq!(v.offset, 74);
        v.resize(9);
        assert_eq!(v.offset, 3);
        v.resize(0);
        assert_eq!(v.offset, 0);
    }
    #[test]
    fn high_resolution_and_direction_changes() {
        let mut v = Viewport::default();
        v.resize(20);
        v.wheel(-60);
        assert_eq!(v.offset, 0);
        v.wheel(-60);
        assert_eq!(v.offset, 1);
        v.wheel(240);
        assert_eq!(v.offset, 0);
        v.wheel(-60);
        v.wheel(60);
        v.wheel(-120);
        assert_eq!(v.offset, 1);
        v.seek(100);
        assert_eq!(v.offset, 14);
        v.seek(-1);
        assert_eq!(v.offset, 0);
    }
    #[test]
    fn viewports_are_independent_and_resettable() {
        let mut pair = [Viewport::default(); 2];
        pair[0].resize(12);
        pair[1].resize(16);
        pair[0].wheel(-240);
        pair[1].wheel(-120);
        assert_eq!([pair[0].offset, pair[1].offset], [2, 1]);
        pair = [Viewport::default(); 2];
        assert_eq!([pair[0].offset, pair[1].offset], [0, 0]);
    }
    #[test]
    fn the_perk_row_keeps_five_visible() {
        let mut v = Viewport::new(PERK_VISIBLE);
        v.resize(5);
        v.wheel(-120);
        assert_eq!(v.offset, 0);
        v.resize(9);
        v.wheel(-120);
        assert_eq!(v.offset, 1);
        v.seek(100);
        assert_eq!(v.offset, 4);
        v.resize(6);
        assert_eq!(v.offset, 1);
    }
}
