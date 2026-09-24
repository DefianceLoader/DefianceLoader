//! The patch descriptor: what `tools/payload.py` and `tools/icon.py` write,
//! parsed. Moved out of the injector so the loader's core plugin reads the same
//! format and applies the same bytes.
//!
//! The injector's copy of this parser was bespoke, column by column, to avoid a
//! JSON crate; it is kept that way here, with the field names unchanged. The
//! caller supplies the descriptor text (`include_str!` in the injector and in
//! `plugins/core`), so this crate carries no payload of its own.

use crate::relocate::{parse_edit_fixups, parse_sites, parse_verified};
use crate::scan::Site;

/// One detour: a jmp into the block.
#[derive(Clone)]
pub struct Hook {
    pub feature: u32,
    pub rva: usize,
    pub displaced: Vec<u8>,
    pub entry: usize,
}

/// A slot in the block holding an absolute address into the module. `feature`
/// is the feature that owns the branch or slot; 0 means shared, always needed.
#[derive(Clone)]
pub struct Fixup {
    pub offset: usize,
    pub target_rva: usize,
    pub feature: u32,
}

/// An import the game.dll payload resolves by name.
#[derive(Clone)]
pub struct Export {
    pub offset: usize,
    pub dll: String,
    pub name: String,
}

/// A pose site: the handler's call or an AiUtils thunk's jmp, which becomes a
/// call into the block followed by `tail` (a thunk's ret, over its padding).
/// The movement states' reads and fn_2caeb0's call are retargeted the same way.
#[derive(Clone)]
pub struct PoseCall {
    pub feature: u32,
    pub rva: usize,
    pub before: Vec<u8>,
    pub entry: usize,
    pub tail: Vec<u8>,
    /// the function the site's rel32 reaches, zero where it has none
    pub stock: usize,
}

/// A disp32 in the block that holds the distance from one rva to another: the
/// pose split's, from a caller's return point to the stock function it calls.
#[derive(Clone)]
pub struct DeltaFixup {
    pub offset: usize,
    pub from: usize,
    pub to: usize,
}

/// The logic.dll patch, as built.
#[derive(Clone)]
pub struct Patch {
    pub source_sha256: String,
    pub block_bytes: usize,
    pub cursor_offset: usize,
    pub stock_chooser: usize,
    pub move_offset: usize,
    pub move_call_site: usize,
    pub move_displaced: Vec<u8>,
    pub trace_offset: usize,
    pub ammo_scratch: usize,
    /// The squad preview's cell: Core's dimmed-material callback at +0, which
    /// Core writes with the selection feature (`patch/preview-dim.asm`).
    pub preview_dim_cell: usize,
    pub detours: Vec<Hook>,
    pub setter_offset: usize,
    pub trace_fixups: Vec<Fixup>,
    pub rel_fixups: Vec<Fixup>,
    pub pose_offset: usize,
    pub census_offset: usize,
    pub pose_calls: Vec<PoseCall>,
    pub delta_fixups: Vec<DeltaFixup>,
    pub call_site: usize,
    pub call_before: Vec<u8>,
    pub module_bytes: usize,
    pub image_bytes: usize,
    pub anchor_rva: usize,
    pub anchor: Vec<u8>,
    pub select_is_rva: usize,
    pub select_is_before: Vec<u8>,
    pub select_is_after: Vec<u8>,
    pub select_squad_rva: usize,
    pub select_squad_before: Vec<u8>,
    pub select_squad_after: Vec<u8>,
    pub select_type_rva: usize,
    pub select_type_before: Vec<u8>,
    pub select_type_after: Vec<u8>,
    pub select_toggle_rva: usize,
    pub select_toggle_before: Vec<u8>,
    pub select_toggle_after: Vec<u8>,
    /// where each site is in a build this was not written for (--scan)
    pub sites: Vec<Site>,
    /// other builds checked against the signatures, relocated without --scan
    pub verified: Vec<String>,
    pub edit_fixups: Vec<crate::relocate::EditFixup>,
}

pub fn field(text: &str, name: &str) -> String {
    let key = format!("\"{name}\"");
    let at = text
        .find(&key)
        .unwrap_or_else(|| panic!("no {name} in the descriptor"));
    let rest = &text[at + key.len()..];
    let rest = rest.trim_start().trim_start_matches(':').trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        stripped[..stripped.find('"').expect("unterminated string")].to_string()
    } else {
        rest.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
    }
}

/// Every value for `name`, in the order they appear. The descriptor's arrays
/// are fixed in shape, so reading them column by column is enough and keeps
/// this free of a JSON dependency. Keys are distinct across the two arrays for
/// exactly this reason.
pub fn fields(text: &str, name: &str) -> Vec<String> {
    let key = format!("\"{name}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(&key) {
        rest = &rest[at + key.len()..];
        let value = rest.trim_start().trim_start_matches(':').trim_start();
        out.push(if let Some(stripped) = value.strip_prefix('"') {
            stripped[..stripped.find('"').expect("unterminated string")].to_string()
        } else {
            value.chars().take_while(|c| c.is_ascii_digit()).collect()
        });
    }
    out
}

pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("bad hex"))
        .collect()
}

impl Patch {
    /// The patch a `payload.json` describes.
    pub fn parse(text: &str) -> Self {
        assert_eq!(
            field(text, "feature_schema"),
            "1",
            "regenerate payload descriptors"
        );
        assert_eq!(
            fields(text, "hook_rva").len(),
            fields(text, "hook_feature").len(),
            "every detour needs an owner"
        );
        assert_eq!(
            fields(text, "pose_site").len(),
            fields(text, "pose_feature").len(),
            "every call needs an owner"
        );
        let number = |name: &str| field(text, name).parse::<usize>().expect(name);
        let many = |name: &str| -> Vec<usize> {
            fields(text, name)
                .iter()
                .map(|v| v.parse().expect(name))
                .collect()
        };
        Self {
            source_sha256: field(text, "source_sha256"),
            block_bytes: number("block_bytes"),
            cursor_offset: number("cursor_offset"),
            stock_chooser: number("stock_chooser"),
            move_offset: number("move_offset"),
            move_call_site: number("move_call_site"),
            move_displaced: unhex(&field(text, "move_displaced")),
            trace_offset: number("trace_offset"),
            ammo_scratch: number("ammo_scratch"),
            preview_dim_cell: number("preview_dim_cell"),
            setter_offset: number("setter_offset"),
            detours: many("hook_rva")
                .into_iter()
                .zip(fields(text, "hook_displaced"))
                .zip(many("hook_entry"))
                .zip(many("hook_feature"))
                .map(|(((rva, displaced), entry), feature)| Hook {
                    feature: feature as u32,
                    rva,
                    displaced: unhex(&displaced),
                    entry,
                })
                .collect(),
            trace_fixups: many("trace_fix_offset")
                .into_iter()
                .zip(many("trace_fix_rva"))
                // the diagnostics trace stubs are the only trace owners
                .map(|(offset, target_rva)| Fixup {
                    offset,
                    target_rva,
                    feature: 7,
                })
                .collect(),
            rel_fixups: many("rel_offset")
                .into_iter()
                .zip(many("rel_target"))
                .zip(many("rel_feature"))
                .map(|((offset, target_rva), feature)| Fixup {
                    offset,
                    target_rva,
                    feature: feature as u32,
                })
                .collect(),
            pose_offset: number("pose_offset"),
            census_offset: number("census_offset"),
            pose_calls: many("pose_site")
                .into_iter()
                .zip(fields(text, "pose_before"))
                .zip(many("pose_entry"))
                .zip(fields(text, "pose_tail"))
                .zip(many("pose_stock"))
                .zip(many("pose_feature"))
                .map(
                    |(((((rva, before), entry), tail), stock), feature)| PoseCall {
                        feature: feature as u32,
                        rva,
                        before: unhex(&before),
                        entry,
                        tail: unhex(&tail),
                        stock,
                    },
                )
                .collect(),
            delta_fixups: many("delta_offset")
                .into_iter()
                .zip(many("delta_from"))
                .zip(many("delta_to"))
                .map(|((offset, from), to)| DeltaFixup { offset, from, to })
                .collect(),
            call_site: number("call_site"),
            call_before: unhex(&field(text, "call_before")),
            module_bytes: number("module_bytes"),
            image_bytes: number("image_bytes"),
            anchor_rva: number("anchor_rva"),
            anchor: unhex(&field(text, "anchor")),
            select_is_rva: number("select_is_rva"),
            select_is_before: unhex(&field(text, "select_is_before")),
            select_is_after: unhex(&field(text, "select_is_after")),
            select_squad_rva: number("select_squad_rva"),
            select_squad_before: unhex(&field(text, "select_squad_before")),
            select_squad_after: unhex(&field(text, "select_squad_after")),
            select_type_rva: number("select_type_rva"),
            select_type_before: unhex(&field(text, "select_type_before")),
            select_type_after: unhex(&field(text, "select_type_after")),
            select_toggle_rva: number("select_toggle_rva"),
            select_toggle_before: unhex(&field(text, "select_toggle_before")),
            select_toggle_after: unhex(&field(text, "select_toggle_after")),
            sites: parse_sites(text),
            verified: parse_verified(text),
            edit_fixups: parse_edit_fixups(text),
        }
    }
}

/// The game.dll patch, as built.
#[derive(Clone)]
pub struct GamePatch {
    pub source_sha256: String,
    pub image_bytes: usize,
    pub block_bytes: usize,
    pub anchor_rva: usize,
    pub anchor: Vec<u8>,
    pub trace_offset: usize,
    /// The block cell holding the squad TAB modifier's virtual key (0 off),
    /// which Core writes from the selection feature's setting.
    pub tab_modifier_offset: usize,
    pub hooks: Vec<Hook>,
    pub fixups: Vec<Fixup>,
    pub exports: Vec<Export>,
    pub sites: Vec<Site>,
    /// other builds checked against the signatures, relocated without --scan
    pub verified: Vec<String>,
}

impl GamePatch {
    /// The patch a `payload-game.json` describes.
    pub fn parse(text: &str) -> Self {
        assert_eq!(
            field(text, "feature_schema"),
            "1",
            "regenerate game descriptor"
        );
        assert_eq!(
            fields(text, "rva").len(),
            fields(text, "hook_feature").len(),
            "every game hook needs an owner"
        );
        let number = |name: &str| field(text, name).parse::<usize>().expect(name);
        let many = |name: &str| -> Vec<usize> {
            fields(text, name)
                .iter()
                .map(|v| v.parse::<usize>().expect(name))
                .collect()
        };
        Self {
            source_sha256: field(text, "source_sha256"),
            image_bytes: number("image_bytes"),
            block_bytes: number("block_bytes"),
            anchor_rva: number("anchor_rva"),
            anchor: unhex(&field(text, "anchor")),
            trace_offset: number("trace_offset"),
            tab_modifier_offset: number("tab_modifier_offset"),
            hooks: many("rva")
                .into_iter()
                .zip(fields(text, "displaced"))
                .zip(many("entry"))
                .zip(many("hook_feature"))
                .map(|(((rva, bytes), entry), feature)| Hook {
                    feature: feature as u32,
                    rva,
                    displaced: unhex(&bytes),
                    entry,
                })
                .collect(),
            fixups: many("offset")
                .into_iter()
                .zip(many("target_rva"))
                .zip(many("fixup_feature"))
                .map(|((offset, target_rva), feature)| Fixup {
                    offset,
                    target_rva,
                    feature: feature as u32,
                })
                .collect(),
            exports: many("export_offset")
                .into_iter()
                .zip(fields(text, "dll"))
                .zip(fields(text, "name"))
                .map(|((offset, dll), name)| Export { offset, dll, name })
                .collect(),
            sites: parse_sites(text),
            verified: parse_verified(text),
        }
    }
}
