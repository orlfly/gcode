//! Generate `.kicad_sch` files from `NetlistIr`.
//!
//! Output mirrors the KiCad 7 s-expression form (`version 20230121`), which is
//! the most widely readable token: every later KiCad reads it and upgrades on
//! save. The writer is deterministic: identical IR + layout produce
//! byte-identical files.
//!
//! Connectivity is real, not decorative. Each connected pin gets a label
//! placed exactly at the pin's connection point, so KiCad's netlister (and
//! therefore ERC) sees the intended nets. Symbol definitions are embedded from
//! the local KiCad symbol library so pins, pin numbers and electrical types
//! are the real ones; a generic stub is used only as a last resort and is
//! reported as a warning.

use crate::netlist::{Component, IrError, NetlistIr};
use crate::sexpr::Sexpr;
use crate::symbols::{self, PinInfo, SymbolLibrary};
use std::collections::BTreeMap;

/// KiCad schematic format token. 20260306 is what KiCad 10 writes; the file is
/// verified against a real `kicad-cli` 10 (load + netlist + ERC).
pub const KICAD_SCH_VERSION: &str = "20260306";
/// Generator name written into the file.
pub const GENERATOR: &str = "gcode";
/// Generator version, matching the format we emit.
pub const GENERATOR_VERSION: &str = "10.0";
/// Project name used in per-symbol `instances` blocks.
pub const PROJECT: &str = "gcode";

/// Deterministic UUID namespace for Gcode-generated files.
const GCODE_NS: uuid::Uuid = uuid::Uuid::from_bytes([
    0x67, 0x63, 0x6f, 0x64, 0x65, 0x2d, 0x34, 0x3a, 0x9b, 0x1e, 0x00, 0x11, 0x22, 0x33, 0x44, 0x56,
]);

/// Layout config. Components are placed in a deterministic column; the anchor
/// is the top-left corner of the first component's bounding box.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub origin_x: f64,
    pub origin_y: f64,
    /// Minimum vertical gap between consecutive component bounding boxes (mm).
    pub min_gap: f64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            origin_x: 76.2,
            origin_y: 50.8,
            min_gap: 10.16,
        }
    }
}

/// Where a component ended up, in schematic millimetres.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub reference: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Result of a generation run.
#[derive(Debug, Clone)]
pub struct GenerationReport {
    pub content: String,
    pub warnings: Vec<String>,
    /// `lib_id`s that had to fall back to the generic stub.
    pub missing_symbols: Vec<String>,
    pub placements: Vec<Placement>,
}

/// Generate a `.kicad_sch` document, discarding the diagnostics.
pub fn generate_schematic(ir: &NetlistIr, layout: Layout) -> Result<String, IrError> {
    Ok(generate_with_report(ir, layout)?.content)
}

/// Generate a `.kicad_sch` document plus diagnostics.
pub fn generate_with_report(ir: &NetlistIr, layout: Layout) -> Result<GenerationReport, IrError> {
    ir.validate()?;
    let ir = ir.normalized();

    let mut warnings = Vec::new();
    let mut missing_symbols = Vec::new();
    let mut lib = SymbolLibrary::locate();
    if lib.dir().is_none() {
        warnings.push(
            "未找到 KiCad 符号库（可用 GCODE_KICAD_SYMBOL_DIR 指定），已回退到通用符号；ERC 结果可能失真"
                .to_string(),
        );
    }

    // Root document identity is content-derived, so re-generating the same
    // circuit yields the same file.
    let ir_key = ir.to_sexpr().to_string_kicad();
    let root_uuid = det_uuid(&format!("sch:{ir_key}"));

    // Embedded symbol definitions, one per distinct lib_id (sorted).
    let mut embedded: BTreeMap<String, Sexpr> = BTreeMap::new();
    let mut resolved: BTreeMap<String, Vec<PinInfo>> = BTreeMap::new();
    for c in &ir.components {
        if embedded.contains_key(&c.lib_id) {
            continue;
        }
        match lib.symbol(&c.lib_id) {
            Some(sym) => {
                resolved.insert(c.lib_id.clone(), symbols::pins_of(&sym));
                embedded.insert(c.lib_id.clone(), sym);
            }
            None => {
                if !missing_symbols.contains(&c.lib_id) {
                    missing_symbols.push(c.lib_id.clone());
                    warnings.push(format!(
                        "符号 {} 不在本地库中，已用通用两脚符号代替（引脚语义丢失）",
                        c.lib_id
                    ));
                }
                let stub = stub_symbol(&c.lib_id);
                resolved.insert(c.lib_id.clone(), symbols::pins_of(&stub));
                embedded.insert(c.lib_id.clone(), stub);
            }
        }
    }

    // Deterministic column placement, sized by each symbol's real bounding box
    // so bodies never overlap.
    let mut cursor_y = layout.origin_y;
    let mut placements = Vec::new();
    let mut placed: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for c in &ir.components {
        let sym = &embedded[&c.lib_id];
        let b = bbox_of(sym).unwrap_or(Bounds {
            min_x: -5.08,
            min_y: -2.54,
            max_x: 5.08,
            max_y: 2.54,
        });
        let sx = layout.origin_x - b.min_x;
        let sy = cursor_y + b.max_y;
        cursor_y += (b.max_y - b.min_y) + layout.min_gap;
        placed.insert(c.reference.clone(), (sx, sy));
        placements.push(Placement {
            reference: c.reference.clone(),
            x: sx,
            y: sy,
            width: b.max_x - b.min_x,
            height: b.max_y - b.min_y,
        });
    }

    // Overlap check (defensive: the cursor guarantees it, this proves it).
    for i in 0..placements.len() {
        for j in (i + 1)..placements.len() {
            let a = &placements[i];
            let b = &placements[j];
            let overlap = (a.x - b.x).abs() * 2.0 < (a.width + b.width)
                && (a.y - b.y).abs() * 2.0 < (a.height + b.height);
            if overlap {
                warnings.push(format!(
                    "组件 {} 与 {} 的符号框重叠，请调整布局参数",
                    a.reference, b.reference
                ));
            }
        }
    }

    let mut root = Vec::new();
    root.push(Sexpr::Atom("kicad_sch".into()));
    root.push(list("version", vec![atom(KICAD_SCH_VERSION)]));
    root.push(list("generator", vec![str_(GENERATOR)]));
    root.push(list("generator_version", vec![str_(GENERATOR_VERSION)]));
    root.push(list("uuid", vec![str_(&root_uuid)]));
    root.push(list("paper", vec![str_("A4")]));
    root.push(list(
        "title_block",
        vec![list("title", vec![str_("Gcode generated")])],
    ));

    let mut lib_children = vec![Sexpr::Atom("lib_symbols".into())];
    for sym in embedded.values() {
        lib_children.push(sym.clone());
    }
    root.push(Sexpr::List(lib_children));

    // Symbol instances.
    for c in &ir.components {
        let (sx, sy) = placed[&c.reference];
        let sym = &embedded[&c.lib_id];
        let is_power = sym.find("power").is_some();
        root.push(symbol_instance(c, sym, sx, sy, &root_uuid, is_power));
    }

    // Labels: one per connected pin, at the pin's exact connection point.
    let mut label_sites: BTreeMap<(String, String), String> = BTreeMap::new(); // coord -> net
    for net in &ir.nets {
        for (comp, pin) in &net.connections {
            let Some((sx, sy)) = placed.get(comp) else {
                continue;
            };
            let Some(pins) = resolved.get(
                &ir.components
                    .iter()
                    .find(|c| &c.reference == comp)
                    .map(|c| c.lib_id.clone())
                    .unwrap_or_default(),
            ) else {
                continue;
            };
            let Some(p) = pins.iter().find(|p| &p.number == pin) else {
                warnings.push(format!(
                    "网 {} 引用 {}.{}，但符号 {} 没有该引脚号（连接被忽略）",
                    net.name,
                    comp,
                    pin,
                    ir.component(comp).map(|c| c.lib_id.as_str()).unwrap_or("?"),
                ));
                continue;
            };
            // Symbol library Y grows up, schematic Y grows down.
            let x = sx + p.x;
            let y = sy - p.y;
            let key = (fmt_num(x), fmt_num(y));
            if let Some(other) = label_sites.get(&key) {
                if other != &net.name {
                    warnings.push(format!(
                        "网 {} 与 {} 的标签落在同一坐标 ({x}, {y})，会短路，请调整布局",
                        other, net.name
                    ));
                }
                continue;
            }
            label_sites.insert(key, net.name.clone());
            root.push(label(&net.name, x, y, &format!("{}:{}", comp, pin)));
        }
    }

    root.push(Sexpr::List(vec![
        Sexpr::Atom("sheet_instances".into()),
        Sexpr::List({
            let mut p = vec![Sexpr::Atom("path".into()), str_("/")];
            p.push(list("page", vec![str_("1")]));
            p
        }),
    ]));

    let mut content = Sexpr::List(root).to_string_kicad();
    content.push('\n');

    Ok(GenerationReport {
        content,
        warnings,
        missing_symbols,
        placements,
    })
}

// ---------- helpers ----------

fn atom(s: impl Into<String>) -> Sexpr {
    Sexpr::Atom(s.into())
}
fn str_(s: impl Into<String>) -> Sexpr {
    Sexpr::Str(s.into())
}
fn list(tag: &str, args: Vec<Sexpr>) -> Sexpr {
    let mut items = vec![Sexpr::Atom(tag.to_string())];
    items.extend(args);
    Sexpr::List(items)
}

/// Deterministic UUIDv5 in the Gcode namespace.
pub fn det_uuid(seed: &str) -> String {
    uuid::Uuid::new_v5(&GCODE_NS, seed.as_bytes()).to_string()
}

/// KiCad-friendly number formatting (no trailing zeros, no scientific notation).
pub fn fmt_num(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// Generic two-pin fallback symbol, used only when the library lookup fails.
fn stub_symbol(lib_id: &str) -> Sexpr {
    let (_lib, name) = lib_id.split_once(':').unwrap_or((lib_id, lib_id));
    let prefix = name.chars().next().map(|c| c.to_string()).unwrap_or("U".into());
    let pin = |num: &str, x: &str, angle: &str| {
        Sexpr::List(vec![
            Sexpr::Atom("pin".into()),
            Sexpr::Atom("passive".into()),
            Sexpr::Atom("line".into()),
            list("at", vec![atom(x), atom("0"), atom(angle)]),
            list("length", vec![atom("2.54")]),
            Sexpr::List(vec![
                Sexpr::Atom("name".into()),
                str_(num),
                list("effects", vec![list("font", vec![list("size", vec![atom("1.27"), atom("1.27")])])]),
            ]),
            Sexpr::List(vec![
                Sexpr::Atom("number".into()),
                str_(num),
                list("effects", vec![list("font", vec![list("size", vec![atom("1.27"), atom("1.27")])])]),
            ]),
        ])
    };
    Sexpr::List(vec![
        Sexpr::Atom("symbol".into()),
        str_(lib_id),
        list("pin_names", vec![list("offset", vec![atom("0.254")])]),
        list("exclude_from_sim", vec![Sexpr::Atom("no".into())]),
        list("in_bom", vec![Sexpr::Atom("yes".into())]),
        list("on_board", vec![Sexpr::Atom("yes".into())]),
        list("in_pos_files", vec![Sexpr::Atom("yes".into())]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Reference"),
            str_(prefix),
            list("at", vec![atom("0"), atom("2.54"), atom("0")]),
            list("effects", vec![list("font", vec![list("size", vec![atom("1.27"), atom("1.27")])])]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Value"),
            str_(name),
            list("at", vec![atom("0"), atom("-2.54"), atom("0")]),
            list("effects", vec![list("font", vec![list("size", vec![atom("1.27"), atom("1.27")])])]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("symbol".into()),
            str_(format!("{name}_0_1")),
            Sexpr::List(vec![
                Sexpr::Atom("rectangle".into()),
                list("start", vec![atom("-5.08"), atom("2.54")]),
                list("end", vec![atom("5.08"), atom("-2.54")]),
                list("stroke", vec![list("width", vec![atom("0")]), list("type", vec![Sexpr::Atom("default".into())])]),
                list("fill", vec![list("type", vec![Sexpr::Atom("none".into())])]),
            ]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("symbol".into()),
            str_(format!("{name}_1_1")),
            pin("1", "-7.62", "0"),
            pin("2", "7.62", "180"),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Datasheet"),
            str_(""),
            list("at", vec![atom("0"), atom("0"), atom("0")]),
            list("hide", vec![Sexpr::Atom("yes".into())]),
            list("show_name", vec![Sexpr::Atom("no".into())]),
            list("do_not_autoplace", vec![Sexpr::Atom("no".into())]),
            list(
                "effects",
                vec![list(
                    "font",
                    vec![list("size", vec![atom("1.27"), atom("1.27")])],
                )],
            ),
        ]),
    ])
}

fn symbol_instance(
    c: &Component,
    sym: &Sexpr,
    sx: f64,
    sy: f64,
    root_uuid: &str,
    is_power: bool,
) -> Sexpr {
    let mut items = vec![
        Sexpr::Atom("symbol".into()),
        list("lib_id", vec![str_(&c.lib_id)]),
        list(
            "at",
            vec![atom(fmt_num(sx)), atom(fmt_num(sy)), atom("0")],
        ),
        list("unit", vec![atom("1")]),
        list("body_style", vec![atom("1")]),
        list("exclude_from_sim", vec![Sexpr::Atom("no".into())]),
        list("in_bom", vec![Sexpr::Atom("yes".into())]),
        list("on_board", vec![Sexpr::Atom("yes".into())]),
        list("in_pos_files", vec![Sexpr::Atom("yes".into())]),
        list("dnp", vec![Sexpr::Atom("no".into())]),
        list("uuid", vec![str_(&det_uuid(&format!("inst:{}:{}", c.lib_id, c.reference)))]),
        // Reference above, Value below the symbol body.
        property(
            "Reference",
            &c.reference,
            sx,
            sy,
            is_power,
            &format!("{}:ref", c.reference),
        ),
        property(
            "Value",
            &c.value,
            sx,
            sy + 5.08,
            is_power,
            &format!("{}:value", c.reference),
        ),
        if let Some(fp) = &c.footprint {
            property(
                "Footprint",
                fp,
                sx,
                sy + 7.62,
                true,
                &format!("{}:footprint", c.reference),
            )
        } else {
            property("Footprint", "", sx, sy + 7.62, true, &format!("{}:footprint", c.reference))
        },
        property("Datasheet", "", sx, sy + 10.16, true, &format!("{}:datasheet", c.reference)),
    ];

    for p in symbols::pins_of(sym) {
        items.push(Sexpr::List(vec![
            Sexpr::Atom("pin".into()),
            str_(&p.number),
            list(
                "uuid",
                vec![str_(&det_uuid(&format!("pin:{}:{}", c.reference, p.number)))],
            ),
        ]));
    }

    // (instances (project "gcode" (path "/<root-uuid>" (reference "R1") (unit 1))))
    let mut path_items = vec![
        Sexpr::Atom("path".into()),
        str_(format!("/{root_uuid}")),
    ];
    path_items.push(list("reference", vec![str_(&c.reference)]));
    path_items.push(list("unit", vec![atom("1")]));
    items.push(Sexpr::List(vec![
        Sexpr::Atom("instances".into()),
        Sexpr::List(vec![
            Sexpr::Atom("project".into()),
            str_(PROJECT),
            Sexpr::List(path_items),
        ]),
    ]));

    Sexpr::List(items)
}

fn property(
    name: &str,
    value: &str,
    sx: f64,
    sy: f64,
    hide: bool,
    _seed: &str,
) -> Sexpr {
    // KiCad 10 property form: at, [hide yes], show_name, do_not_autoplace, effects.
    let mut items = vec![
        Sexpr::Atom("property".into()),
        str_(name),
        str_(value),
        list("at", vec![atom(fmt_num(sx)), atom(fmt_num(sy)), atom("0")]),
    ];
    if hide {
        items.push(list("hide", vec![Sexpr::Atom("yes".into())]));
    }
    items.push(list("show_name", vec![Sexpr::Atom("no".into())]));
    items.push(list("do_not_autoplace", vec![Sexpr::Atom("no".into())]));
    items.push(list(
        "effects",
        vec![list(
            "font",
            vec![list("size", vec![atom("1.27"), atom("1.27")])],
        )],
    ));
    Sexpr::List(items)
}

fn label(name: &str, x: f64, y: f64, seed: &str) -> Sexpr {
    Sexpr::List(vec![
        Sexpr::Atom("label".into()),
        str_(name),
        list("at", vec![atom(fmt_num(x)), atom(fmt_num(y)), atom("0")]),
        Sexpr::List(vec![
            Sexpr::Atom("effects".into()),
            list("font", vec![list("size", vec![atom("1.27"), atom("1.27")])]),
            list(
                "justify",
                vec![Sexpr::Atom("left".into()), Sexpr::Atom("bottom".into())],
            ),
        ]),
        list("uuid", vec![str_(&det_uuid(&format!("label:{seed}:{name}")))]),
    ])
}

// ---------- geometry ----------

#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

/// Bounding box of a symbol's graphical body and pin endpoints, in symbol
/// coordinates (library convention, Y up).
pub fn bbox_of(sym: &Sexpr) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    let mut add = |x: f64, y: f64| {
        b = Some(match b {
            None => Bounds {
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
            },
            Some(v) => Bounds {
                min_x: v.min_x.min(x),
                min_y: v.min_y.min(y),
                max_x: v.max_x.max(x),
                max_y: v.max_y.max(y),
            },
        });
    };
    walk_geometry(sym, &mut add);
    b
}

fn walk_geometry(node: &Sexpr, add: &mut impl FnMut(f64, f64)) {
    match node.tag() {
        Some("rectangle") => {
            if let (Some((x1, y1)), Some((x2, y2))) = (xy_of(node, "start"), xy_of(node, "end")) {
                add(x1, y1);
                add(x2, y2);
            }
        }
        Some("circle") => {
            if let (Some((cx, cy)), Some(r)) = (
                xy_of(node, "center"),
                node.get("radius").and_then(Sexpr::as_str).and_then(|s| s.parse::<f64>().ok()),
            ) {
                add(cx - r, cy - r);
                add(cx + r, cy + r);
            }
        }
        Some("polyline") | Some("pts") => {
            for p in node.find_all("xy") {
                if let (Some(x), Some(y)) = (num_at(p, 1), num_at(p, 2)) {
                    add(x, y);
                }
            }
        }
        Some("arc") => {
            for tag in ["start", "mid", "end"] {
                if let Some((x, y)) = xy_of(node, tag) {
                    add(x, y);
                }
            }
        }
        Some("pin") => {
            if let Some((x, y)) = xy_of(node, "at") {
                add(x, y);
            }
        }
        _ => {}
    }
    if let Some(items) = node.as_list() {
        for c in items {
            walk_geometry(c, add);
        }
    }
}

fn num_at(node: &Sexpr, idx: usize) -> Option<f64> {
    node.as_list()?.get(idx)?.as_str()?.parse().ok()
}

fn xy_of(node: &Sexpr, tag: &str) -> Option<(f64, f64)> {
    let child = node.find(tag)?;
    Some((num_at(child, 1)?, num_at(child, 2)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    fn divider() -> NetlistIr {
        catalog::by_id("voltage-divider").unwrap().ir
    }

    #[test]
    fn generates_parseable_kicad_sch() {
        let doc = generate_schematic(&divider(), Layout::default()).unwrap();
        let parsed = Sexpr::parse(&doc).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tag(), Some("kicad_sch"));
    }

    #[test]
    fn embeds_real_symbol_definitions() {
        let report = generate_with_report(&divider(), Layout::default()).unwrap();
        assert!(
            report.missing_symbols.is_empty(),
            "expected library symbols, missing: {:?}",
            report.missing_symbols
        );
        let root = &Sexpr::parse(&report.content).unwrap()[0];
        let libs = root.find("lib_symbols").unwrap();
        let names: Vec<String> = libs
            .find_all("symbol")
            .into_iter()
            .filter_map(|s| s.as_list()?.get(1)?.as_str().map(str::to_string))
            .collect();
        assert!(names.contains(&"Device:R".to_string()));
        assert!(names.contains(&"power:PWR_FLAG".to_string()));
    }

    #[test]
    fn every_connected_pin_gets_a_label() {
        let ir = divider();
        let report = generate_with_report(&ir, Layout::default()).unwrap();
        let root = &Sexpr::parse(&report.content).unwrap()[0];
        let labels = root.find_all("label");
        let expected: usize = ir.nets.iter().map(|n| n.connections.len()).sum();
        assert_eq!(labels.len(), expected, "one label per connected pin");
        let mut names: Vec<&str> = labels
            .iter()
            .filter_map(|l| l.as_list()?.get(1)?.as_str())
            .collect();
        names.sort();
        assert!(names.contains(&"VOUT"));
        assert!(names.contains(&"GND"));
    }

    #[test]
    fn output_is_byte_identical_across_runs() {
        for entry in catalog::all() {
            let a = generate_schematic(&entry.ir, Layout::default()).unwrap();
            let b = generate_schematic(&entry.ir, Layout::default()).unwrap();
            assert_eq!(a, b, "{} is not deterministic", entry.id);
        }
    }

    #[test]
    fn output_is_insensitive_to_input_order() {
        let mut a = divider();
        let mut b = divider();
        b.components.reverse();
        b.nets.reverse();
        for n in &mut b.nets {
            n.connections.reverse();
        }
        assert_eq!(
            generate_schematic(&a, Layout::default()).unwrap(),
            generate_schematic(&b, Layout::default()).unwrap()
        );
        // and the layout-relevant order matches too
        a.components.reverse();
        assert_eq!(a.normalized().components, b.normalized().components);
    }

    #[test]
    fn no_overlap_across_catalog() {
        for entry in catalog::all() {
            let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
            assert!(
                !report
                    .warnings
                    .iter()
                    .any(|w| w.contains("重叠")),
                "{}: {:?}",
                entry.id,
                report.warnings
            );
        }
    }

    #[test]
    fn no_accidental_short_across_catalog() {
        for entry in catalog::all() {
            let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
            assert!(
                !report.warnings.iter().any(|w| w.contains("短路")),
                "{}: {:?}",
                entry.id,
                report.warnings
            );
        }
    }

    #[test]
    fn roundtrip_fidelity() {
        // Every catalog circuit: re-parse the output and recover refs + nets.
        for entry in catalog::all() {
            let report = generate_with_report(&entry.ir, Layout::default()).unwrap();
            let root = &Sexpr::parse(&report.content).unwrap()[0];
            let mut refs: Vec<String> = root
                .find_all("symbol")
                .into_iter()
                .filter(|s| s.find("lib_id").is_some())
                .filter_map(|s| {
                    s.find_all("property")
                        .into_iter()
                        .find(|p| {
                            p.as_list().and_then(|l| l.get(1)).and_then(Sexpr::as_str)
                                == Some("Reference")
                        })
                        .and_then(|p| p.as_list()?.get(2))
                        .and_then(Sexpr::as_str)
                        .map(str::to_string)
                })
                .collect();
            refs.sort();
            let mut expected = entry.ir.references();
            expected.sort();
            assert_eq!(refs, expected, "{}: references differ", entry.id);

            let mut got_nets: Vec<String> = root
                .find_all("label")
                .into_iter()
                .filter_map(|l| l.as_list()?.get(1)?.as_str().map(str::to_string))
                .collect();
            got_nets.sort();
            got_nets.dedup();
            let mut want_nets: Vec<String> =
                entry.ir.nets.iter().map(|n| n.name.clone()).collect();
            want_nets.sort();
            want_nets.dedup();
            assert_eq!(got_nets, want_nets, "{}: labels differ", entry.id);
        }
    }

    #[test]
    fn rejects_invalid_ir_without_writing() {
        let mut ir = divider();
        ir.nets[0].connections.push(("Q9".into(), "1".into()));
        assert!(generate_with_report(&ir, Layout::default()).is_err());
    }

    #[test]
    fn unknown_symbol_falls_back_with_warning() {
        let ir = NetlistIr::new(
            vec![
                crate::netlist::Component::new("R1", "NoSuchLib:R", "1k"),
                crate::netlist::Component::new("R2", "Device:R", "1k"),
            ],
            vec![
                crate::netlist::Net::new("A", &[("R1", "1"), ("R2", "1")]),
                crate::netlist::Net::new("B", &[("R1", "2"), ("R2", "2")]),
            ],
        );
        let report = generate_with_report(&ir, Layout::default()).unwrap();
        assert_eq!(report.missing_symbols, vec!["NoSuchLib:R".to_string()]);
        assert!(report.warnings.iter().any(|w| w.contains("NoSuchLib:R")));
    }

    #[test]
    fn fmt_num_is_kicad_friendly() {
        assert_eq!(fmt_num(0.0), "0");
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(76.2), "76.2");
        assert_eq!(fmt_num(1.0 / 3.0), "0.3333");
        assert_eq!(fmt_num(-7.62), "-7.62");
    }
}
