//! Planning contains no game pointers or mutations. Every split conserves the
//! pool's current rounds, capacity, reservations and carrier count exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Supply {
    pub capacity: u32,
    pub rounds: u32,
    pub reserved: u32,
    pub carriers: u32,
    pub disabled: u32,
}

impl Supply {
    pub fn split(self, picked: u32, total: u32, loaded: u32) -> Result<(Self, Self), &'static str> {
        if total == 0
            || picked > total
            || loaded > self.reserved
            || self.reserved > self.rounds
            || self.rounds > self.capacity
        {
            return Err("ammunition accounting is inconsistent");
        }
        if (picked == 0 && loaded != 0) || (picked == total && loaded != self.reserved) {
            return Err("loaded rounds do not match selected gun ownership");
        }
        let fraction = |n: u32| ((n as u64 * picked as u64) / total as u64) as u32;
        let rounds = loaded + fraction(self.rounds - self.reserved);
        // Distribute spare capacity separately so both resulting pools can
        // contain their rounds, including unevenly loaded magazines.
        let capacity = rounds + fraction(self.capacity - self.rounds);
        let moved = Self {
            capacity,
            rounds,
            reserved: loaded,
            carriers: fraction(self.carriers),
            disabled: self.disabled,
        };
        let left = Self {
            capacity: self.capacity - capacity,
            rounds: self.rounds - rounds,
            reserved: self.reserved - loaded,
            carriers: self.carriers - moved.carriers,
            disabled: self.disabled,
        };
        Ok((moved, left))
    }

    pub fn combine(self, rhs: Self) -> Result<Self, &'static str> {
        let add = |a: u32, b: u32| a.checked_add(b).ok_or("ammunition count overflow");
        Ok(Self {
            capacity: add(self.capacity, rhs.capacity)?,
            rounds: add(self.rounds, rhs.rounds)?,
            reserved: add(self.reserved, rhs.reserved)?,
            carriers: add(self.carriers, rhs.carriers)?,
            disabled: self.disabled & rhs.disabled,
        })
    }
}

/// Ammo pins are indexed by pool slot. Rebuild them by ammo identity after the
/// native pool sorter changes slot order; never apply a launcher pin to a rifle.
pub fn remap_pins(old: &[usize], new: &[usize], mask: u8, values: u8) -> (u8, u8) {
    let mut out = (0, 0);
    for (i, ammo) in old.iter().take(8).enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        if let Some(j) = new.iter().take(8).position(|a| a == ammo) {
            out.0 |= 1 << j;
            if values & (1 << i) != 0 {
                out.1 |= 1 << j;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_conserves_every_count_with_uneven_magazines() {
        for total in 1..17 {
            for picked in 0..=total {
                let s = Supply {
                    capacity: 500,
                    rounds: 301,
                    reserved: 160,
                    carriers: total,
                    disabled: 1,
                };
                let loaded = 160 * picked / total;
                let (m, l) = s.split(picked, total, loaded).unwrap();
                assert_eq!(m.combine(l).unwrap(), s);
                assert!(m.reserved <= m.rounds && m.rounds <= m.capacity);
                assert!(l.reserved <= l.rounds && l.rounds <= l.capacity);
            }
        }
    }
    #[test]
    fn all_and_none_are_exact() {
        let s = Supply {
            capacity: 101,
            rounds: 83,
            reserved: 39,
            carriers: 3,
            disabled: 0,
        };
        assert_eq!(s.split(3, 3, 39).unwrap(), (s, Supply::default()));
        assert_eq!(s.split(0, 3, 0).unwrap(), (Supply::default(), s));
    }
    #[test]
    fn invalid_reservations_fail_before_mutation() {
        let s = Supply {
            capacity: 50,
            rounds: 20,
            reserved: 10,
            carriers: 2,
            disabled: 0,
        };
        for (n, total, loaded) in [(1, 0, 0), (3, 2, 0), (1, 2, 11), (2, 2, 9), (0, 2, 1)] {
            assert!(s.split(n, total, loaded).is_err());
        }
        assert!(Supply { rounds: 51, ..s }.split(1, 2, 0).is_err());
        assert!(Supply { reserved: 21, ..s }.split(1, 2, 0).is_err());
    }
    #[test]
    fn combines_without_wrapping_and_enables_mixed_slots() {
        let s = Supply {
            capacity: u32::MAX,
            ..Supply::default()
        };
        assert!(s
            .combine(Supply {
                capacity: 1,
                ..Supply::default()
            })
            .is_err());
        assert_eq!(
            Supply {
                disabled: 1,
                ..Supply::default()
            }
            .combine(Supply::default())
            .unwrap()
            .disabled,
            0
        );
    }
    #[test]
    fn pins_follow_ammo_identity_and_drop_missing_slots() {
        assert_eq!(
            remap_pins(&[11, 22, 33], &[33, 11], 0b111, 0b101),
            (0b11, 0b11)
        );
        assert_eq!(remap_pins(&[11, 22], &[22, 11], 0b01, 0), (0b10, 0));
        assert_eq!(remap_pins(&[11], &[22], 1, 1), (0, 0));
        // The byte-sized native pin cannot represent a type sorted into slot 9.
        assert_eq!(
            remap_pins(&[99], &[1, 2, 3, 4, 5, 6, 7, 8, 99], 1, 1),
            (0, 0)
        );
    }
}
