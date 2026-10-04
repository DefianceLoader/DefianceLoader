use std::collections::HashSet;

use defiance_api::{
    TraceFieldV1, TraceRequestV1, TRACE_F32, TRACE_F64, TRACE_FIELDS, TRACE_NO_FILTER, TRACE_U16,
    TRACE_U32, TRACE_U64, TRACE_U8,
};
use defiance_core::json::Value;

use crate::presets;

const MAX_PROBES: usize = 4;
const MAX_FIELDS: usize = TRACE_FIELDS;
const MAX_STACK_FRAMES: usize = 16;
const MAX_JSON_NESTING: usize = 16;
const MAX_SAFE_JSON_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Events,
    Census,
}

#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub name: String,
    pub spec: TraceFieldV1,
}

#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub name: String,
    pub module: String,
    pub rva: Option<usize>,
    /// A unique byte signature; the parser cannot decode instructions, so a
    /// custom signature author must place `site_offset` on an instruction
    /// boundary suitable for a hardware breakpoint.
    pub signature: Option<String>,
    pub site_offset: usize,
    pub module_sha256: Option<String>,
    pub mode: Mode,
    pub fields: Vec<Field>,
    pub request: TraceRequestV1,
}

fn object<'a>(value: &'a Value, at: &str) -> Result<&'a [(String, Value)], String> {
    match value {
        Value::Object(entries) => {
            let mut seen = HashSet::new();
            for (key, _) in entries {
                if !seen.insert(key.as_str()) {
                    return Err(format!("{at}: duplicate key `{key}`"));
                }
            }
            Ok(entries)
        }
        _ => Err(format!("{at}: expected object")),
    }
}

fn keys(value: &Value, allowed: &[&str], at: &str) -> Result<(), String> {
    for (key, _) in object(value, at)? {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{at}: unknown key `{key}`"));
        }
    }
    Ok(())
}

fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn required<'a>(value: &'a Value, key: &str, at: &str) -> Result<&'a Value, String> {
    get(value, key).ok_or_else(|| format!("{at}: missing `{key}`"))
}

fn text(value: &Value, at: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{at}: expected string"))
}

fn name(value: &Value, at: &str) -> Result<String, String> {
    let name = text(value, at)?;
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(format!("{at}: invalid name `{name}`"));
    }
    Ok(name)
}

fn bool_value(value: &Value, at: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{at}: expected boolean"))
}

fn json_integer(value: &Value, at: &str) -> Result<i128, String> {
    match value {
        Value::Number(n)
            if n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_SAFE_JSON_INTEGER =>
        {
            Ok(*n as i128)
        }
        Value::Number(_) => Err(format!(
            "{at}: expected an exact JSON integer (at most 2^53-1)"
        )),
        _ => Err(format!("{at}: expected integer")),
    }
}

fn unsigned(value: &Value, at: &str, max: u64) -> Result<u64, String> {
    let parsed = match value {
        Value::String(s) if s.starts_with("0x") || s.starts_with("0X") => {
            u64::from_str_radix(&s[2..], 16)
                .map_err(|_| format!("{at}: invalid hexadecimal integer `{s}`"))?
                as i128
        }
        Value::String(s) => {
            return Err(format!(
                "{at}: expected integer or hexadecimal string, got `{s}`"
            ))
        }
        _ => json_integer(value, at)?,
    };
    if parsed < 0 || parsed > max as i128 {
        return Err(format!("{at}: integer is outside 0..={max}"));
    }
    Ok(parsed as u64)
}

fn signed_offset(value: &Value, at: &str) -> Result<i32, String> {
    let parsed = match value {
        Value::String(s) => {
            let (negative, digits) =
                if let Some(rest) = s.strip_prefix("-0x").or_else(|| s.strip_prefix("-0X")) {
                    (true, rest)
                } else if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    (false, rest)
                } else {
                    return Err(format!(
                        "{at}: expected signed integer or `0x...` offset string"
                    ));
                };
            let magnitude = i64::from_str_radix(digits, 16)
                .map_err(|_| format!("{at}: invalid hexadecimal offset `{s}`"))?;
            if negative {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => i64::try_from(json_integer(value, at)?)
            .map_err(|_| format!("{at}: offset is outside the signed 64-bit range"))?,
    };
    i32::try_from(parsed).map_err(|_| format!("{at}: offset is outside the signed 32-bit range"))
}

fn register(value: &Value, at: &str) -> Result<u32, String> {
    let index = match value.as_str() {
        Some("rax") => 0,
        Some("rcx") => 1,
        Some("rdx") => 2,
        Some("rbx") => 3,
        Some("rsp") => 4,
        Some("rbp") => 5,
        Some("rsi") => 6,
        Some("rdi") => 7,
        Some("r8") => 8,
        Some("r9") => 9,
        Some("r10") => 10,
        Some("r11") => 11,
        Some("r12") => 12,
        Some("r13") => 13,
        Some("r14") => 14,
        Some("r15") => 15,
        Some("rip") => 16,
        _ => return Err(format!("{at}: unsupported register; use rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15, or rip")),
    };
    Ok(index)
}

fn kind(value: &Value, at: &str) -> Result<u32, String> {
    match value.as_str() {
        Some("u8") => Ok(TRACE_U8),
        Some("u16") => Ok(TRACE_U16),
        Some("u32") => Ok(TRACE_U32),
        Some("u64") => Ok(TRACE_U64),
        Some("f32") => Ok(TRACE_F32),
        Some("f64") => Ok(TRACE_F64),
        _ => Err(format!("{at}: expected one of u8, u16, u32, u64, f32, f64")),
    }
}

fn custom_fields(value: Option<&Value>, at: &str) -> Result<Vec<Field>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("{at}: expected array"))?;
    if values.len() > MAX_FIELDS {
        return Err(format!("{at}: at most {MAX_FIELDS} fields are allowed"));
    }
    let mut out = Vec::with_capacity(values.len());
    let mut names = HashSet::new();
    for (index, value) in values.iter().enumerate() {
        let path = format!("{at}[{index}]");
        keys(value, &["name", "register", "type", "path"], &path)?;
        let field_name = name(required(value, "name", &path)?, &format!("{path}.name"))?;
        if !names.insert(field_name.clone()) {
            return Err(format!("{path}: duplicate field name `{field_name}`"));
        }
        let register = register(
            required(value, "register", &path)?,
            &format!("{path}.register"),
        )?;
        let kind = kind(required(value, "type", &path)?, &format!("{path}.type"))?;
        let offsets = match get(value, "path") {
            None => Vec::new(),
            Some(path_value) => {
                let path_values = path_value
                    .as_array()
                    .ok_or_else(|| format!("{path}.path: expected array"))?;
                if path_values.len() > 4 {
                    return Err(format!("{path}.path: at most four offsets are allowed"));
                }
                path_values
                    .iter()
                    .enumerate()
                    .map(|(i, item)| signed_offset(item, &format!("{path}.path[{i}]")))
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        let mut spec = TraceFieldV1 {
            register,
            kind,
            depth: offsets.len() as u32,
            ..TraceFieldV1::default()
        };
        spec.offsets[..offsets.len()].copy_from_slice(&offsets);
        out.push(Field {
            name: field_name,
            spec,
        });
    }
    Ok(out)
}

fn mode(value: Option<&Value>, at: &str) -> Result<Mode, String> {
    match value {
        None => Ok(Mode::Events),
        Some(Value::String(text)) if text == "events" => Ok(Mode::Events),
        Some(Value::String(text)) if text == "census" => Ok(Mode::Census),
        _ => Err(format!("{at}: expected `events` or `census`")),
    }
}

fn module(value: Option<&Value>, at: &str) -> Result<String, String> {
    let module = value
        .map(|v| text(v, at))
        .transpose()?
        .unwrap_or_else(|| "logic.dll".into());
    if module != "logic.dll" && module != "game.dll" {
        return Err(format!("{at}: module must be `logic.dll` or `game.dll`"));
    }
    Ok(module)
}

fn sha256(value: &Value, at: &str) -> Result<String, String> {
    let digest = text(value, at)?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{at}: expected a 64-digit SHA-256 hex digest"));
    }
    Ok(digest.to_ascii_lowercase())
}

fn string_signature(value: &Value, at: &str) -> Result<String, String> {
    let signature = text(value, at)?;
    let pattern = defiance_core::pattern::parse(&signature).map_err(|e| format!("{at}: {e}"))?;
    if pattern.len() > 128 {
        return Err(format!("{at}: signature may contain at most 128 bytes"));
    }
    if !pattern.iter().any(|byte| byte.is_some()) {
        return Err(format!(
            "{at}: signature must include at least one fixed byte"
        ));
    }
    Ok(signature)
}

fn request(
    fields: &[Field],
    hits: u64,
    every: u64,
    interval_ms: u64,
    stack_frames: u64,
) -> TraceRequestV1 {
    let mut request = TraceRequestV1 {
        size: std::mem::size_of::<TraceRequestV1>() as u32,
        hits: hits as u32,
        every: every as u32,
        min_interval_ms: interval_ms as u32,
        field_count: fields.len() as u32,
        stack_frames: stack_frames as u32,
        filter_field: TRACE_NO_FILTER,
        filter_mask: u64::MAX,
        ..TraceRequestV1::default()
    };
    for (destination, field) in request.fields.iter_mut().zip(fields) {
        *destination = field.spec;
    }
    request
}

fn filter(
    value: Option<&Value>,
    fields: &[Field],
    request: &mut TraceRequestV1,
    at: &str,
) -> Result<(), String> {
    let Some(value) = value else { return Ok(()) };
    keys(value, &["field", "value", "mask"], at)?;
    let field_name = text(required(value, "field", at)?, &format!("{at}.field"))?;
    let index = fields
        .iter()
        .position(|field| field.name == field_name)
        .ok_or_else(|| format!("{at}: field `{field_name}` does not exist"))?;
    request.filter_field = index as u32;
    request.filter_value = unsigned(
        required(value, "value", at)?,
        &format!("{at}.value"),
        u64::MAX,
    )?;
    request.filter_mask = get(value, "mask")
        .map(|v| unsigned(v, &format!("{at}.mask"), u64::MAX))
        .transpose()?
        .unwrap_or(u64::MAX);
    Ok(())
}

fn parse_probe(value: &Value, index: usize) -> Result<Option<Probe>, String> {
    let at = format!("probes[{index}]");
    keys(
        value,
        &[
            "name",
            "preset",
            "enabled",
            "module",
            "rva",
            "signature",
            "site_offset",
            "module_sha256",
            "mode",
            "hits",
            "every",
            "interval_ms",
            "stack_frames",
            "fields",
            "filter",
        ],
        &at,
    )?;
    let probe_name = name(required(value, "name", &at)?, &format!("{at}.name"))?;
    let enabled = get(value, "enabled")
        .map(|v| bool_value(v, &format!("{at}.enabled")))
        .transpose()?
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }

    let is_preset = get(value, "preset").is_some();
    let mut probe = if let Some(preset_value) = get(value, "preset") {
        let preset_name = text(preset_value, &format!("{at}.preset"))?;
        let mut probe = presets::preset(&preset_name)
            .ok_or_else(|| format!("{at}: unknown preset `{preset_name}`"))?;
        probe.name = probe_name;
        probe
    } else {
        Probe {
            name: probe_name,
            module: module(get(value, "module"), &format!("{at}.module"))?,
            rva: get(value, "rva")
                .map(|v| unsigned(v, &format!("{at}.rva"), usize::MAX as u64).map(|n| n as usize))
                .transpose()?,
            signature: get(value, "signature")
                .map(|v| string_signature(v, &format!("{at}.signature")))
                .transpose()?,
            site_offset: get(value, "site_offset")
                .map(|v| {
                    unsigned(v, &format!("{at}.site_offset"), usize::MAX as u64).map(|n| n as usize)
                })
                .transpose()?
                .unwrap_or(0),
            module_sha256: get(value, "module_sha256")
                .map(|v| sha256(v, &format!("{at}.module_sha256")))
                .transpose()?,
            mode: mode(get(value, "mode"), &format!("{at}.mode"))?,
            fields: custom_fields(get(value, "fields"), &format!("{at}.fields"))?,
            request: TraceRequestV1::default(),
        }
    };
    if is_preset
        && [
            "module",
            "rva",
            "signature",
            "site_offset",
            "module_sha256",
            "mode",
            "fields",
        ]
        .iter()
        .any(|key| get(value, key).is_some())
    {
        return Err(format!(
            "{at}: a preset cannot override its module, site, mode, or fields"
        ));
    }
    if !is_preset
        && probe.signature.is_none()
        && (probe.rva.is_none() || probe.module_sha256.is_none())
    {
        return Err(format!(
            "{at}: custom probes require a signature, or both `rva` and `module_sha256`"
        ));
    }
    if !is_preset && probe.signature.is_some() && probe.module_sha256.is_some() {
        return Err(format!(
            "{at}: choose a signature or a build-specific SHA-256/RVA site"
        ));
    }
    if probe.signature.is_none() && probe.site_offset != 0 {
        return Err(format!("{at}: `site_offset` requires a signature"));
    }
    if let Some(signature) = &probe.signature {
        let length = defiance_core::pattern::parse(signature)
            .map_err(|e| format!("{at}.signature: {e}"))?
            .len();
        // find_pattern_at returns the match start plus this offset. The offset
        // must name an in-pattern byte so it cannot advance past the match.
        if probe.site_offset >= length {
            return Err(format!(
                "{at}.site_offset: must be inside the signature (0..{length})"
            ));
        }
    }

    let hits = get(value, "hits")
        .map(|v| unsigned(v, &format!("{at}.hits"), 1_000_000))
        .transpose()?
        .unwrap_or(1000);
    if hits == 0 {
        return Err(format!("{at}.hits: must be at least 1"));
    }
    let every = get(value, "every")
        .map(|v| unsigned(v, &format!("{at}.every"), 1_000_000))
        .transpose()?
        .unwrap_or(1);
    if every == 0 {
        return Err(format!("{at}.every: must be at least 1"));
    }
    let interval = get(value, "interval_ms")
        .map(|v| unsigned(v, &format!("{at}.interval_ms"), u32::MAX as u64))
        .transpose()?
        .unwrap_or(if is_preset {
            probe.request.min_interval_ms as u64
        } else if probe.mode == Mode::Census {
            0
        } else {
            100
        });
    let stack_frames = get(value, "stack_frames")
        .map(|v| unsigned(v, &format!("{at}.stack_frames"), MAX_STACK_FRAMES as u64))
        .transpose()?
        .unwrap_or(if probe.mode == Mode::Census { 0 } else { 8 });
    if is_preset {
        probe.request.hits = hits as u32;
        probe.request.every = every as u32;
        probe.request.min_interval_ms = interval as u32;
        probe.request.stack_frames = stack_frames as u32;
    } else {
        probe.request = request(&probe.fields, hits, every, interval, stack_frames);
    }
    if probe.mode == Mode::Census {
        let caller = probe.fields.iter().find(|field| field.name == "caller");
        if caller.map_or(true, |field| field.spec.kind != TRACE_U64) {
            return Err(format!(
                "{at}: census mode requires a readable u64 field named `caller`"
            ));
        }
    }
    filter(
        get(value, "filter"),
        &probe.fields,
        &mut probe.request,
        &format!("{at}.filter"),
    )?;
    Ok(Some(probe))
}

fn check_nesting(text: &str) -> Result<(), String> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_JSON_NESTING {
                    return Err(format!(
                        "config: JSON nesting exceeds {MAX_JSON_NESTING} levels"
                    ));
                }
            }
            // Structural mismatches remain the JSON parser's responsibility.
            // Saturating here avoids hiding a later excessive opening depth.
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn parse(text: &str) -> Result<Vec<Probe>, String> {
    check_nesting(text)?;
    let root = defiance_core::json::parse(text)?;
    keys(&root, &["version", "probes"], "config")?;
    let version = unsigned(
        required(&root, "version", "config")?,
        "config.version",
        u64::MAX,
    )?;
    if version != 1 {
        return Err(format!("config: unsupported version {version}"));
    }
    let values = required(&root, "probes", "config")?
        .as_array()
        .ok_or_else(|| "config.probes: expected array".to_string())?;
    let mut names = HashSet::new();
    let mut probes = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let at = format!("probes[{index}]");
        let probe_name = name(required(value, "name", &at)?, &format!("{at}.name"))?;
        if !names.insert(probe_name.clone()) {
            return Err(format!("{at}: duplicate probe name `{probe_name}`"));
        }
        if let Some(probe) = parse_probe(value, index)? {
            probes.push(probe);
            if probes.len() > MAX_PROBES {
                return Err(format!(
                    "config: at most {MAX_PROBES} enabled probes are allowed"
                ));
            }
        }
    }
    Ok(probes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(probe: &str) -> Result<Vec<Probe>, String> {
        parse(&format!(r#"{{"version":1,"probes":[{probe}]}}"#))
    }

    #[test]
    fn disabled_probe_defaults_to_off_and_skips_site_semantics() {
        assert!(parse_one(r#"{"name":"later","module":"bad.dll","rva":-1}"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_custom_fields_and_signed_chained_offsets() {
        let parsed = parse_one(r#"{"name":"custom","enabled":true,"signature":"48 8B ??","fields":[{"name":"x","register":"rcx","type":"u32","path":["-0x10",16]}]}"#).unwrap();
        assert_eq!(parsed[0].fields[0].spec.register, 1);
        assert_eq!(parsed[0].fields[0].spec.offsets[..2], [-16, 16]);
        assert_eq!(parsed[0].fields[0].spec.offsets[2..], [0, 0]);
        assert_eq!(
            parsed[0].request.size as usize,
            std::mem::size_of::<TraceRequestV1>()
        );
    }

    #[test]
    fn validates_required_build_identity_and_module() {
        assert!(parse_one(r#"{"name":"custom","enabled":true,"rva":16}"#).is_err());
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"module":"x.dll","signature":"90"}"#)
                .is_err()
        );
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"rva":16,"module_sha256":"bad"}"#)
                .is_err()
        );
    }

    #[test]
    fn rejects_duplicate_names_unknown_keys_and_bad_filters() {
        assert!(parse(r#"{"version":1,"probes":[{"name":"same"},{"name":"same"}]}"#).is_err());
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"signature":"90","typo":1}"#).is_err()
        );
        assert!(parse_one(r#"{"name":"custom","enabled":true,"signature":"90","fields":[{"name":"x","register":"rax","type":"u64"}],"filter":{"field":"missing","value":0}}"#).is_err());
    }

    #[test]
    fn rejects_unknown_preset_overrides_and_malformed_signatures() {
        assert!(parse_one(
            r#"{"name":"p","preset":"selection-select","enabled":true,"fields":[]}"#
        )
        .is_err());
        assert!(parse_one(r#"{"name":"p","enabled":true,"signature":""}"#).is_err());
        assert!(parse_one(r#"{"name":"p","enabled":true,"signature":"?? ??"}"#).is_err());
        assert!(
            parse_one(r#"{"name":"p","enabled":true,"signature":"90","site_offset":1}"#).is_err()
        );
        assert!(
            parse_one(r#"{"name":"p","enabled":true,"signature":"90","site_offset":2}"#).is_err()
        );
    }

    #[test]
    fn census_requires_a_named_caller_and_depth_zero_values_preserve_raw_bits() {
        assert!(
            parse_one(r#"{"name":"c","enabled":true,"mode":"census","signature":"90"}"#).is_err()
        );
        assert!(parse_one(r#"{"name":"c","enabled":true,"mode":"census","signature":"90","fields":[{"name":"caller","register":"rsp","type":"u32"}]}"#).is_err());
        let parsed = parse_one(r#"{"name":"c","enabled":true,"mode":"census","signature":"90","fields":[{"name":"caller","register":"rsp","type":"u64"},{"name":"small","register":"rax","type":"u8"},{"name":"float_bits","register":"rdx","type":"f32"}]}"#).unwrap();
        assert_eq!(parsed[0].fields[0].spec.depth, 0);
        assert_eq!(parsed[0].fields[1].spec.kind, defiance_api::TRACE_U8);
        assert_eq!(parsed[0].fields[2].spec.kind, TRACE_F32);
        assert_eq!(parsed[0].fields[2].spec.depth, 0);
    }

    #[test]
    fn rejects_bounds_fractional_and_unsafe_json_integer_values() {
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"signature":"90","mode":42}"#).is_err()
        );
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"signature":"90","hits":1.5}"#).is_err()
        );
        assert!(
            parse_one(r#"{"name":"custom","enabled":true,"signature":"90","every":0}"#).is_err()
        );
        assert!(parse_one(
            r#"{"name":"custom","enabled":true,"signature":"90","stack_frames":17}"#
        )
        .is_err());
        assert!(parse_one(r#"{"name":"custom","enabled":true,"signature":"90","fields":[{"name":"x","register":"rax","type":"u64","path":[2147483648]}]}"#).is_err());
        assert!(parse_one(r#"{"name":"custom","enabled":true,"signature":"90","filter":{"field":"x","value":9007199254740992}}"#).is_err());
    }

    #[test]
    fn presets_expand_to_signature_sites_and_obey_the_four_site_limit() {
        let config = r#"{"version":1,"probes":[{"name":"a","preset":"selection-select","enabled":true},{"name":"b","preset":"selection-toggle","enabled":true},{"name":"c","preset":"selection-deselect","enabled":true},{"name":"d","preset":"selection-clear","enabled":true}]}"#;
        let probes = parse(config).unwrap();
        assert_eq!(probes.len(), 4);
        assert!(probes
            .iter()
            .all(|probe| probe.request.min_interval_ms == 0));
        let too_many = config
            .replace("selection-clear", "behaviour-census")
            .replace(
                "]}",
                ", {\"name\":\"e\",\"preset\":\"selection-select\",\"enabled\":true}]}",
            );
        assert!(parse(&too_many).is_err());
    }

    #[test]
    fn request_filter_accepts_full_u64_hex() {
        let parsed = parse_one(r#"{"name":"custom","enabled":true,"signature":"90","fields":[{"name":"x","register":"rax","type":"u64"}],"filter":{"field":"x","value":"0xffffffffffffffff"}}"#).unwrap();
        assert_eq!(parsed[0].request.filter_value, u64::MAX);
        assert_eq!(parsed[0].request.filter_mask, u64::MAX);
    }

    #[test]
    fn rejects_excessive_nesting_before_recursive_json_parse() {
        let nested = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_NESTING + 1),
            "]".repeat(MAX_JSON_NESTING + 1)
        );
        assert!(nested.len() < 64 * 1024);
        let error = parse(&nested).unwrap_err();
        assert!(error.contains("nesting exceeds 16 levels"));
    }

    #[test]
    fn nesting_preflight_ignores_brackets_and_escaped_quotes_in_strings() {
        let text = r#"{"version":1,"probes":[],"extra":"braces { [ and escaped quote \" } ]"}"#;
        assert!(check_nesting(text).is_ok());
        let error = parse(text).unwrap_err();
        assert!(error.contains("unknown key `extra`"));
    }
}
