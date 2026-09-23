/// What the decision needs to know about one member.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    /// the slot type he is carrying, or 0 for nothing
    pub held: i32,
    /// whether the player marked him; only meaningful for a candidate
    pub marked: bool,
}

/// The member to hand the weapon to, and the cursor to leave behind (only the
/// swap pass moves it). Mirrors `patch/pickup.asm`: a marked candidate wins
/// while an unmarked one exists, then a free slot goes to the first empty
/// member, then the swap pass rotates.
pub fn choose(
    members: &[Member],
    want: i32,
    allow_swap: bool,
    free_slot: bool,
    cursor: u32,
) -> Option<(usize, Option<u32>)> {
    let candidate =
        |m: &Member| (m.held == 0 && free_slot) || (m.held != 0 && m.held == want && allow_swap);

    // pass 0: the first marked candidate, if not every candidate is marked
    let mut first_marked = None;
    let mut any_unmarked = false;
    for (i, member) in members.iter().enumerate() {
        if !candidate(member) {
            continue;
        }
        if member.marked {
            if first_marked.is_none() {
                first_marked = Some(i);
            }
        } else {
            any_unmarked = true;
        }
    }
    if let Some(i) = first_marked {
        if any_unmarked {
            return Some((i, None));
        }
    }

    // pass 1: a free slot goes to the first member carrying nothing
    if free_slot {
        if let Some(i) = members.iter().position(|member| member.held == 0) {
            return Some((i, None));
        }
    }

    // pass 2: swap, starting at the cursor
    if !allow_swap || members.is_empty() {
        return None;
    }
    let count = members.len();
    let start = if (cursor as usize) < count {
        cursor as usize
    } else {
        0
    };
    for step in 0..count {
        let index = (start + step) % count;
        if members[index].held == want {
            return Some((index, Some((index + 1) as u32)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(held: &[i32], marked: &[usize]) -> Vec<Member> {
        held.iter()
            .enumerate()
            .map(|(i, &held)| Member {
                held,
                marked: marked.contains(&i),
            })
            .collect()
    }

    #[test]
    fn a_marked_candidate_wins_while_an_unmarked_one_exists() {
        // two members can take it, the second is marked
        let squad = members(&[0, 0], &[1]);
        assert_eq!(choose(&squad, 1, true, true, 0), Some((1, None)));
    }

    #[test]
    fn every_candidate_marked_is_no_preference() {
        // selecting the whole squad marks everyone; the free-slot pass decides
        let squad = members(&[0, 0], &[0, 1]);
        assert_eq!(choose(&squad, 1, true, true, 0), Some((0, None)));
    }

    #[test]
    fn a_free_slot_goes_to_the_first_empty_member() {
        let squad = members(&[0, 0], &[]);
        assert_eq!(choose(&squad, 1, true, true, 0), Some((0, None)));
    }

    #[test]
    fn the_swap_pass_rotates_and_leaves_the_cursor_past_the_choice() {
        // all carrying the wanted weapon; start at 0, choose 0, cursor becomes 1
        let squad = members(&[1, 1, 1], &[]);
        assert_eq!(choose(&squad, 1, true, false, 0), Some((0, Some(1))));
        // from cursor 1, choose 1
        assert_eq!(choose(&squad, 1, true, false, 1), Some((1, Some(2))));
        // from the last, wrap to the first
        assert_eq!(choose(&squad, 1, true, false, 2), Some((2, Some(3))));
        // a cursor at count is normalised to 0
        assert_eq!(choose(&squad, 1, true, false, 3), Some((0, Some(1))));
    }

    #[test]
    fn a_swap_needs_a_wanted_weapon_and_permission() {
        let squad = members(&[1, 2], &[]);
        assert_eq!(choose(&squad, 1, false, false, 0), None);
        assert_eq!(choose(&squad, 7, true, false, 0), None);
    }

    #[test]
    fn empty_type_with_full_slots_skips_mark_preference() {
        let squad = members(&[0, 0], &[1]);
        assert_eq!(choose(&squad, 0, true, false, 0), Some((0, Some(1))));
    }

    #[test]
    fn no_pickup_when_the_wanted_slot_is_full_and_nothing_matches() {
        // empty members but no free slot, and no one carries the type
        let squad = members(&[0, 0], &[]);
        assert_eq!(choose(&squad, 1, true, false, 0), None);
    }
}
