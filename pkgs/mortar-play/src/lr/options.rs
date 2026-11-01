// SPDX-License-Identifier: GPL-2.0-or-later
//! Core options, read-only. The core declares its options (legacy
//! SET_VARIABLES, or v1/v2 definitions); the frontend answers GET_VARIABLE
//! with the system preset from the config when it names a valid value, else
//! with the core's default. There is no options menu by design: presets
//! are curated in modules/mortar-play.nix.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{CStr, CString, c_char};

use super::ffi;

#[derive(Clone, Debug, PartialEq)]
pub struct OptionDef {
    pub key: String,
    pub desc: String,
    pub values: Vec<String>,
    pub default: String,
}

#[derive(Default)]
pub struct Options {
    pub defs: BTreeMap<String, OptionDef>,
    presets: HashMap<String, String>,
    /// Answers handed to the core; the pointers must stay valid while the
    /// core runs, so they live here.
    answers: HashMap<String, CString>,
    /// Set when the answers changed since the core last asked
    /// (GET_VARIABLE_UPDATE).
    pub updated: bool,
    warned: bool,
}

unsafe fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()) }
}

impl Options {
    pub fn new(presets: HashMap<String, String>) -> Options {
        Options { presets, ..Default::default() }
    }

    /// Legacy "Description; value1|value2|..." form (first value is the default).
    pub fn parse_legacy(key: &str, spec: &str) -> OptionDef {
        let (desc, vals) = spec.split_once(';').unwrap_or((spec, ""));
        let values: Vec<String> = vals.trim().split('|').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        OptionDef { key: key.into(), desc: desc.trim().into(), default: values.first().cloned().unwrap_or_default(), values }
    }

    fn insert(&mut self, d: OptionDef) {
        self.answers.remove(&d.key);
        self.defs.insert(d.key.clone(), d);
        self.updated = true;
    }

    /// SET_VARIABLES: array of retro_variable terminated by a NULL key.
    pub unsafe fn set_legacy(&mut self, mut v: *const ffi::retro_variable) {
        unsafe {
            while !v.is_null() && !(*v).key.is_null() {
                if let (Some(k), Some(s)) = (cstr((*v).key), cstr((*v).value)) {
                    self.insert(Options::parse_legacy(&k, &s));
                }
                v = v.add(1);
            }
        }
        self.check_presets();
    }

    unsafe fn values(vals: &[ffi::retro_core_option_value; ffi::NUM_CORE_OPTION_VALUES_MAX]) -> Vec<String> {
        vals.iter().map_while(|v| unsafe { cstr(v.value) }).collect()
    }

    /// SET_CORE_OPTIONS (v1 definitions, NULL-key terminated).
    pub unsafe fn set_v1(&mut self, mut d: *const ffi::retro_core_option_definition) {
        unsafe {
            while !d.is_null() && !(*d).key.is_null() {
                let key = cstr((*d).key).unwrap_or_default();
                let values = Options::values(&(*d).values);
                let default = cstr((*d).default_value).or_else(|| values.first().cloned()).unwrap_or_default();
                self.insert(OptionDef { key, desc: cstr((*d).desc).unwrap_or_default(), values, default });
                d = d.add(1);
            }
        }
        self.check_presets();
    }

    /// SET_CORE_OPTIONS_V2: categories are ignored.
    pub unsafe fn set_v2(&mut self, o: *const ffi::retro_core_options_v2) {
        if o.is_null() {
            return;
        }
        unsafe {
            let mut d = (*o).definitions;
            while !d.is_null() && !(*d).key.is_null() {
                let key = cstr((*d).key).unwrap_or_default();
                let values = Options::values(&(*d).values);
                let default = cstr((*d).default_value).or_else(|| values.first().cloned()).unwrap_or_default();
                self.insert(OptionDef { key, desc: cstr((*d).desc).unwrap_or_default(), values, default });
                d = d.add(1);
            }
        }
        self.check_presets();
    }

    /// Log presets that name unknown options or values (once).
    fn check_presets(&mut self) {
        if self.warned || self.defs.is_empty() {
            return;
        }
        self.warned = true;
        for (k, v) in &self.presets {
            match self.defs.get(k) {
                None => eprintln!("mortar-play: preset {k}={v}: the core has no such option"),
                Some(d) if !d.values.is_empty() && !d.values.contains(v) => {
                    eprintln!("mortar-play: preset {k}={v}: not one of {:?}, using '{}'", d.values, d.default)
                }
                _ => {}
            }
        }
    }

    /// Presets that don't fit what the core declared: (key, value, problem).
    pub fn preset_problems(&self) -> Vec<(String, String, String)> {
        let mut v: Vec<_> = self
            .presets
            .iter()
            .filter_map(|(k, val)| match self.defs.get(k) {
                None => Some((k.clone(), val.clone(), "no such option".to_string())),
                Some(d) if !d.values.is_empty() && !d.values.contains(val) => Some((k.clone(), val.clone(), format!("not one of {:?}", d.values))),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    /// The value the core gets for `key`.
    pub fn value(&self, key: &str) -> Option<String> {
        let def = self.defs.get(key);
        if let Some(p) = self.presets.get(key) {
            match def {
                Some(d) if !d.values.is_empty() && !d.values.contains(p) => {}
                _ => return Some(p.clone()),
            }
        }
        def.map(|d| d.default.clone())
    }

    /// GET_VARIABLE: a pointer valid until the next call for the same key.
    pub fn get_ptr(&mut self, key: &str) -> *const c_char {
        match self.value(key) {
            Some(v) => {
                let c = CString::new(v).unwrap_or_default();
                let e = self.answers.entry(key.to_string()).or_insert(c.clone());
                if *e != c {
                    *e = c;
                }
                e.as_ptr()
            }
            None => std::ptr::null(),
        }
    }

    /// SET_VARIABLE: the core changes one of its own options at runtime.
    pub fn set_value(&mut self, key: &str, value: &str) {
        self.presets.insert(key.into(), value.into());
        self.updated = true;
    }

    pub fn take_updated(&mut self) -> bool {
        std::mem::take(&mut self.updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_parse() {
        let d = Options::parse_legacy("snes9x_region", "Console Region; auto|ntsc|pal");
        assert_eq!(d.desc, "Console Region");
        assert_eq!(d.values, vec!["auto", "ntsc", "pal"]);
        assert_eq!(d.default, "auto");
    }

    #[test]
    fn presets_override_valid_only() {
        let mut o = Options::new(HashMap::from([
            ("a".to_string(), "pal".to_string()),
            ("b".to_string(), "bogus".to_string()),
            ("c".to_string(), "free".to_string()),
        ]));
        o.insert(Options::parse_legacy("a", "A; auto|ntsc|pal"));
        o.insert(Options::parse_legacy("b", "B; on|off"));
        assert_eq!(o.value("a").as_deref(), Some("pal"));
        assert_eq!(o.value("b").as_deref(), Some("on"), "invalid preset falls back to default");
        // unknown to the core (e.g. declared late): the preset is passed through
        assert_eq!(o.value("c").as_deref(), Some("free"));
        assert_eq!(o.value("d"), None);
        let p = o.get_ptr("a");
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "pal");
    }

    #[test]
    fn v1_definitions() {
        let k = CString::new("x_mode").unwrap();
        let d = CString::new("Mode").unwrap();
        let v0 = CString::new("fast").unwrap();
        let v1 = CString::new("slow").unwrap();
        let def = CString::new("slow").unwrap();
        let none = ffi::retro_core_option_value { value: std::ptr::null(), label: std::ptr::null() };
        let mut values: [ffi::retro_core_option_value; ffi::NUM_CORE_OPTION_VALUES_MAX] =
            std::array::from_fn(|_| ffi::retro_core_option_value { value: std::ptr::null(), label: std::ptr::null() });
        values[0].value = v0.as_ptr();
        values[1].value = v1.as_ptr();
        let _ = none;
        let defs = [
            ffi::retro_core_option_definition { key: k.as_ptr(), desc: d.as_ptr(), info: std::ptr::null(), values, default_value: def.as_ptr() },
            ffi::retro_core_option_definition {
                key: std::ptr::null(),
                desc: std::ptr::null(),
                info: std::ptr::null(),
                values: std::array::from_fn(|_| ffi::retro_core_option_value { value: std::ptr::null(), label: std::ptr::null() }),
                default_value: std::ptr::null(),
            },
        ];
        let mut o = Options::new(HashMap::new());
        unsafe { o.set_v1(defs.as_ptr()) };
        assert_eq!(o.defs["x_mode"].values, vec!["fast", "slow"]);
        assert_eq!(o.value("x_mode").as_deref(), Some("slow"));
        assert!(o.take_updated());
        assert!(!o.take_updated());
    }
}
