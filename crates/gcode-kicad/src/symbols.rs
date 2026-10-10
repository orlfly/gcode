//! KiCad symbol library resolution.
//!
//! The schematic writer must embed *real* symbol definitions: ERC depends on
//! pin numbers, pin names and pin electrical types, which the generic stub
//! cannot provide. This module locates a local KiCad symbol library
//! (`*.kicad_sym`), extracts the requested symbol, resolves `extends`
//! inheritance, and rewrites the symbol name to its full `lib_id`.

use crate::sexpr::Sexpr;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Environment override for the symbol directory.
pub const SYMBOL_DIR_ENV: &str = "GCODE_KICAD_SYMBOL_DIR";

/// One pin as declared by a symbol definition.
#[derive(Debug, Clone, PartialEq)]
pub struct PinInfo {
    pub number: String,
    pub name: String,
    /// Electrical type: `passive`, `input`, `output`, `power_in`, ...
    pub etype: String,
    /// Connection point in symbol coordinates (library Y-up convention).
    pub x: f64,
    pub y: f64,
    /// Orientation in degrees (0 = pin extends to the right of its point).
    pub angle: f64,
}

/// Candidate symbol directories, most specific first.
pub fn candidate_symbol_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::var_os(SYMBOL_DIR_ENV) {
        v.push(PathBuf::from(dir));
    }
    v.push(PathBuf::from("/usr/share/kicad/symbols"));
    v.push(PathBuf::from("/usr/local/share/kicad/symbols"));
    v.push(PathBuf::from("/app/extensions/Library/symbols"));
    if let Some(home) = dirs_home() {
        for base in [
            home.join(".local/share/flatpak/app/org.kicad.KiCad/current/active/files/share/kicad/symbols"),
            home.join(".var/app/org.kicad.KiCad/data/kicad/symbols"),
        ] {
            v.push(base);
        }
        if let Ok(entries) = std::fs::read_dir(home.join(".local/share/kicad")) {
            for e in entries.flatten() {
                v.push(e.path().join("symbols"));
            }
        }
    }
    v
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A loaded KiCad symbol library directory with a small parse cache.
pub struct SymbolLibrary {
    dir: Option<PathBuf>,
    /// lib name (file stem) -> parsed top-level symbol nodes
    docs: BTreeMap<String, Vec<Sexpr>>,
    /// `lib_id` -> resolved symbol (already named `lib:Name`)
    cache: BTreeMap<String, Option<Sexpr>>,
    misses: Vec<String>,
}

impl SymbolLibrary {
    /// Locate a symbol library. `None` when no directory is found.
    pub fn locate() -> Self {
        let dir = candidate_symbol_dirs()
            .into_iter()
            .find(|d| d.join("Device.kicad_sym").is_file() || d.is_dir());
        Self {
            dir,
            docs: BTreeMap::new(),
            cache: BTreeMap::new(),
            misses: Vec::new(),
        }
    }

    /// Build from an explicit directory (tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            docs: BTreeMap::new(),
            cache: BTreeMap::new(),
            misses: Vec::new(),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// `lib_id`s that could not be resolved (deduplicated, in first-seen order).
    pub fn misses(&self) -> &[String] {
        &self.misses
    }

    /// Resolve a `lib_id` such as `Device:R` to an embeddable symbol node
    /// whose top-level name is the full `lib_id`.
    pub fn symbol(&mut self, lib_id: &str) -> Option<Sexpr> {
        if let Some(hit) = self.cache.get(lib_id) {
            return hit.clone();
        }
        let resolved = self.resolve(lib_id);
        if resolved.is_none() && !self.misses.iter().any(|m| m == lib_id) {
            self.misses.push(lib_id.to_string());
        }
        self.cache.insert(lib_id.to_string(), resolved.clone());
        resolved
    }

    /// All resolvable `lib_id`s matching a case-insensitive substring,
    /// used for component-library lookup ("which opamp symbols exist?").
    pub fn search(&mut self, needle: &str) -> Vec<String> {
        let needle = needle.to_ascii_lowercase();
        let mut out = Vec::new();
        for lib in self.lib_names() {
            let Some(doc) = self.load_lib(&lib) else {
                continue;
            };
            for node in &doc {
                if node.tag() != Some("symbol") {
                    continue;
                }
                let Some(name) = node.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str)
                else {
                    continue;
                };
                if name.to_ascii_lowercase().contains(&needle) {
                    out.push(format!("{lib}:{name}"));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn lib_names(&self) -> Vec<String> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("kicad_sym") {
                    p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        names
    }

    fn load_lib(&mut self, lib: &str) -> Option<Vec<Sexpr>> {
        if let Some(doc) = self.docs.get(lib) {
            return Some(doc.clone());
        }
        let dir = self.dir.clone()?;
        let path = dir.join(format!("{lib}.kicad_sym"));
        let text = std::fs::read_to_string(&path).ok()?;
        // Files are large; keep only the top-level symbol nodes.
        let parsed = Sexpr::parse(&text).ok()?;
        let root = parsed.into_iter().next()?;
        let items: Vec<Sexpr> = root
            .find_all("symbol")
            .into_iter()
            .cloned()
            .collect();
        self.docs.insert(lib.to_string(), items.clone());
        Some(items)
    }

    fn find_raw(&mut self, lib: &str, name: &str) -> Option<Sexpr> {
        let doc = self.load_lib(lib)?;
        doc.iter().find_map(|node| {
            let n = node.as_list()?.get(1)?.as_str()?;
            (n == name).then(|| node.clone())
        })
    }

    /// Resolve a symbol, following `(extends "Parent")` chains.
    fn resolve(&mut self, lib_id: &str) -> Option<Sexpr> {
        let (lib, name) = lib_id.split_once(':')?;
        let raw = self.find_raw(lib, name)?;
        let parent = raw
            .find("extends")
            .and_then(|e| e.as_list())
            .and_then(|l| l.get(1))
            .and_then(Sexpr::as_str)
            .map(str::to_string);

        let mut body = match parent {
            Some(parent_name) => {
                let base = self.resolve_in(lib, &parent_name)?;
                let inherited = inherit(&base, &parent_name, name);
                // The child's own properties/sub-units override the inherited ones.
                overlay(&raw, &inherited)
            }
            None => raw.clone(),
        };

        // Rename the top-level symbol to the full lib_id.
        if let Sexpr::List(items) = &mut body {
            if let Some(Sexpr::Str(s)) = items.get_mut(1) {
                *s = lib_id.to_string();
            }
            // `extends` must not survive into an embedded definition.
            items.retain(|i| i.tag() != Some("extends"));
        }
        Some(body)
    }

    fn resolve_in(&mut self, lib: &str, name: &str) -> Option<Sexpr> {
        self.find_raw(lib, name)
    }
}

/// Flatten KiCad symbol inheritance: take the parent's body, rename its
/// `Parent_i_u` sub-units to `Child_i_u`, then overlay the child's own items
/// (same-named properties and sub-units replace the inherited ones).
fn inherit(parent: &Sexpr, parent_name: &str, child_name: &str) -> Sexpr {
    let mut items: Vec<Sexpr> = parent
        .as_list()
        .map(|l| l[1..].to_vec())
        .unwrap_or_default();

    for item in &mut items {
        if item.tag() == Some("symbol") {
            if let Sexpr::List(sub) = item {
                if let Some(Sexpr::Str(s)) = sub.get_mut(1) {
                    if let Some(rest) = s.strip_prefix(parent_name) {
                        *s = format!("{child_name}{rest}");
                    }
                }
            }
        }
    }
    let mut out = vec![Sexpr::Atom("symbol".into()), Sexpr::Str(child_name.to_string())];
    out.extend(items);
    Sexpr::List(out)
}

/// Overlay the child's own items onto an inherited body.
pub fn overlay(child: &Sexpr, base: &Sexpr) -> Sexpr {
    let Some(child_items) = child.as_list() else {
        return base.clone();
    };
    let Some(base_items) = base.as_list() else {
        return child.clone();
    };
    let key = |i: &Sexpr| -> Option<String> {
        match i.tag() {
            Some("property") => {
                let n = i.as_list()?.get(1)?.as_str()?.to_string();
                Some(format!("property:{n}"))
            }
            Some("symbol") => {
                let n = i.as_list()?.get(1)?.as_str()?.to_string();
                Some(format!("symbol:{n}"))
            }
            _ => None,
        }
    };
    let mut out: Vec<Sexpr> = base_items[1..].to_vec();
    for item in &child_items[1..] {
        match key(item) {
            Some(k) => match out.iter().position(|b| key(b).as_deref() == Some(k.as_str())) {
                Some(idx) => out[idx] = item.clone(),
                None => out.push(item.clone()),
            },
            None => out.push(item.clone()),
        }
    }
    let mut head = vec![
        Sexpr::Atom("symbol".into()),
        child_items
            .get(1)
            .cloned()
            .unwrap_or(Sexpr::Str(String::new())),
    ];
    head.extend(out);
    Sexpr::List(head)
}

/// Pins declared anywhere in a symbol (sub-units included).
pub fn pins_of(symbol: &Sexpr) -> Vec<PinInfo> {
    let mut out = Vec::new();
    collect_pins(symbol, &mut out);
    out
}

fn collect_pins(node: &Sexpr, out: &mut Vec<PinInfo>) {
    if node.tag() == Some("pin") {
        if let Some(items) = node.as_list() {
            let number = items
                .iter()
                .find(|c| c.tag() == Some("number"))
                .and_then(|c| c.as_list()?.get(1))
                .and_then(Sexpr::as_str)
                .unwrap_or("")
                .to_string();
            let name = items
                .iter()
                .find(|c| c.tag() == Some("name"))
                .and_then(|c| c.as_list()?.get(1))
                .and_then(Sexpr::as_str)
                .unwrap_or("")
                .to_string();
            let etype = items
                .iter()
                .find_map(|c| c.as_atom().map(str::to_string))
                .unwrap_or_default();
            let etype = items
                .iter()
                .filter_map(Sexpr::as_atom)
                .find(|a| {
                    matches!(
                        *a,
                        "input"
                            | "output"
                            | "bidirectional"
                            | "tri_state"
                            | "passive"
                            | "free"
                            | "unspecified"
                            | "power_in"
                            | "power_out"
                            | "open_collector"
                            | "open_emitter"
                            | "no_connect"
                    )
                })
                .unwrap_or(etype.as_str())
                .to_string();
            let at = items.iter().find(|c| c.tag() == Some("at"));
            let num = |i: usize| -> f64 {
                at.and_then(|a| a.as_list()?.get(i))
                    .and_then(Sexpr::as_str)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0)
            };
            out.push(PinInfo {
                number,
                name,
                etype,
                x: num(1),
                y: num(2),
                angle: num(3),
            });
        }
    }
    if let Some(items) = node.as_list() {
        for c in items {
            collect_pins(c, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_library() -> bool {
        SymbolLibrary::locate().dir().is_some()
    }

    #[test]
    fn resolves_simple_symbol() {
        if !has_library() {
            eprintln!("skip: no KiCad symbol library");
            return;
        }
        let mut lib = SymbolLibrary::locate();
        let sym = lib.symbol("Device:R").expect("Device:R resolvable");
        assert_eq!(
            sym.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str),
            Some("Device:R")
        );
        let pins = pins_of(&sym);
        assert_eq!(pins.len(), 2, "R has two pins");
        assert!(pins.iter().all(|p| p.etype == "passive"));
    }

    #[test]
    fn resolves_extends_inheritance() {
        if !has_library() {
            eprintln!("skip: no KiCad symbol library");
            return;
        }
        let mut lib = SymbolLibrary::locate();
        // LM358 is defined as (extends "LM2904") — pins must be inherited.
        let sym = lib.symbol("Amplifier_Operational:LM358").expect("LM358");
        let pins = pins_of(&sym);
        let mut nums: Vec<&str> = pins.iter().map(|p| p.number.as_str()).collect();
        nums.sort();
        assert_eq!(nums, vec!["1", "2", "3", "4", "5", "6", "7", "8"]);
        // value overridden by the child, not the parent
        let value = sym
            .find_all("property")
            .into_iter()
            .find(|p| p.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str) == Some("Value"))
            .and_then(|p| p.as_list()?.get(2))
            .and_then(Sexpr::as_str)
            .map(str::to_string);
        assert_eq!(value.as_deref(), Some("LM358"));
        assert!(sym.find("extends").is_none(), "extends must be flattened");
    }

    #[test]
    fn missing_symbol_is_tracked() {
        let mut lib = SymbolLibrary::locate();
        assert!(lib.symbol("NoSuchLib:Nope").is_none());
        assert_eq!(lib.misses(), vec!["NoSuchLib:Nope"]);
    }

    #[test]
    fn search_finds_devices() {
        if !has_library() {
            eprintln!("skip: no KiCad symbol library");
            return;
        }
        let mut lib = SymbolLibrary::locate();
        let hits = lib.search("lm358");
        assert!(hits.contains(&"Amplifier_Operational:LM358".to_string()));
    }
}
