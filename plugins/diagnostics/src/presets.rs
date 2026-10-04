use defiance_api::{TraceFieldV1, TraceRequestV1, TRACE_NO_FILTER, TRACE_U32, TRACE_U64};

use crate::config::{Field, Mode, Probe};

const SELECT: &str = "40 53 48 81 EC C0 00 00 00 48 8B DA 48 85 D2 0F 84 ?? ?? ?? ?? 48 8B 02";
const TOGGLE: &str = "48 89 5C 24 08 48 89 54 24 10 57 48 83 EC 40 48 8B DA 48 8B F9 48 8B 02";
const DESELECT: &str = "48 83 EC 28 48 8B 02 48 8B CA FF 90 B0 00 00 00 48 8B 48 50 48 85 C9 74 0D";
const CLEAR: &str = "48 89 5C 24 08 57 48 83 EC 20 48 8B 79 30 48 8B 59 28 48 3B DF 74 2F 66 0F 1F 84 00 00 00 00 00";
const CENSUS: &str = "8B 81 00 02 00 00 C3 CC CC CC CC CC CC CC CC CC 8B 81 04 02 00 00 C3 CC";

fn field(name: &str, register: u32, kind: u32, offsets: &[i32]) -> Field {
    let mut spec = TraceFieldV1 {
        register,
        kind,
        depth: offsets.len() as u32,
        ..TraceFieldV1::default()
    };
    spec.offsets[..offsets.len()].copy_from_slice(offsets);
    Field {
        name: name.into(),
        spec,
    }
}

fn base(name: &str, signature: &str, fields: Vec<Field>, mode: Mode) -> Probe {
    let mut request = TraceRequestV1 {
        size: std::mem::size_of::<TraceRequestV1>() as u32,
        hits: 1000,
        every: 1,
        // Selection operations arrive in short bursts; report every hit in
        // that timeline. Custom probes retain the slower 100 ms default.
        min_interval_ms: 0,
        stack_frames: if mode == Mode::Census { 0 } else { 8 },
        filter_field: TRACE_NO_FILTER,
        filter_mask: u64::MAX,
        ..TraceRequestV1::default()
    };
    request.field_count = fields.len() as u32;
    for (dst, field) in request.fields.iter_mut().zip(&fields) {
        *dst = field.spec;
    }
    Probe {
        name: name.into(),
        module: "logic.dll".into(),
        rva: None,
        signature: Some(signature.into()),
        site_offset: 0,
        module_sha256: None,
        mode,
        fields,
        request,
    }
}

pub(crate) fn preset(name: &str) -> Option<Probe> {
    let (site, signature, fields, mode) = match name {
        "selection-select" => (
            "selection-select",
            SELECT,
            vec![
                field("caller", 4, TRACE_U64, &[0]),
                field("entity", 2, TRACE_U64, &[]),
            ],
            Mode::Events,
        ),
        "selection-toggle" => (
            "selection-toggle",
            TOGGLE,
            vec![
                field("caller", 4, TRACE_U64, &[0]),
                field("entity", 2, TRACE_U64, &[]),
            ],
            Mode::Events,
        ),
        "selection-deselect" => (
            "selection-deselect",
            DESELECT,
            vec![
                field("caller", 4, TRACE_U64, &[0]),
                field("entity", 2, TRACE_U64, &[]),
            ],
            Mode::Events,
        ),
        "selection-clear" => (
            "selection-clear",
            CLEAR,
            vec![
                field("caller", 4, TRACE_U64, &[0]),
                field("manager", 1, TRACE_U64, &[]),
            ],
            Mode::Events,
        ),
        "behaviour-census" => (
            "behaviour-census",
            CENSUS,
            vec![
                field("caller", 4, TRACE_U64, &[0]),
                field("behaviour", 1, TRACE_U32, &[0x200]),
            ],
            Mode::Census,
        ),
        _ => return None,
    };
    Some(base(site, signature, fields, mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preset_has_a_unique_signature_and_expected_capture() {
        for name in [
            "selection-select",
            "selection-toggle",
            "selection-deselect",
            "selection-clear",
            "behaviour-census",
        ] {
            let probe = preset(name).unwrap();
            assert_eq!(probe.module, "logic.dll");
            assert!(probe.signature.is_some());
            assert!(probe.rva.is_none());
            assert_eq!(probe.request.field_count as usize, probe.fields.len());
        }
        let census = preset("behaviour-census").unwrap();
        assert_eq!(census.mode, Mode::Census);
        assert_eq!(census.request.stack_frames, 0);
        assert_eq!(census.request.min_interval_ms, 0);
        assert_eq!(census.fields[1].spec.offsets[0], 0x200);
        assert_eq!(census.fields[1].spec.kind, TRACE_U32);
        for name in [
            "selection-select",
            "selection-toggle",
            "selection-deselect",
            "selection-clear",
        ] {
            let selection = preset(name).unwrap();
            assert_eq!(selection.request.every, 1);
            assert_eq!(selection.request.min_interval_ms, 0);
        }
    }

    #[test]
    fn unknown_preset_is_not_resolved() {
        assert!(preset("passenger-shot-distance").is_none());
    }
}
