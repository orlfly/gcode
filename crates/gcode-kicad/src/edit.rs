//! IR-level diff for conversational circuit modification.
//!
//! All edits are expressed as a structured `IrDiff` between the old and new
//! netlist. The diff is shown to the user, and the file is only written after
//! explicit confirmation. Because schematic UUIDs are content-derived
//! (see [`crate::schematic`]), elements that did not change keep byte-identical
//! identities across a rewrite, so unchanged parts of the file are stable.

use crate::netlist::{Component, IrError, Net, NetlistIr};
use crate::schematic::{generate_with_report, Layout};
use crate::sexpr::Sexpr;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A single changed field of a component.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    pub reference: String,
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

/// Structural difference between two netlists.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IrDiff {
    pub components_added: Vec<Component>,
    pub components_removed: Vec<Component>,
    pub fields_changed: Vec<FieldChange>,
    pub nets_added: Vec<Net>,
    pub nets_removed: Vec<Net>,
    /// (net, reference, pin)
    pub connections_added: Vec<(String, String, String)>,
    pub connections_removed: Vec<(String, String, String)>,
}

impl IrDiff {
    pub fn is_empty(&self) -> bool {
        self.components_added.is_empty()
            && self.components_removed.is_empty()
            && self.fields_changed.is_empty()
            && self.nets_added.is_empty()
            && self.nets_removed.is_empty()
            && self.connections_added.is_empty()
            && self.connections_removed.is_empty()
    }

    /// Every component reference touched by this diff.
    pub fn touched_references(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for c in self.components_added.iter().chain(&self.components_removed) {
            out.insert(c.reference.clone());
        }
        for f in &self.fields_changed {
            out.insert(f.reference.clone());
        }
        for (_, r, _) in self.connections_added.iter().chain(&self.connections_removed) {
            out.insert(r.clone());
        }
        out
    }

    /// Human-readable rendering, suitable for showing before a write.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return "无改动".to_string();
        }
        let mut out = String::from("电路改动（确认后才会写入文件）：\n");
        for c in &self.components_added {
            out.push_str(&format!(
                "+ 新增元件 {} ({}, 值 {})\n",
                c.reference, c.lib_id, c.value
            ));
        }
        for c in &self.components_removed {
            out.push_str(&format!("- 删除元件 {} ({})\n", c.reference, c.lib_id));
        }
        for f in &self.fields_changed {
            out.push_str(&format!(
                "~ 修改 {} 的 {}：{} -> {}\n",
                f.reference, f.field, f.from, f.to
            ));
        }
        for n in &self.nets_added {
            out.push_str(&format!("+ 新增网络 {}\n", n.name));
        }
        for n in &self.nets_removed {
            out.push_str(&format!("- 删除网络 {}\n", n.name));
        }
        for (net, r, p) in &self.connections_added {
            out.push_str(&format!("+ 连接 {}.{} -> {}\n", r, p, net));
        }
        for (net, r, p) in &self.connections_removed {
            out.push_str(&format!("- 断开 {}.{} -/-> {}\n", r, p, net));
        }
        out
    }

    /// Compact one-line summary, e.g. "+1 -0 ~1 nets+0/-0 conn+1/-0".
    pub fn summary(&self) -> String {
        format!(
            "元件 +{} -{} 改{}；网络 +{} -{}；连接 +{} -{}",
            self.components_added.len(),
            self.components_removed.len(),
            self.fields_changed.len(),
            self.nets_added.len(),
            self.nets_removed.len(),
            self.connections_added.len(),
            self.connections_removed.len(),
        )
    }
}

/// Compute the diff from `old` to `new`.
pub fn diff(old: &NetlistIr, new: &NetlistIr) -> IrDiff {
    let old_by_ref: BTreeMap<&str, &Component> = old
        .components
        .iter()
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let new_by_ref: BTreeMap<&str, &Component> = new
        .components
        .iter()
        .map(|c| (c.reference.as_str(), c))
        .collect();

    let mut d = IrDiff::default();

    for (r, c) in &new_by_ref {
        match old_by_ref.get(r) {
            None => d.components_added.push((*c).clone()),
            Some(o) => {
                if o.lib_id != c.lib_id {
                    d.fields_changed.push(FieldChange {
                        reference: (*r).to_string(),
                        field: "symbol",
                        from: o.lib_id.clone(),
                        to: c.lib_id.clone(),
                    });
                }
                if o.value != c.value {
                    d.fields_changed.push(FieldChange {
                        reference: (*r).to_string(),
                        field: "value",
                        from: o.value.clone(),
                        to: c.value.clone(),
                    });
                }
                if o.footprint != c.footprint {
                    d.fields_changed.push(FieldChange {
                        reference: (*r).to_string(),
                        field: "footprint",
                        from: o.footprint.clone().unwrap_or_default(),
                        to: c.footprint.clone().unwrap_or_default(),
                    });
                }
            }
        }
    }
    for (r, c) in &old_by_ref {
        if !new_by_ref.contains_key(r) {
            d.components_removed.push((*c).clone());
        }
    }

    let net_map = |ir: &NetlistIr| -> BTreeMap<String, BTreeSet<(String, String)>> {
        ir.nets
            .iter()
            .map(|n| {
                (
                    n.name.clone(),
                    n.connections
                        .iter()
                        .map(|(r, p)| (r.clone(), p.clone()))
                        .collect(),
                )
            })
            .collect()
    };
    let old_nets = net_map(old);
    let new_nets = net_map(new);

    for (name, conns) in &new_nets {
        match old_nets.get(name) {
            None => {
                if let Some(n) = new.nets.iter().find(|n| &n.name == name) {
                    d.nets_added.push(n.clone());
                }
                for (r, p) in conns {
                    d.connections_added.push((name.clone(), r.clone(), p.clone()));
                }
            }
            Some(before) => {
                for (r, p) in conns.difference(before) {
                    d.connections_added.push((name.clone(), r.clone(), p.clone()));
                }
                for (r, p) in before.difference(conns) {
                    d.connections_removed.push((name.clone(), r.clone(), p.clone()));
                }
            }
        }
    }
    for (name, conns) in &old_nets {
        if !new_nets.contains_key(name) {
            if let Some(n) = old.nets.iter().find(|n| n.name == *name) {
                d.nets_removed.push(n.clone());
            }
            for (r, p) in conns {
                d.connections_removed.push((name.clone(), r.clone(), p.clone()));
            }
        }
    }

    d.components_added.sort_by(|a, b| a.reference.cmp(&b.reference));
    d.components_removed.sort_by(|a, b| a.reference.cmp(&b.reference));
    d.connections_added.sort();
    d.connections_removed.sort();
    d
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("netlist invalid: {0}")]
    InvalidIr(#[from] IrError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file {0} does not exist; nothing to edit")]
    Missing(PathBuf),
}

/// A staged (not yet written) modification of a schematic file.
///
/// `stage` never touches the file system. `confirm` writes the new content;
/// `revert` restores the content captured at stage time.
pub struct FileEdit {
    path: PathBuf,
    before: String,
    after: String,
    diff: IrDiff,
    warnings: Vec<String>,
    written: bool,
}

impl FileEdit {
    /// Compute the edit but do not write anything.
    pub fn stage(
        path: impl Into<PathBuf>,
        old_ir: &NetlistIr,
        new_ir: &NetlistIr,
        layout: Layout,
    ) -> Result<Self, EditError> {
        let path = path.into();
        let before = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => EditError::Missing(path.clone()),
            _ => EditError::Io(e),
        })?;
        let report = generate_with_report(new_ir, layout)?;
        let d = diff(old_ir, new_ir);
        Ok(Self {
            path,
            before,
            after: report.content,
            diff: d,
            warnings: report.warnings,
            written: false,
        })
    }

    pub fn diff(&self) -> &IrDiff {
        &self.diff
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_empty(&self) -> bool {
        self.diff.is_empty()
    }

    /// Human-readable diff to show the user before confirming.
    pub fn render(&self) -> String {
        self.diff.render()
    }

    /// Apply the staged change. Only this method writes.
    pub fn confirm(&mut self) -> Result<(), EditError> {
        if self.diff.is_empty() {
            return Ok(());
        }
        std::fs::write(&self.path, &self.after)?;
        self.written = true;
        Ok(())
    }

    /// Drop the staged change (nothing was written, so this is a no-op that
    /// documents intent).
    pub fn discard(&mut self) {
        self.written = false;
    }

    /// Restore the content captured when the edit was staged.
    pub fn revert(&mut self) -> Result<(), EditError> {
        std::fs::write(&self.path, &self.before)?;
        self.written = false;
        Ok(())
    }

    pub fn has_written(&self) -> bool {
        self.written
    }
}

/// Extract the instance UUID generated for a component reference.
///
/// Used to prove that untouched elements keep their identity across a rewrite.
pub fn instance_uuid(schematic: &str, reference: &str) -> Option<String> {
    let docs = Sexpr::parse(schematic).ok()?;
    let root = docs.first()?;
    for sym in root.find_all("symbol") {
        if sym.find("lib_id").is_none() {
            continue;
        }
        let is_target = sym
            .find_all("property")
            .into_iter()
            .any(|p| {
                p.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str) == Some("Reference")
                    && p.as_list().and_then(|l| l.get(2)).and_then(Sexpr::as_str) == Some(reference)
            });
        if is_target {
            return sym
                .get("uuid")
                .and_then(Sexpr::as_str)
                .map(str::to_string);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn divider() -> NetlistIr {
        catalog::by_id("voltage-divider").unwrap().ir
    }

    fn with_value(ir: &NetlistIr, reference: &str, value: &str) -> NetlistIr {
        let mut n = ir.clone();
        for c in &mut n.components {
            if c.reference == reference {
                c.value = value.into();
            }
        }
        n
    }

    #[test]
    fn identical_ir_yields_empty_diff() {
        let d = diff(&divider(), &divider());
        assert!(d.is_empty());
        assert_eq!(d.render(), "无改动");
    }

    #[test]
    fn detects_value_change() {
        let d = diff(&divider(), &with_value(&divider(), "R2", "4.7k"));
        assert_eq!(d.fields_changed.len(), 1);
        let f = &d.fields_changed[0];
        assert_eq!(f.reference, "R2");
        assert_eq!(f.field, "value");
        assert_eq!(f.from, "10k");
        assert_eq!(f.to, "4.7k");
        assert!(d.render().contains("R2"));
        assert!(d.render().contains("4.7k"));
        assert_eq!(d.touched_references().len(), 1);
    }

    #[test]
    fn detects_component_added_and_removed() {
        let mut added = divider();
        added
            .components
            .push(Component::new("C1", "Device:C", "100nF"));
        added.nets.push(Net::new("VOUT", &[("C1", "1")]));
        added.nets.push(Net::new("GND", &[("C1", "2")]));
        let d = diff(&divider(), &added);
        assert_eq!(d.components_added.len(), 1);
        assert_eq!(d.components_added[0].reference, "C1");
        assert!(d.connections_added.iter().any(|(n, r, p)| n == "VOUT" && r == "C1" && p == "1"));

        let d2 = diff(&added, &divider());
        assert_eq!(d2.components_removed.len(), 1);
        assert!(d2.render().contains("删除元件 C1"));
    }

    #[test]
    fn detects_rewiring() {
        let mut rewired = divider();
        // move R2's bottom pin from GND to VOUT
        for n in &mut rewired.nets {
            n.connections.retain(|(r, p)| !(r == "R2" && p == "2"));
        }
        for n in &mut rewired.nets {
            if n.name == "VOUT" {
                n.connections.push(("R2".into(), "2".into()));
            }
        }
        let d = diff(&divider(), &rewired);
        assert!(d
            .connections_removed
            .iter()
            .any(|(n, r, p)| n == "GND" && r == "R2" && p == "2"));
        assert!(d
            .connections_added
            .iter()
            .any(|(n, r, p)| n == "VOUT" && r == "R2" && p == "2"));
        let text = d.render();
        assert!(text.contains("断开 R2.2"));
        assert!(text.contains("连接 R2.2"));
    }

    #[test]
    fn detected_net_added_and_removed() {
        let mut ir = divider();
        ir.nets.retain(|n| n.name != "VOUT");
        ir.nets.push(Net::new("MID", &[("R1", "2"), ("R2", "1")]));
        let d = diff(&divider(), &ir);
        assert!(d.nets_added.iter().any(|n| n.name == "MID"));
        assert!(d.nets_removed.iter().any(|n| n.name == "VOUT"));
    }

    #[test]
    fn stage_does_not_touch_the_file() {
        let dir = std::env::temp_dir().join(format!("gcode-edit-stage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.kicad_sch");
        let old = divider();
        std::fs::write(&path, generate_with_report(&old, Layout::default()).unwrap().content)
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let mut edit =
            FileEdit::stage(&path, &old, &with_value(&old, "R2", "4.7k"), Layout::default())
                .unwrap();
        assert!(!edit.is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "staging must not write"
        );
        assert!(!edit.has_written());

        edit.confirm().unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_ne!(after, before);
        assert!(after.contains("4.7k"));
        assert!(edit.has_written());

        edit.revert().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!edit.has_written());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discard_leaves_file_untouched() {
        let dir = std::env::temp_dir().join(format!("gcode-edit-discard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.kicad_sch");
        let old = divider();
        std::fs::write(&path, generate_with_report(&old, Layout::default()).unwrap().content)
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let mut edit =
            FileEdit::stage(&path, &old, &with_value(&old, "R2", "4.7k"), Layout::default())
                .unwrap();
        edit.discard();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_elements_keep_their_identity() {
        // Identity is derived from the element (lib_id + reference), not from
        // position or value, so a rewrite changes content without churning the
        // identities of elements that still exist.
        let old = divider();
        let new = with_value(&old, "R2", "4.7k");
        let a = generate_with_report(&old, Layout::default()).unwrap().content;
        let b = generate_with_report(&new, Layout::default()).unwrap().content;
        assert_ne!(a, b, "content must change when a value changes");
        for reference in ["R1", "R2", "FLG1", "FLG2"] {
            assert_eq!(
                instance_uuid(&a, reference),
                instance_uuid(&b, reference),
                "{reference} identity must be stable across a rewrite"
            );
        }
        // Adding a component must not disturb the identities of the others.
        let mut grown = old.clone();
        grown
            .components
            .push(Component::new("C1", "Device:C", "100nF"));
        for n in &mut grown.nets {
            match n.name.as_str() {
                "VOUT" => n.connections.push(("C1".into(), "1".into())),
                "GND" => n.connections.push(("C1".into(), "2".into())),
                _ => {}
            }
        }
        let c = generate_with_report(&grown, Layout::default()).unwrap().content;
        for reference in ["R1", "R2", "FLG1", "FLG2"] {
            assert_eq!(
                instance_uuid(&a, reference),
                instance_uuid(&c, reference),
                "{reference} identity must survive an insertion"
            );
        }
    }

    #[test]
    fn staging_a_missing_file_errors() {
        let old = divider();
        let res = FileEdit::stage(
            "/nonexistent/gcode/nope.kicad_sch",
            &old,
            &old,
            Layout::default(),
        );
        assert!(matches!(res, Err(EditError::Missing(_))));
    }
}
