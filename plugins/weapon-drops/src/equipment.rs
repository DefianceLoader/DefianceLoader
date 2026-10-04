//! Read declared standard weapon slots and member item slot categories.
//!
//! This module has no hooks or global state. Callers provide the squad
//! ScriptInfo pointer and a validated-by-bounds view of member item pointers;
//! returned candidates still require the caller's native gun-resolution and
//! eligibility checks.

const SQUAD_STANDARD_SLOTS: usize = 0x228;
const SLOT_CATEGORY: usize = 0x28;
const SLOT_PROPERTIES: usize = 0x70;
const SLOT_DEFAULT_ITEM: usize = 0x68;
const ITEM_SLOT_TYPES: usize = 0x30;
const HIDDEN_PROPERTY: u32 = 1;
const SLOT_STRIDE: usize = 8;
const SLOT_TYPE_STRIDE: usize = 0x20;
const HOLDER_PAIR_STRIDE: usize = 0x10;
const MAX_STANDARD_SLOTS: usize = 32;
const MAX_MEMBER_ITEMS: usize = 128;
const MAX_SLOT_TYPES_PER_ITEM: usize = 32;
const MAX_CATEGORY_BYTES: usize = 96;

/// A caller-supplied view of a native `std::vector<InventoryItemScriptInfo*>`.
#[derive(Clone, Copy, Debug)]
pub struct ItemBounds {
    pub begin: usize,
    pub end: usize,
    pub capacity: usize,
}

/// An item that declares compatibility with one visible standard slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub slot_index: usize,
    pub slot: usize,
    pub item: usize,
    pub category: String,
}

/// A visible declared standard slot and its current cached/default item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimarySlot {
    pub slot_index: usize,
    pub definition: usize,
    pub category: String,
    pub current_item: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    InvalidSquad,
    InvalidVector,
    TooManyEntries,
    InvalidSlot,
    InvalidItem,
    InvalidString,
}

/// Returns compatible item/slot pairs from the squad's declared standard slots.
///
/// # Safety
/// `squad` must point to a readable native `SquadScriptInfo`. Every pointer in
/// its standard-slot vector and in `items` must point to a readable native
/// object of the corresponding type. Heap-backed MSVC strings and their vector
/// storage must remain readable for the duration of this call. `items` must be
/// a bounds snapshot for the member's inventory-item pointer vector.
pub unsafe fn compatible_primary_items(
    squad: usize,
    items: ItemBounds,
) -> Result<Vec<Candidate>, ReadError> {
    let visible_slots = read_primary_slots(squad, None)?;
    let (item_count, _) = bounds_count(items, SLOT_STRIDE).ok_or(ReadError::InvalidVector)?;
    if item_count > MAX_MEMBER_ITEMS {
        return Err(ReadError::TooManyEntries);
    }

    let mut candidates = Vec::new();
    for item_index in 0..item_count {
        let item = read_word(items.begin + item_index * SLOT_STRIDE);
        if item == 0 || !item.is_multiple_of(8) {
            return Err(ReadError::InvalidItem);
        }
        let (types, type_count) = native_vector(item + ITEM_SLOT_TYPES, SLOT_TYPE_STRIDE)
            .ok_or(ReadError::InvalidVector)?;
        if type_count > MAX_SLOT_TYPES_PER_ITEM {
            return Err(ReadError::TooManyEntries);
        }
        let mut categories = Vec::with_capacity(type_count);
        for type_index in 0..type_count {
            categories.push(read_msvc_string(types + type_index * SLOT_TYPE_STRIDE)?);
        }
        for slot in &visible_slots {
            if categories
                .iter()
                .any(|item_category| item_category == &slot.category)
            {
                candidates.push(Candidate {
                    slot_index: slot.slot_index,
                    slot: slot.definition,
                    item,
                    category: slot.category.clone(),
                });
            }
        }
    }
    Ok(candidates)
}

/// Reads visible primary-category standard slots and their cached/default items.
///
/// Cached rows are the optional `SquadHolderFacet` vector of 16-byte
/// `[slot-definition pointer, InventoryItem pointer]` pairs. A null or absent
/// cached item falls back to the slot definition's `squad_default_item` pointer.
///
/// # Safety
/// `squad` and, when supplied, all storage addressed by `cached_pairs` must be
/// readable native objects for the duration of this call. Every slot definition
/// pointer must reference a readable `WeaponSlotScriptInfo`; MSVC string storage
/// must also remain readable.
pub unsafe fn read_primary_slots(
    squad: usize,
    cached_pairs: Option<ItemBounds>,
) -> Result<Vec<PrimarySlot>, ReadError> {
    if squad == 0 || !squad.is_multiple_of(8) {
        return Err(ReadError::InvalidSquad);
    }
    let (slots, slot_count) =
        native_vector(squad + SQUAD_STANDARD_SLOTS, SLOT_STRIDE).ok_or(ReadError::InvalidVector)?;
    if slot_count > MAX_STANDARD_SLOTS {
        return Err(ReadError::TooManyEntries);
    }
    let cached = if let Some(bounds) = cached_pairs {
        let (count, _) =
            bounds_count(bounds, HOLDER_PAIR_STRIDE).ok_or(ReadError::InvalidVector)?;
        if count > MAX_STANDARD_SLOTS {
            return Err(ReadError::TooManyEntries);
        }
        Some((bounds.begin, count))
    } else {
        None
    };

    let mut primary_slots = Vec::with_capacity(slot_count);
    for index in 0..slot_count {
        let definition = read_word(slots + index * SLOT_STRIDE);
        if definition == 0 || !definition.is_multiple_of(8) {
            return Err(ReadError::InvalidSlot);
        }
        if read_u32(definition + SLOT_PROPERTIES) & HIDDEN_PROPERTY != 0 {
            continue;
        }
        let category = read_msvc_string(definition + SLOT_CATEGORY)?;
        if !primary_category(&category) {
            continue;
        }
        let mut current_item = read_word(definition + SLOT_DEFAULT_ITEM);
        if let Some((pairs, count)) = cached {
            for pair_index in 0..count {
                let pair = pairs + pair_index * HOLDER_PAIR_STRIDE;
                if read_word(pair) == definition {
                    let cached_item = read_word(pair + 8);
                    if cached_item != 0 {
                        current_item = cached_item;
                    }
                    break;
                }
            }
        }
        primary_slots.push(PrimarySlot {
            slot_index: index,
            definition,
            category,
            current_item,
        });
    }
    Ok(primary_slots)
}

fn primary_category(category: &str) -> bool {
    // Qualification categories retain their full names for compatibility;
    // only their primary-weapon classification follows the native category.
    let native = category
        .strip_prefix("gp_energy_basic_")
        .or_else(|| category.strip_prefix("gp_energy_qualified_"))
        .unwrap_or(category);
    matches!(
        native,
        "rifles" | "rifles_m4" | "rifles_ngsw" | "pistols" | "shotguns" | "gp_crew_personal"
    )
}

unsafe fn native_vector(object: usize, stride: usize) -> Option<(usize, usize)> {
    let bounds = ItemBounds {
        begin: read_word(object),
        end: read_word(object + 8),
        capacity: read_word(object + 16),
    };
    let (count, _) = bounds_count(bounds, stride)?;
    Some((bounds.begin, count))
}

fn bounds_count(bounds: ItemBounds, stride: usize) -> Option<(usize, usize)> {
    if bounds.begin == 0
        || !bounds.begin.is_multiple_of(8)
        || bounds.end < bounds.begin
        || bounds.capacity < bounds.end
        || !(bounds.end - bounds.begin).is_multiple_of(stride)
    {
        return None;
    }
    Some(((bounds.end - bounds.begin) / stride, bounds.begin))
}

/// Reads an MSVC x64 `std::string` (16-byte inline buffer, then size/capacity).
unsafe fn read_msvc_string(address: usize) -> Result<String, ReadError> {
    let size = read_word(address + 0x10);
    let capacity = read_word(address + 0x18);
    if size > MAX_CATEGORY_BYTES || capacity < size || capacity > 1024 * 1024 {
        return Err(ReadError::InvalidString);
    }
    let data = if capacity > 15 {
        read_word(address)
    } else {
        address
    };
    if size != 0 && data == 0 {
        return Err(ReadError::InvalidString);
    }
    let bytes = core::slice::from_raw_parts(data as *const u8, size);
    let value = core::str::from_utf8(bytes).map_err(|_| ReadError::InvalidString)?;
    Ok(value.to_owned())
}

unsafe fn read_word(address: usize) -> usize {
    core::ptr::read_unaligned(address as *const usize)
}

unsafe fn read_u32(address: usize) -> u32 {
    core::ptr::read_unaligned(address as *const u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{alloc_zeroed, dealloc, Layout};

    struct Block {
        pointer: usize,
        layout: Layout,
    }

    impl Block {
        fn new(size: usize) -> Self {
            let layout = Layout::from_size_align(size, 16).unwrap();
            let pointer = unsafe { alloc_zeroed(layout) } as usize;
            assert_ne!(pointer, 0);
            Self { pointer, layout }
        }

        unsafe fn word(&self, offset: usize, value: usize) {
            core::ptr::write_unaligned((self.pointer + offset) as *mut usize, value);
        }

        unsafe fn u32(&self, offset: usize, value: u32) {
            core::ptr::write_unaligned((self.pointer + offset) as *mut u32, value);
        }

        unsafe fn string(&self, offset: usize, value: &str, storage: &mut Vec<Block>) {
            let (destination, capacity) = if value.len() <= 15 {
                (self.pointer + offset, 15)
            } else {
                let bytes = Block::new(value.len() + 1);
                let destination = bytes.pointer;
                storage.push(bytes);
                self.word(offset, destination);
                (destination, value.len())
            };
            core::ptr::copy_nonoverlapping(value.as_ptr(), destination as *mut u8, value.len());
            self.word(offset + 0x10, value.len());
            self.word(offset + 0x18, capacity);
        }
    }

    impl Drop for Block {
        fn drop(&mut self) {
            unsafe { dealloc(self.pointer as *mut u8, self.layout) }
        }
    }

    struct Fixture {
        squad: Block,
        _slots: Vec<Block>,
        items: Vec<Block>,
        item_ptrs: Block,
        slot_ptrs: Block,
        _category_storage: Vec<Block>,
    }

    impl Fixture {
        fn new(slot_categories: &[(&str, bool)], item_categories: &[&[&str]]) -> Self {
            let squad = Block::new(0x250);
            let slot_ptrs = Block::new(slot_categories.len().max(1) * 8);
            let slots: Vec<_> = slot_categories.iter().map(|_| Block::new(0x90)).collect();
            let mut category_storage = Vec::new();
            for (index, (category, hidden)) in slot_categories.iter().enumerate() {
                let slot = &slots[index];
                unsafe {
                    slot.string(SLOT_CATEGORY, category, &mut category_storage);
                    slot.u32(SLOT_PROPERTIES, if *hidden { HIDDEN_PROPERTY } else { 0 });
                    slot_ptrs.word(index * 8, slot.pointer);
                }
            }
            unsafe {
                squad.word(SQUAD_STANDARD_SLOTS, slot_ptrs.pointer);
                squad.word(
                    SQUAD_STANDARD_SLOTS + 8,
                    slot_ptrs.pointer + slot_categories.len() * 8,
                );
                squad.word(
                    SQUAD_STANDARD_SLOTS + 16,
                    slot_ptrs.pointer + slot_categories.len() * 8,
                );
            }

            let items: Vec<_> = item_categories.iter().map(|_| Block::new(0x70)).collect();
            let item_ptrs = Block::new(item_categories.len().max(1) * 8);
            for (item_index, types) in item_categories.iter().enumerate() {
                let strings = Block::new(types.len().max(1) * SLOT_TYPE_STRIDE);
                for (type_index, category) in types.iter().enumerate() {
                    let field = Block::new(SLOT_TYPE_STRIDE);
                    unsafe { field.string(0, category, &mut category_storage) }
                    category_storage.push(field);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            category_storage.last().unwrap().pointer as *const u8,
                            (strings.pointer + type_index * SLOT_TYPE_STRIDE) as *mut u8,
                            SLOT_TYPE_STRIDE,
                        );
                    }
                }
                unsafe {
                    items[item_index].word(ITEM_SLOT_TYPES, strings.pointer);
                    items[item_index].word(
                        ITEM_SLOT_TYPES + 8,
                        strings.pointer + types.len() * SLOT_TYPE_STRIDE,
                    );
                    items[item_index].word(
                        ITEM_SLOT_TYPES + 16,
                        strings.pointer + types.len() * SLOT_TYPE_STRIDE,
                    );
                    item_ptrs.word(item_index * 8, items[item_index].pointer);
                }
                category_storage.push(strings);
            }
            Self {
                squad,
                _slots: slots,
                items,
                item_ptrs,
                slot_ptrs,
                _category_storage: category_storage,
            }
        }

        fn read(&self) -> Result<Vec<Candidate>, ReadError> {
            unsafe {
                compatible_primary_items(
                    self.squad.pointer,
                    ItemBounds {
                        begin: self.item_ptrs.pointer,
                        end: self.item_ptrs.pointer + self.items.len() * 8,
                        capacity: self.item_ptrs.pointer + self.items.len() * 8,
                    },
                )
            }
        }
    }

    #[test]
    fn rifle_matches_rifle_and_ignores_grenades_and_auxiliary_pistol() {
        let fixture = Fixture::new(
            &[("rifles", false)],
            &[&["rifles"], &["inf_hg"], &["inf_sg"], &["pistols"]],
        );
        let found = fixture.read().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, "rifles");
        assert_eq!(found[0].item, fixture.items[0].pointer);
    }

    #[test]
    fn declared_pistol_is_a_primary_slot() {
        let fixture = Fixture::new(&[("pistols", false)], &[&["pistols"]]);
        let found = fixture.read().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, "pistols");
    }

    #[test]
    fn crew_personal_slot_matches_approved_items_without_widening_rifle_compatibility() {
        let fixture = Fixture::new(
            &[("gp_crew_personal", false)],
            &[
                &["pistols", "gp_crew_personal"],
                &["pistols", "gp_crew_personal"],
                &["rifles", "gp_crew_personal"],
                &["rifles_m4"],
                &["pistols"],
            ],
        );
        let found = fixture.read().unwrap();
        assert_eq!(found.len(), 3);
        for (candidate, item) in found.iter().zip(&fixture.items) {
            assert_eq!(candidate.category, "gp_crew_personal");
            assert_eq!(candidate.item, item.pointer);
        }
    }

    #[test]
    fn shotgun_matches_its_declared_category() {
        let fixture = Fixture::new(&[("shotguns", false)], &[&["shotguns"]]);
        assert_eq!(fixture.read().unwrap().len(), 1);
    }

    #[test]
    fn qualification_categories_keep_energy_pickups_gated() {
        let conventional = &[
            "rifles",
            "gp_energy_basic_rifles",
            "gp_energy_qualified_rifles",
        ][..];
        let energy = &["rifles", "gp_energy_qualified_rifles"][..];
        let ordinary = &["rifles"][..];
        let basic = Fixture::new(
            &[("gp_energy_basic_rifles", false)],
            &[conventional, energy, ordinary],
        );
        let found = basic.read().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].category, "gp_energy_basic_rifles");
        assert_eq!(found[0].item, basic.items[0].pointer);
        let qualified = Fixture::new(
            &[("gp_energy_qualified_rifles", false)],
            &[conventional, energy, ordinary],
        );
        let found = qualified.read().unwrap();
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|c| c.category == "gp_energy_qualified_rifles"));
    }

    #[test]
    fn qualification_does_not_make_special_or_hidden_slots_primary() {
        let fixture = Fixture::new(
            &[
                ("gp_energy_basic_shotguns", true),
                ("gp_energy_qualified_gp_support_flex", false),
            ],
            &[
                &["gp_energy_basic_shotguns"],
                &["gp_energy_qualified_gp_support_flex"],
            ],
        );
        assert!(fixture.read().unwrap().is_empty());
    }

    #[test]
    fn hidden_or_incompatible_categories_are_not_candidates() {
        let fixture = Fixture::new(
            &[
                ("rifles", true),
                ("gp_crew_personal", true),
                ("gp_support_flex", false),
                ("shotguns", false),
            ],
            &[
                &["rifles"],
                &["gp_crew_personal"],
                &["gp_support_flex"],
                &["rifles_m4"],
            ],
        );
        assert!(fixture.read().unwrap().is_empty());
    }

    #[test]
    fn multiple_declared_slots_return_multiple_compatible_pairs() {
        let fixture = Fixture::new(
            &[("rifles", false), ("shotguns", false)],
            &[&["rifles", "shotguns"]],
        );
        let found = fixture.read().unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].slot_index, 0);
        assert_eq!(found[1].slot_index, 1);
    }

    #[test]
    fn cached_holder_item_overrides_slot_default() {
        let fixture = Fixture::new(&[("rifles", false)], &[]);
        let default_item = Block::new(0x10);
        let current_item = Block::new(0x10);
        let pairs = Block::new(HOLDER_PAIR_STRIDE);
        unsafe {
            fixture._slots[0].word(SLOT_DEFAULT_ITEM, default_item.pointer);
            pairs.word(0, fixture._slots[0].pointer);
            pairs.word(8, current_item.pointer);
        }
        let found = unsafe {
            read_primary_slots(
                fixture.squad.pointer,
                Some(ItemBounds {
                    begin: pairs.pointer,
                    end: pairs.pointer + HOLDER_PAIR_STRIDE,
                    capacity: pairs.pointer + HOLDER_PAIR_STRIDE,
                }),
            )
        }
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].current_item, current_item.pointer);
    }

    #[test]
    fn bad_vector_header_is_rejected() {
        let fixture = Fixture::new(&[("rifles", false)], &[&["rifles"]]);
        unsafe {
            fixture
                .squad
                .word(SQUAD_STANDARD_SLOTS + 8, fixture.slot_ptrs.pointer - 8);
        }
        assert_eq!(fixture.read(), Err(ReadError::InvalidVector));
    }
}
