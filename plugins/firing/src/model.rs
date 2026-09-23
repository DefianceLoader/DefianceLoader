//! Decision rules, independent of engine pointers. Pins store value+1; zero
//! inherits the squad's flag. Preserve bytewise OR semantics, not just booleans.
#[derive(Clone, Copy)]
pub struct Member {
    pub selected: bool,
    pub pin: u8,
}
pub fn discriminates(members: &[Member]) -> bool {
    let marked = members.iter().filter(|m| m.selected).count();
    marked != 0 && marked < members.len()
}
pub fn ui(members: &[Member], squad: u8) -> u8 {
    if members.is_empty() {
        return squad;
    }
    let selected_only = discriminates(members);
    members
        .iter()
        .filter(|m| !selected_only || m.selected)
        .fold(0, |flags, m| {
            flags | if m.pin == 0 { squad } else { m.pin - 1 }
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_subset_controls_the_ui_but_select_all_has_no_preference() {
        let mut members = [
            Member {
                selected: true,
                pin: 1,
            },
            Member {
                selected: false,
                pin: 2,
            },
        ];
        assert_eq!(ui(&members, 1), 0);
        assert!(discriminates(&members));
        members[1].selected = true;
        assert_eq!(ui(&members, 0), 1);
        assert!(!discriminates(&members));
    }
    #[test]
    fn empty_inheritance_and_non_boolean_bytes_match_assembly() {
        assert_eq!(ui(&[], 0xa0), 0xa0);
        let members = [
            Member {
                selected: false,
                pin: 0,
            },
            Member {
                selected: false,
                pin: 4,
            },
        ];
        assert_eq!(ui(&members, 0xa0), 0xa3);
    }
}
