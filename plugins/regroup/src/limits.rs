//! Configured policy is separate from the installed UI capability.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub soldiers: usize,
    pub weapon_types: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            soldiers: 16,
            weapon_types: 0,
        }
    }
}
impl Limits {
    pub fn new(soldiers: i64, weapon_types: i64) -> Result<Self, &'static str> {
        if !(1..=64).contains(&soldiers) {
            return Err("max_soldiers must be 1..64");
        }
        if !(0..=126).contains(&weapon_types) {
            return Err("max_weapon_types must be 0..126 (0 = automatic)");
        }
        Ok(Self {
            soldiers: soldiers as usize,
            weapon_types: weapon_types as usize,
        })
    }
    /// Command dispatch still has Entity*[20] locals. Keep larger configured
    /// values readable for compatibility, but never create such squads.
    pub fn supported(self) -> Self {
        Self {
            soldiers: self.soldiers.min(20),
            ..self
        }
    }
    pub fn ammo(self, installed: u32) -> usize {
        let installed = installed.clamp(9, 126) as usize;
        if self.weapon_types == 0 {
            installed
        } else {
            self.weapon_types.min(installed)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_buffer_ceiling_preserves_lower_limits_and_ammo_settings() {
        for count in 1..=64 {
            let configured = Limits::new(count, 36).unwrap();
            let effective = configured.supported();
            assert_eq!(effective.soldiers, (count as usize).min(20));
            assert_eq!(effective.weapon_types, 36);
        }
    }
    #[test]
    fn validates_configuration_and_uses_installed_capacity() {
        for soldiers in [0, 65, -1] {
            assert!(Limits::new(soldiers, 0).is_err());
        }
        for ammo in [-1, 127] {
            assert!(Limits::new(16, ammo).is_err());
        }
        for soldiers in [1, 16, 17, 64] {
            for capacity in [9, 12, 36, 126] {
                let automatic = Limits::new(soldiers, 0).unwrap();
                assert_eq!(automatic.soldiers, soldiers as usize);
                assert_eq!(automatic.ammo(capacity), capacity as usize);
                for requested in [1, 8, 9, 10, 36, 126] {
                    assert_eq!(
                        Limits::new(soldiers, requested).unwrap().ammo(capacity),
                        (requested as usize).min(capacity as usize)
                    );
                }
            }
        }
    }
}
