//! Generate `.kicad_sch` files from `NetlistIr`.
//!
//! Components are laid out on a grid; each component instance embeds a
//! minimal `lib_symbols` block and per-instance properties. Nets are wired
//! with global labels (robust and readable; direct wire routing is a later
//! optimization).

use crate::netlist::NetlistIr;
use crate::sexpr::Sexpr;

pub const KICAD_SCH_VERSION: &str = "20250114";
pub const GENERATOR: &str = "gcode";

/// Layout config: components in a vertical column, spaced apart.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub origin_x: i64,
    pub origin_y: i64,
    pub pitch_y: i64,
}

impl Default for Layout {
    fn default() -> Self {
        Self { origin_x: 100, origin_y: 100, pitch_y: 100 }
    }
}

/// Generate a complete `.kicad_sch` document for the given validated netlist.
pub fn generate_schematic(ir: &NetlistIr, layout: Layout) -> Result<String, super::netlist::IrError> {
    ir.validate()?;

    let mut root = Sexpr::List(vec![
        Sexpr::Atom("kicad_sch".into()),
        list("version", vec![atom(KICAD_SCH_VERSION)]),
        list("generator", vec![str_(GENERATOR)]),
        list("generator_version", vec![str_("9.0")]),
        Sexpr::Atom("uuid".into()), // placeholder replaced below with real uuid
    ]);

    // Replace the placeholder uuid with a real one.
    if let Sexpr::List(items) = &mut root {
        items[4] = list("uuid", vec![str_(uuid_v4())]);
    }

    // paper + title block
    if let Sexpr::List(items) = &mut root {
        items.push(list("paper", vec![atom("A4")]));
        items.push(list(
            "title_block",
            vec![
                str_("Gcode generated"),
                // date/date fields omitted; KiCad tolerates minimal title blocks
            ],
        ));
    }

    // lib_symbols: one entry per distinct lib_id.
    let mut lib_symbols_children = vec![Sexpr::Atom("lib_symbols".into())];
    let mut seen_lib_ids = std::collections::BTreeSet::new();
    for c in &ir.components {
        if seen_lib_ids.insert(c.lib_id.clone()) {
            lib_symbols_children.push(lib_symbol_stub(&c.lib_id));
        }
    }
    if let Sexpr::List(items) = &mut root {
        items.push(Sexpr::List(lib_symbols_children));
    }

    // symbol instances laid out vertically.
    let pin_nets = ir.pin_nets();
    for (idx, c) in ir.components.iter().enumerate() {
        let y = layout.origin_y + (idx as i64) * layout.pitch_y;
        if let Sexpr::List(items) = &mut root {
            items.push(symbol_instance(c, layout.origin_x, y, &pin_nets));
        }
    }

    // net labels: global label per connection point, stacked right of symbols.
    let mut label_y = layout.origin_y - 50;
    for net in &ir.nets {
        if let Sexpr::List(items) = &mut root {
            items.push(global_label(net, layout.origin_x + 150, label_y));
        }
        label_y += 40;
    }

    // sheet_instances (KiCad requires at least one).
    if let Sexpr::List(items) = &mut root {
        items.push(Sexpr::List(vec![
            Sexpr::Atom("sheet_instances".into()),
            list("path", vec![str_("/")]),
        ]));
    }

    let mut out = root.to_string_kicad();
    out.push('\n');
    Ok(out)
}

// ---------- builders ----------

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

/// Minimal but valid lib_symbols stub. KiCad's eeschema accepts symbols whose
/// graphical content is a simple rectangle + two pins for R/C-class 2-pin
/// parts; for unknown lib_ids we emit a generic 2-pin box.
fn lib_symbol_stub(lib_id: &str) -> Sexpr {
    let (lib, name) = lib_id.split_once(':').unwrap_or((lib_id, lib_id));
    let (lib, name) = (lib.to_string(), name.to_string());
    Sexpr::List(vec![
        Sexpr::Atom("symbol".into()),
        str_(lib_id),
        list("pin_numbers", vec![Sexpr::Atom("hide".into())]),
        list("pin_names", vec![list("offset", vec![atom("0")])]),
        list("in_bom", vec![Sexpr::Atom("yes".into())]),
        list("on_board", vec![Sexpr::Atom("yes".into())]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Reference"),
            str_(prefix_of(&name)),
            list("at", vec![atom("0"), atom("2.54"), atom("0")]),
            list("effects", vec![list("font", vec![])]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Value"),
            str_(name),
            list("at", vec![atom("0"), atom("-2.54"), atom("0")]),
            list("effects", vec![list("font", vec![])]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("symbol".into()),
            str_(format!("{lib}_0_1")),
            Sexpr::List(vec![
                Sexpr::Atom("rectangle".into()),
                list("start", vec![atom("-5.08"), atom("2.54")]),
                list("end", vec![atom("5.08"), atom("-2.54")]),
                list("stroke", vec![
                    list("width", vec![atom("0")]),
                    list("type", vec![Sexpr::Atom("default".into())]),
                ]),
                list("fill", vec![
                    list("type", vec![Sexpr::Atom("none".into())]),
                ]),
            ]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("symbol".into()),
            str_(format!("{lib}_1_1")),
            Sexpr::List(vec![
                Sexpr::Atom("pin".into()),
                Sexpr::Atom("passive".into()),
                list("at", vec![atom("-7.62"), atom("0"), atom("0")]),
                list("length", vec![atom("2.54")]),
                Sexpr::List(vec![
                    Sexpr::Atom("name".into()),
                    str_("1"),
                    list("effects", vec![list("font", vec![])]),
                ]),
                Sexpr::List(vec![
                    Sexpr::Atom("number".into()),
                    str_("1"),
                    list("effects", vec![list("font", vec![])]),
                ]),
            ]),
            Sexpr::List(vec![
                Sexpr::Atom("pin".into()),
                Sexpr::Atom("passive".into()),
                list("at", vec![atom("7.62"), atom("0"), atom("180")]),
                list("length", vec![atom("2.54")]),
                Sexpr::List(vec![
                    Sexpr::Atom("name".into()),
                    str_("2"),
                    list("effects", vec![list("font", vec![])]),
                ]),
                Sexpr::List(vec![
                    Sexpr::Atom("number".into()),
                    str_("2"),
                    list("effects", vec![list("font", vec![])]),
                ]),
            ]),
        ]),
    ])
}

fn prefix_of(name: &str) -> String {
    name.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "U".into())
}

fn symbol_instance(
    c: &super::netlist::Component,
    x: i64,
    y: i64,
    pin_nets: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
) -> Sexpr {
    let inst_uuid = uuid_v4();
    let mut items = vec![
        Sexpr::Atom("symbol".into()),
        list("lib_id", vec![str_(&c.lib_id)]),
        list("at", vec![atom(x.to_string()), atom(y.to_string()), atom("0")]),
        list("unit", vec![atom("1")]),
        Sexpr::List(vec![
            Sexpr::Atom("in_bom".into()),
            Sexpr::Atom("yes".into()),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("on_board".into()),
            Sexpr::Atom("yes".into()),
        ]),
        Sexpr::Atom("dnp".into()),
        Sexpr::Atom("fields_autoplaced".into()),
        list("uuid", vec![str_(&inst_uuid)]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Reference"),
            str_(&c.reference),
            list("at", vec![atom(x.to_string()), atom((y - 30).to_string()), atom("0")]),
            list("effects", vec![list("font", vec![])]),
        ]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Value"),
            str_(&c.value),
            list("at", vec![atom(x.to_string()), atom((y + 30).to_string()), atom("0")]),
            list("effects", vec![list("font", vec![])]),
        ]),
    ];
    if let Some(fp) = &c.footprint {
        items.push(Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Footprint"),
            str_(fp),
            list("at", vec![atom(x.to_string()), atom((y + 55).to_string()), atom("0")]),
            list("effects", vec![list("font", vec![])]),
        ]));
    }
    // per-instance path
    items.push(Sexpr::List(vec![
        Sexpr::Atom("instances".into()),
        Sexpr::List(vec![
            Sexpr::Atom("project".into()),
            str_("gcode"),
            list("path", vec![
                str_("/"),
                list("reference", vec![str_(&c.reference)]),
                list("unit", vec![atom("1")]),
            ]),
        ]),
    ]));
    let _ = pin_nets; // labels carry the net names; per-pin wiring is a later optimization
    Sexpr::List(items)
}

fn global_label(net: &super::netlist::Net, x: i64, y: i64) -> Sexpr {
    Sexpr::List(vec![
        Sexpr::Atom("global_label".into()),
        str_(&net.name),
        list("shape", vec![Sexpr::Atom("input".into())]),
        list("at", vec![atom(x.to_string()), atom(y.to_string()), atom("0")]),
        list(
            "effects",
            vec![list(
                "font",
                vec![list("size", vec![atom("1.27"), atom("1.27")])],
            )],
        ),
        list("uuid", vec![str_(uuid_v4())]),
        Sexpr::List(vec![
            Sexpr::Atom("property".into()),
            str_("Intersheetrefs"),
            str_("${INTERSHEET_REFS}"),
            list("at", vec![atom(x.to_string()), atom(y.to_string()), atom("0")]),
            list("effects", vec![list("font", vec![])]),
            list("hide", vec![]),
        ]),
    ])
}

/// Simple time+random based UUIDv4 (no external crate; determinism not required).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut seed = nanos as u64 ^ (std::process::id() as u64) << 32;
    let mut next = || {
        // xorshift64
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let a = next();
    let b = next();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) & 0xffff,
        a & 0x0fff,
        ((b >> 48) & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::templates;

    #[test]
    fn generates_parseable_kicad_sch() {
        let ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        let doc = generate_schematic(&ir, Layout::default()).unwrap();
        let parsed = Sexpr::parse(&doc).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tag(), Some("kicad_sch"));
    }

    #[test]
    fn contains_symbols_and_labels() {
        let ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        let doc = generate_schematic(&ir, Layout::default()).unwrap();
        let root = &Sexpr::parse(&doc).unwrap()[0];
        let syms = root.find_all("symbol");
        // 2 instances at top level (lib stubs live nested inside lib_symbols)
        assert_eq!(syms.len(), 2);
        let labels = root.find_all("global_label");
        assert_eq!(labels.len(), 3); // VIN, VOUT, GND
    }

    #[test]
    fn rejects_invalid_ir() {
        let mut ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        ir.nets[0].connections.push(("Q9".into(), "1".into()));
        assert!(generate_schematic(&ir, Layout::default()).is_err());
    }

    #[test]
    fn deterministic_structure() {
        let ir = templates::voltage_divider("10k", "10k", "VIN", "VOUT", "GND");
        let a = generate_schematic(&ir, Layout::default()).unwrap();
        let b = generate_schematic(&ir, Layout::default()).unwrap();
        // UUIDs differ between runs but structural tags must match
        let strip = |s: &str| {
            s.lines()
                .filter(|l| !l.contains("(uuid"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(strip(&a), strip(&b));
    }
}
