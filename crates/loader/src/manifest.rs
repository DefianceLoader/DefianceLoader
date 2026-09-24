//! Versioned sidecar manifests, `<dll-stem>.plugin.json`.
//!
//! Discovery reads a manifest *instead of* executing the DLL, so a disabled or
//! blocked managed plugin is never loaded just to learn what it wants. The
//! manifest declares the stable plugin ID, the DLL basename, the plugin
//! version, the supported host ABI, the config group, the declared settings and
//! the hard dependencies with supported version ranges.
//!
//! Built-in manifests are generated from `config::builtin`, the one
//! authoritative table, so feature IDs, dependency declarations and the
//! packaged manifests cannot drift apart. A known built-in whose manifest is
//! missing or disagrees with the table is a packaging error, not a reason to
//! fall back to the legacy path.

use super::config::builtin::{self, Builtin};
use super::config::schema::ValueType;
use super::json::{self, Value};
use defiance_api::ABI_VERSION;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The manifest schema this loader understands. Independent of the plugin ABI
/// and the config schema.
pub const SCHEMA: u32 = 1;

/// A parsed, validated manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub dll: String,
    pub version: Version,
    pub abi: u32,
    pub group: String,
    pub settings: Vec<ManifestSetting>,
    pub depends: Vec<Dependency>,
    pub conflicts: Vec<String>,
    /// `multiplayer_safe`: the plugin changes nothing another player's game
    /// would need to match, so it may stay active in multiplayer. Absent means
    /// false; an active plugin without it blocks multiplayer.
    pub multiplayer_safe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSetting {
    pub key: String,
    pub ty: String,
    pub default: String,
    pub description: String,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub id: String,
    pub min: Option<Version>,
    pub max: Option<Version>,
}

/// A dotted numeric version, as the product versions are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn parse(text: &str) -> Result<Version, String> {
        let mut parts = text.split('.');
        let mut numbers = [0u32; 3];
        let mut count = 0;
        for part in &mut parts {
            if count == 3 {
                return Err(format!("`{text}` has too many components"));
            }
            numbers[count] = part
                .parse()
                .map_err(|_| format!("`{text}` is not a dotted numeric version"))?;
            count += 1;
        }
        if count == 0 {
            return Err(format!("`{text}` is not a version"));
        }
        Ok(Version {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
        })
    }
}

impl core::fmt::Display for Version {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Whether `version` is within a dependency's declared range.
pub fn satisfies(version: Version, dependency: &Dependency) -> bool {
    dependency.min.is_none_or(|min| version >= min)
        && dependency.max.is_none_or(|max| version <= max)
}

/// Strip a trailing `suffix`, ignoring ASCII case, without allocating.
fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let start = text.len().checked_sub(suffix.len())?;
    if text.is_char_boundary(start) && text[start..].eq_ignore_ascii_case(suffix) {
        Some(&text[..start])
    } else {
        None
    }
}

/// The stem of a DLL basename: a trailing `.dll` is removed regardless of case.
/// Every place that turns a filename into an identity uses this, so `Example.DLL`
/// and `example.dll` agree.
pub fn dll_stem(dll: &str) -> &str {
    strip_suffix_ignore_case(dll, ".dll").unwrap_or(dll)
}

/// The stem of a sidecar filename, or `None` when it is not `<stem>.plugin.json`
/// regardless of case. A bare `.plugin.json` (empty stem) is not a sidecar.
pub fn sidecar_stem(sidecar: &str) -> Option<&str> {
    strip_suffix_ignore_case(sidecar, ".plugin.json").filter(|stem| !stem.is_empty())
}

/// The sidecar file name for a DLL basename.
pub fn sidecar_name(dll: &str) -> String {
    format!("{}.plugin.json", dll_stem(dll))
}

/// Whether a string is a bare DLL basename: no directory, no traversal.
pub fn safe_dll(dll: &str) -> bool {
    !dll.is_empty()
        && dll.len() <= 128
        && dll.to_ascii_lowercase().ends_with(".dll")
        && !dll.contains(['/', '\\'])
        && !dll.contains("..")
        && !dll.contains(':')
}

fn string_field(map: &[(String, Value)], key: &str) -> Result<String, String> {
    match json_object(map).get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(format!(
            "`{key}` must be a string, not {}",
            json::kind(other)
        )),
        None => Err(format!("missing `{key}`")),
    }
}

/// Build a keyed view of an object, rejecting duplicate keys.
fn json_object(entries: &[(String, Value)]) -> std::collections::BTreeMap<String, Value> {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in entries {
        map.insert(key.clone(), value.clone());
    }
    map
}

pub fn parse(text: &str, expected_dll: &str) -> Result<Manifest, String> {
    let entries = json::parse_object(text)?;
    // Duplicate keys are a hard error: a manifest must not hide one value.
    for key in duplicate_keys(&entries) {
        return Err(format!("duplicate key `{key}`"));
    }
    let map = json_object(&entries);

    let schema = match map.get("schema") {
        Some(Value::Number(number)) if number.fract() == 0.0 => *number as u32,
        Some(_) => return Err("`schema` must be a number".into()),
        None => return Err("missing `schema`".into()),
    };
    if schema > SCHEMA {
        return Err(format!("manifest schema {schema} is newer than {SCHEMA}"));
    }

    let id = string_field(&entries, "id")?;
    if id.is_empty() {
        return Err("`id` must not be empty".into());
    }
    let dll = string_field(&entries, "dll")?;
    if !safe_dll(&dll) {
        return Err(format!("`dll` `{dll}` is not a DLL basename"));
    }
    if !dll.eq_ignore_ascii_case(expected_dll) {
        return Err(format!(
            "manifest names `{dll}` but is the sidecar for `{expected_dll}`"
        ));
    }
    let version = Version::parse(&string_field(&entries, "version")?)?;
    let abi = match map.get("abi") {
        Some(Value::Number(number)) if number.fract() == 0.0 => *number as u32,
        Some(_) => return Err("`abi` must be a number".into()),
        None => return Err("missing `abi`".into()),
    };
    let group = string_field(&entries, "group")?;
    if !builtin::safe_group(&group) {
        return Err(format!(
            "`group` `{group}` is not a safe filename component"
        ));
    }

    let mut settings = Vec::new();
    if let Some(value) = map.get("settings") {
        for item in value.as_array().ok_or("`settings` must be an array")? {
            settings.push(parse_setting(item)?);
        }
    }
    for (index, setting) in settings.iter().enumerate() {
        if settings[..index]
            .iter()
            .any(|other| other.key.eq_ignore_ascii_case(&setting.key))
        {
            return Err(format!("setting `{}` is declared twice", setting.key));
        }
    }

    let mut depends = Vec::new();
    if let Some(value) = map.get("depends") {
        for item in value.as_array().ok_or("`depends` must be an array")? {
            depends.push(parse_dependency(item)?);
        }
    }
    for (index, dependency) in depends.iter().enumerate() {
        if dependency.id.eq_ignore_ascii_case(&id) {
            return Err("a plugin cannot depend on itself".into());
        }
        if depends[..index]
            .iter()
            .any(|other| other.id.eq_ignore_ascii_case(&dependency.id))
        {
            return Err(format!("dependency `{}` is declared twice", dependency.id));
        }
    }

    let mut conflicts = Vec::new();
    if let Some(value) = map.get("conflicts") {
        for item in value.as_array().ok_or("`conflicts` must be an array")? {
            let other = item.as_str().ok_or("`conflicts` entries must be strings")?;
            if other.eq_ignore_ascii_case(&id) {
                return Err("a plugin cannot conflict with itself".into());
            }
            conflicts.push(other.to_string());
        }
    }

    let multiplayer_safe = match map.get("multiplayer_safe") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(other) => {
            return Err(format!(
                "`multiplayer_safe` must be a boolean, not {}",
                json::kind(other)
            ))
        }
    };

    if multiplayer_safe
        && builtin::NOT_MULTIPLAYER_SAFE
            .iter()
            .any(|locked| locked.eq_ignore_ascii_case(&id))
    {
        return Err(format!(
            "`{id}` changes gameplay and cannot be multiplayer_safe"
        ));
    }

    Ok(Manifest {
        schema,
        id,
        dll,
        version,
        abi,
        group,
        settings,
        depends,
        conflicts,
        multiplayer_safe,
    })
}

fn duplicate_keys(entries: &[(String, Value)]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut duplicates = Vec::new();
    for (key, _) in entries {
        if !seen.insert(key.clone()) && !duplicates.contains(key) {
            duplicates.push(key.clone());
        }
    }
    duplicates
}

fn parse_setting(value: &Value) -> Result<ManifestSetting, String> {
    let Value::Object(entries) = value else {
        return Err("a setting must be an object".into());
    };
    let entries: Vec<(String, Value)> = entries.clone();
    let key = string_field(&entries, "key")?;
    if key.is_empty() {
        return Err("a setting `key` must not be empty".into());
    }
    let ty = string_field(&entries, "type")?;
    if !matches!(ty.as_str(), "bool" | "int" | "number" | "choice" | "text") {
        return Err(format!("setting `{key}` has unknown type `{ty}`"));
    }
    let default = string_field(&entries, "default")?;
    let description = string_field(&entries, "description")?;
    let sensitive = match json_object(&entries).get("sensitive") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(other) => {
            return Err(format!(
                "setting `{key}` `sensitive` must be a boolean, not {}",
                json::kind(other)
            ))
        }
    };
    Ok(ManifestSetting {
        key,
        ty,
        default,
        description,
        sensitive,
    })
}

fn parse_dependency(value: &Value) -> Result<Dependency, String> {
    let Value::Object(entries) = value else {
        return Err("a dependency must be an object".into());
    };
    let entries: Vec<(String, Value)> = entries.clone();
    let id = string_field(&entries, "id")?;
    if id.is_empty() {
        return Err("a dependency `id` must not be empty".into());
    }
    let bound = |key: &str| -> Result<Option<Version>, String> {
        match json_object(&entries).get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(Version::parse(text)?)),
            Some(other) => Err(format!(
                "`{key}` must be a version string, not {}",
                json::kind(other)
            )),
        }
    };
    Ok(Dependency {
        id,
        min: bound("min")?,
        max: bound("max")?,
    })
}

/// Render a built-in's manifest from the authoritative table.
pub fn render_builtin(builtin: &Builtin) -> String {
    let settings: Vec<Value> = builtin::settings(builtin.id)
        .iter()
        .map(|decl| {
            Value::Object(vec![
                ("key".into(), Value::String(decl.key.into())),
                ("type".into(), Value::String(type_name(decl.ty).into())),
                ("default".into(), Value::String(decl.default.into())),
                ("description".into(), Value::String(decl.description.into())),
                ("sensitive".into(), Value::Bool(decl.sensitive)),
            ])
        })
        .collect();
    let depends: Vec<Value> = builtin
        .depends
        .iter()
        .map(|id| {
            let min = builtin::find(id).map(|dependency| dependency.version.to_string());
            Value::Object(vec![
                ("id".into(), Value::String((*id).into())),
                ("min".into(), min.map(Value::String).unwrap_or(Value::Null)),
                ("max".into(), Value::Null),
            ])
        })
        .collect();
    let document = Value::Object(vec![
        ("schema".into(), Value::Number(SCHEMA as f64)),
        ("id".into(), Value::String(builtin.id.into())),
        ("dll".into(), Value::String(builtin.dll.into())),
        ("version".into(), Value::String(builtin.version.into())),
        ("abi".into(), Value::Number(ABI_VERSION as f64)),
        ("group".into(), Value::String(builtin.group.into())),
        ("settings".into(), Value::Array(settings)),
        ("depends".into(), Value::Array(depends)),
        ("conflicts".into(), Value::Array(Vec::new())),
        (
            "multiplayer_safe".into(),
            Value::Bool(builtin.multiplayer_safe),
        ),
    ]);
    json::render(&document)
}

fn type_name(ty: ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "bool",
        ValueType::Integer { .. } => "int",
        ValueType::Number { .. } => "number",
        ValueType::Choice(_) => "choice",
        ValueType::Text => "text",
    }
}

/// Every built-in's `(dll basename, manifest JSON)`, for the generator.
pub fn builtin_manifests() -> Vec<(String, String)> {
    builtin::BUILTINS
        .iter()
        .map(|builtin| (builtin.dll.to_string(), render_builtin(builtin)))
        .collect()
}

/// Check a packaged built-in manifest against the authoritative table. Any
/// disagreement is a packaging error rather than a silent downgrade.
pub fn check_builtin(manifest: &Manifest, builtin: &Builtin) -> Result<(), String> {
    if manifest.abi != ABI_VERSION {
        return Err(format!(
            "manifest ABI {} is not {ABI_VERSION}",
            manifest.abi
        ));
    }
    if manifest.id != builtin.id {
        return Err(format!(
            "manifest id `{}` is not built-in `{}`",
            manifest.id, builtin.id
        ));
    }
    if manifest.version.to_string() != builtin.version {
        return Err(format!(
            "manifest version {} is not {}",
            manifest.version, builtin.version
        ));
    }
    if !manifest.dll.eq_ignore_ascii_case(builtin.dll) {
        return Err(format!(
            "manifest dll `{}` is not `{}`",
            manifest.dll, builtin.dll
        ));
    }
    if manifest.group != builtin.group {
        return Err(format!(
            "manifest group `{}` is not `{}`",
            manifest.group, builtin.group
        ));
    }
    if manifest.multiplayer_safe != builtin.multiplayer_safe {
        return Err(format!(
            "manifest multiplayer_safe {} is not {}",
            manifest.multiplayer_safe, builtin.multiplayer_safe
        ));
    }
    let declared: Vec<&str> = manifest.depends.iter().map(|d| d.id.as_str()).collect();
    let expected: Vec<&str> = builtin.depends.iter().copied().collect();
    if declared != expected {
        return Err(format!("manifest depends {declared:?} is not {expected:?}"));
    }
    let declared: Vec<&str> = manifest.settings.iter().map(|s| s.key.as_str()).collect();
    let expected: Vec<&str> = builtin::settings(builtin.id)
        .iter()
        .map(|s| s.key)
        .collect();
    if declared != expected {
        return Err(format!(
            "manifest settings {declared:?} is not {expected:?}"
        ));
    }
    Ok(())
}

/// One plugin DLL found in the discovery directory, with what its manifest (or
/// its absence) says about it. Discovery never loads a DLL.
#[derive(Debug, Clone)]
pub struct Entry {
    pub dll: String,
    pub path: PathBuf,
    pub builtin: Option<&'static Builtin>,
    pub manifest: Option<Manifest>,
    /// Why the node cannot be used, if the manifest is missing or invalid.
    pub error: Option<String>,
    /// A file that is present but deliberately not loaded (obsolete pilot).
    pub ignored: Option<String>,
}

impl Entry {
    /// The stable identity: a manifest's plugin ID, else the DLL stem. The
    /// stem is only a display identity for a legacy plugin.
    pub fn id(&self) -> String {
        match &self.manifest {
            Some(manifest) => manifest.id.clone(),
            None => dll_stem(&self.dll).to_string(),
        }
    }

    pub fn is_legacy(&self) -> bool {
        self.manifest.is_none()
            && self.builtin.is_none()
            && self.error.is_none()
            && self.ignored.is_none()
    }
}

/// The plugin directory as one scan found it: every plugin DLL, paired with
/// its sidecar manifest when one is present, and the discovery warnings
/// (orphan manifests and the obsolete pilot).
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

/// Scan `dir` once. Configuration loading calls this and keeps the result in
/// its snapshot ([`crate::config::Snapshot::catalog`]), which startup planning
/// then uses, so settings and plan come from the same files. Each pair is
/// validated here (built-in packaging, schema, DLL agreement).
pub fn catalog(dir: &Path) -> Catalog {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) => {
            warnings.push(format!("no plugin directory {}: {e}", dir.display()));
            return Catalog { entries, warnings };
        }
    };
    let mut files: Vec<PathBuf> = listing.flatten().map(|entry| entry.path()).collect();
    files.sort();
    let mut seen_manifests: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if sidecar_stem(&name).is_some() {
            seen_manifests.insert(name.to_ascii_lowercase());
        }
    }

    for path in files {
        let dll = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !dll.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        if dll.eq_ignore_ascii_case("defiance_plugin_pickup.dll") {
            warnings.push(
                "obsolete pickup pilot ignored; use defiance_plugin_feature_pickup.dll".into(),
            );
            entries.push(Entry {
                dll,
                path,
                builtin: None,
                manifest: None,
                error: None,
                ignored: Some("obsolete pickup pilot".into()),
            });
            continue;
        }
        let builtin = builtin::by_dll(&dll);
        let sidecar = dir.join(sidecar_name(&dll));
        let mut entry = Entry {
            dll: dll.clone(),
            path,
            builtin,
            manifest: None,
            error: None,
            ignored: None,
        };
        if sidecar.is_file() {
            seen_manifests.remove(&sidecar_name(&dll).to_ascii_lowercase());
            match std::fs::read_to_string(&sidecar) {
                Ok(text) => match parse(&text, &dll) {
                    Ok(parsed) => {
                        if let Some(builtin) = builtin {
                            if let Err(e) = check_builtin(&parsed, builtin) {
                                entry.error = Some(format!("packaging error: {e}"));
                            }
                        }
                        entry.manifest = Some(parsed);
                    }
                    Err(e) => entry.error = Some(format!("{}: {e}", sidecar.display())),
                },
                Err(e) => entry.error = Some(format!("{}: {e}", sidecar.display())),
            }
        } else if builtin.is_some() {
            entry.error = Some(format!(
                "{} is a managed built-in but has no {}",
                dll,
                sidecar_name(&dll)
            ));
        }
        entries.push(entry);
    }

    for orphan in seen_manifests {
        warnings.push(format!("{orphan} has no matching DLL; ignored"));
    }
    Catalog { entries, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::builtin::BUILTINS;

    /// A hand-written manifest with the real ABI, so a version bump cannot make
    /// a rejection test pass for an accidental ABI mismatch.
    fn with_host_abi(json: &str) -> String {
        json.replace("\"abi\":5", &format!("\"abi\":{ABI_VERSION}"))
    }

    #[test]
    fn shipped_gameplay_plugins_cannot_claim_multiplayer_safety() {
        let manifest = |id: &str, safe: &str| {
            with_host_abi(&format!(
                r#"{{"schema":1,"id":"{id}","dll":"x.dll","version":"1.0.0","abi":5,"group":"g","multiplayer_safe":{safe}}}"#
            ))
        };
        let error = parse(&manifest("defiance.regroup", "true"), "x.dll").unwrap_err();
        assert!(error.contains("cannot be multiplayer_safe"), "{error}");
        assert!(
            !parse(&manifest("defiance.regroup", "false"), "x.dll")
                .unwrap()
                .multiplayer_safe
        );
        assert!(
            parse(&manifest("author.display", "true"), "x.dll")
                .unwrap()
                .multiplayer_safe
        );
        assert!(parse(&manifest("author.display", "1"), "x.dll").is_err());
    }

    #[test]
    fn generated_manifests_parse_and_match_the_table() {
        for builtin in BUILTINS {
            let json = render_builtin(builtin);
            let manifest =
                parse(&json, builtin.dll).unwrap_or_else(|e| panic!("{}: {e}\n{json}", builtin.id));
            check_builtin(&manifest, builtin).unwrap_or_else(|e| panic!("{}: {e}", builtin.id));
            assert_eq!(manifest.abi, ABI_VERSION);
        }
    }

    #[test]
    fn a_dll_that_is_not_a_basename_is_refused() {
        let json = render_builtin(&BUILTINS[1]);
        assert!(parse(&json.replace(BUILTINS[1].dll, "../evil.dll"), "../evil.dll").is_err());
    }

    #[test]
    fn sidecar_names_strip_the_dll_suffix() {
        assert_eq!(
            sidecar_name("defiance_plugin_core.dll"),
            "defiance_plugin_core.plugin.json"
        );
    }

    #[test]
    fn mixed_case_extensions_share_one_interpretation() {
        // Discovery accepts `.DLL`; the sidecar for it must be the same file as
        // for the lowercase basename, and the stem must come back unchanged.
        assert_eq!(sidecar_name("Example.DLL"), "Example.plugin.json");
        assert_eq!(sidecar_name("EXAMPLE.Dll"), "EXAMPLE.plugin.json");
        assert_eq!(dll_stem("Example.DLL"), "Example");
        assert_eq!(sidecar_stem("Example.PLUGIN.JSON"), Some("Example"));
        assert_eq!(sidecar_stem("Example.plugin.json"), Some("Example"));
        assert_eq!(sidecar_stem(".plugin.json"), None);
        assert_eq!(sidecar_stem("Example.txt"), None);
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let json = r#"{"schema":1,"schema":2,"id":"x","dll":"x.dll","version":"1.0.0","abi":5,"group":"g"}"#;
        assert!(parse(&with_host_abi(json), "x.dll")
            .unwrap_err()
            .contains("duplicate key"));
    }

    #[test]
    fn a_newer_manifest_schema_is_refused() {
        let json = r#"{"schema":99,"id":"x","dll":"x.dll","version":"1.0.0","abi":5,"group":"g"}"#;
        assert!(parse(&with_host_abi(json), "x.dll")
            .unwrap_err()
            .contains("newer"));
    }

    #[test]
    fn version_ranges_are_checked() {
        let dependency = Dependency {
            id: "a".into(),
            min: Some(Version::parse("0.3.0").unwrap()),
            max: Some(Version::parse("1.0.0").unwrap()),
        };
        assert!(satisfies(Version::parse("0.3.0").unwrap(), &dependency));
        assert!(satisfies(Version::parse("1.0.0").unwrap(), &dependency));
        assert!(!satisfies(Version::parse("0.2.9").unwrap(), &dependency));
        assert!(!satisfies(Version::parse("1.0.1").unwrap(), &dependency));
    }

    #[test]
    fn a_self_dependency_is_refused() {
        let json = r#"{"schema":1,"id":"x","dll":"x.dll","version":"1.0.0","abi":5,"group":"g","depends":[{"id":"x"}]}"#;
        assert!(parse(&with_host_abi(json), "x.dll")
            .unwrap_err()
            .contains("depend on itself"));
    }

    #[test]
    fn a_mismatched_dll_is_refused() {
        let json = render_builtin(&BUILTINS[1]);
        assert!(parse(&json, "other.dll").is_err());
    }
}
