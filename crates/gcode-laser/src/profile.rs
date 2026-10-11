//! Laser parameter guidance: a small, sourced knowledge base.
//!
//! Every entry carries its applicability and where it came from. Outputs always
//! carry the disclaimer: these are starting points that must be calibrated by a
//! test cut on the actual machine.
//!
//! There is no laser here. Nothing in this module claims a cut has been made.

use serde::{Deserialize, Serialize};

pub const DISCLAIMER: &str = "这些参数只是起点，不是结论：必须先在同一台机器上用同一批板材试刻校准；\
换机器、换板材批次、换镜片都必须重新校准。本工具未在真实激光机上验证任何参数。";

/// One recipe: material + thickness -> laser settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaserProfile {
    pub id: String,
    pub material: String,
    /// Thickness range this entry is meant for, in mm.
    pub thickness_mm: (f64, f64),
    /// Laser power as a GRBL `S` value (0..=max_s).
    pub power_s: u32,
    /// Maximum `S` the entry assumes (GRBL `$30`).
    pub max_s: u32,
    pub speed_mm_min: u32,
    pub passes: u32,
    /// How many times the isolation path is repeated with a small offset in the
    /// job (`passes` above is for a single groove's depth).
    pub applicability: String,
    pub source: String,
}

impl LaserProfile {
    /// Short line for G-code comments.
    pub fn comment_line(&self) -> String {
        format!(
            "{} | {} | S{}/{} | F{} mm/min | {} 遍 | 来源: {}",
            self.id, self.material, self.power_s, self.max_s, self.speed_mm_min, self.passes,
            self.source
        )
    }
}

/// A user-supplied override; anything set here wins over the knowledge base.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Eq)]
pub struct ProfileOverride {
    pub power_s: Option<u32>,
    pub speed_mm_min: Option<u32>,
    pub passes: Option<u32>,
    pub max_s: Option<u32>,
    pub material: Option<String>,
}

/// Where the parameters in a job came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProfileSource {
    /// Taken verbatim from the knowledge base.
    KnowledgeBase { id: String, source: String },
    /// Knowledge base plus user overrides.
    KnowledgeBaseWithOverrides {
        id: String,
        source: String,
        overridden: Vec<String>,
    },
    /// Entirely from the user.
    UserSupplied { overridden: Vec<String> },
}

impl ProfileSource {
    pub fn label_zh(&self) -> String {
        match self {
            ProfileSource::KnowledgeBase { id, source } => {
                format!("知识库 {id}（来源：{source}）")
            }
            ProfileSource::KnowledgeBaseWithOverrides {
                id,
                source,
                overridden,
            } => format!(
                "知识库 {id}（来源：{source}）+ 用户覆盖：{}",
                overridden.join("、")
            ),
            ProfileSource::UserSupplied { overridden } => {
                format!("完全由用户提供：{}", overridden.join("、"))
            }
        }
    }
}

/// The knowledge base with its entries.
pub fn profiles() -> Vec<LaserProfile> {
    vec![
        LaserProfile {
            id: "fr4-1.6-isolation".into(),
            material: "FR4 覆铜板（单/双面，1oz 铜）".into(),
            thickness_mm: (1.5, 1.7),
            power_s: 255,
            max_s: 1000,
            speed_mm_min: 200,
            passes: 6,
            applicability: "二极管激光（~2.5W 光功率，405/445nm）隔离雕刻常见起点；\
                            多遍 = 同一条路径重复走（逐遍加深切缝），要加宽隔离缝请增大 clearance\
                            而不是加遍数；首次务必在废料上试刻"
                .into(),
            source: "社区常见做法（无厂商数据）；未在本机验证".into(),
        },
        LaserProfile {
            id: "fr4-0.8-isolation".into(),
            material: "FR4 覆铜板（薄板 0.8mm）".into(),
            thickness_mm: (0.7, 0.9),
            power_s: 200,
            max_s: 1000,
            speed_mm_min: 250,
            passes: 5,
            applicability: "薄板散热差，功率下调；先试刻".into(),
            source: "由 fr4-1.6 按经验按比例下调推导；未在本机验证".into(),
        },
        LaserProfile {
            id: "fr4-outline-1.6".into(),
            material: "FR4 覆铜板 1.6mm 板框切断".into(),
            thickness_mm: (1.5, 1.7),
            power_s: 1000,
            max_s: 1000,
            speed_mm_min: 60,
            passes: 40,
            applicability: "低功率二极管激光切 1.6mm FR4 需要非常多的遍数，通常不现实；\
                            若无更高功率激光或铣刀，请改用机械切割"
                .into(),
            source: "经验推导（FR4 导热且含玻纤，切割效率极低）；未在本机验证".into(),
        },
        LaserProfile {
            id: "fr4-drillmark".into(),
            material: "FR4 覆铜板钻孔标记".into(),
            thickness_mm: (0.5, 2.0),
            power_s: 150,
            max_s: 1000,
            speed_mm_min: 300,
            passes: 3,
            applicability: "只做定位标记（烧穿阻焊/露出铜点），供手电钻或台钻定位；不做通孔".into(),
            source: "本工具默认钻孔模式；未在本机验证".into(),
        },
        LaserProfile {
            id: "fr1-paper-isolation".into(),
            material: "纸基板 / FR-1 覆铜板".into(),
            thickness_mm: (1.0, 2.0),
            power_s: 120,
            max_s: 1000,
            speed_mm_min: 400,
            passes: 3,
            applicability: "纸基板极易起火烧焦，功率必须更低、速度更快，且需要吹气".into(),
            source: "经验推导；切割纸基板有明显火灾风险，未在本机验证".into(),
        },
        LaserProfile {
            id: "cardboard-mockup".into(),
            material: "硬纸板（打样对位用）".into(),
            thickness_mm: (0.5, 3.0),
            power_s: 80,
            max_s: 1000,
            speed_mm_min: 800,
            passes: 1,
            applicability: "先用纸板验证坐标原点、比例与走刀顺序，不要直接上铜板".into(),
            source: "通用做法；本机未验证".into(),
        },
    ]
}

/// Result of a profile query.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileMatch {
    pub profile: Option<LaserProfile>,
    /// Set when the material was not found: near entries for reference only.
    pub nearest: Vec<LaserProfile>,
    /// Human-readable note about the outcome.
    pub note: String,
}

/// Look up a profile by material and thickness.
pub fn lookup(material: &str, thickness_mm: f64) -> ProfileMatch {
    let all = profiles();
    let needle = material.trim();
    let id_match = all
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(needle))
        .cloned();
    if let Some(p) = id_match {
        return ProfileMatch {
            profile: Some(p),
            nearest: vec![],
            note: "按条目 id 命中知识库".into(),
        };
    }
    let name_match = all
        .iter()
        .find(|p| p.material.contains(needle) || needle.contains(&p.material))
        .cloned();
    if let Some(p) = name_match {
        let inside = thickness_mm >= p.thickness_mm.0 && thickness_mm <= p.thickness_mm.1;
        let note = if inside {
            "命中知识库（材质与厚度均在适用范围内）".to_string()
        } else {
            format!(
                "近似命中 {}：该条目适用于 {}-{}mm，你给的 {thickness_mm}mm 在适用范围之外，请勿直接采用",
                p.id, p.thickness_mm.0, p.thickness_mm.1
            )
        };
        return ProfileMatch {
            profile: Some(p),
            nearest: vec![],
            note,
        };
    }
    // No match: return nearest entries for reference, clearly labelled.
    let mut near = all.clone();
    near.sort_by(|a, b| {
        let da = (a.thickness_mm.0 - thickness_mm).abs();
        let db = (b.thickness_mm.0 - thickness_mm).abs();
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    near.truncate(3);
    ProfileMatch {
        profile: None,
        nearest: near,
        note: format!(
            "知识库未收录材质 {needle:?}（{thickness_mm}mm）：下面只是相近条目，仅供参考，\
             不要直接采用，请自行试刻确定参数"
        ),
    }
}

/// Apply user overrides to a profile.
pub fn apply_overrides(profile: &LaserProfile, o: &ProfileOverride) -> (LaserProfile, Vec<String>) {
    let mut p = profile.clone();
    let mut changed = Vec::new();
    if let Some(v) = o.power_s {
        p.power_s = v;
        changed.push(format!("功率 S={v}"));
    }
    if let Some(v) = o.speed_mm_min {
        p.speed_mm_min = v;
        changed.push(format!("速度 F={v}mm/min"));
    }
    if let Some(v) = o.passes {
        p.passes = v;
        changed.push(format!("遍数={v}"));
    }
    if let Some(v) = o.max_s {
        p.max_s = v;
        changed.push(format!("最大 S={v}"));
    }
    if let Some(m) = &o.material {
        p.material = m.clone();
        changed.push(format!("材质={m}"));
    }
    (p, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_base_entries_are_complete_and_sourced() {
        for p in profiles() {
            assert!(!p.id.is_empty());
            assert!(!p.material.is_empty(), "{}", p.id);
            assert!(!p.applicability.is_empty(), "{}", p.id);
            assert!(!p.source.is_empty(), "{} must state its source", p.id);
            assert!(p.power_s <= p.max_s, "{}", p.id);
            assert!(p.speed_mm_min > 0, "{}", p.id);
            assert!(p.passes >= 1, "{}", p.id);
        }
    }

    #[test]
    fn every_entry_admits_it_is_unverified() {
        // Honesty guard: no entry may claim a verified cut on this machine.
        for p in profiles() {
            assert!(
                p.source.contains("未") || p.source.contains("经验") || p.source.contains("常见"),
                "entry {} has a suspicious provenance: {}",
                p.id,
                p.source
            );
        }
    }

    #[test]
    fn lookup_by_id_hits() {
        let m = lookup("fr4-1.6-isolation", 1.6);
        assert!(m.profile.is_some());
        assert_eq!(m.profile.unwrap().id, "fr4-1.6-isolation");
    }

    #[test]
    fn lookup_by_material_name_hits() {
        let m = lookup("FR4", 1.6);
        assert!(m.profile.is_some());
    }

    #[test]
    fn out_of_range_thickness_warns_instead_of_passing() {
        let m = lookup("FR4 覆铜板（薄板 0.8mm）", 3.0);
        assert!(m.profile.is_some());
        assert!(m.note.contains("之外"), "{}", m.note);
        assert!(m.note.contains("请勿直接采用"), "{}", m.note);
    }

    #[test]
    fn unknown_material_returns_nearest_and_refuses_to_assert() {
        let m = lookup("神秘的石头", 5.0);
        assert!(m.profile.is_none());
        assert!(!m.nearest.is_empty());
        assert!(m.note.contains("未收录"), "{}", m.note);
        assert!(m.note.contains("不要直接采用"), "{}", m.note);
    }

    #[test]
    fn overrides_are_recorded() {
        let p = lookup("fr4-1.6-isolation", 1.6).profile.unwrap();
        let o = ProfileOverride {
            power_s: Some(300),
            speed_mm_min: Some(150),
            ..Default::default()
        };
        let (q, changed) = apply_overrides(&p, &o);
        assert_eq!(q.power_s, 300);
        assert_eq!(q.speed_mm_min, 150);
        assert_eq!(q.passes, p.passes, "untouched fields keep the KB value");
        assert_eq!(changed.len(), 2);
        assert!(changed[0].contains("功率"));
    }

    #[test]
    fn disclaimer_is_mandatory_text() {
        assert!(DISCLAIMER.contains("试刻"));
        assert!(DISCLAIMER.contains("未在真实激光机上验证"));
    }
}
