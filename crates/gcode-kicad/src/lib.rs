//! gcode-kicad: KiCad engine for the Gcode agent.
//!
//! Layers:
//! - [`sexpr`]: s-expression parser/writer for KiCad file formats
//! - [`netlist`]: netlist IR (the LLM ↔ deterministic boundary)
//! - [`schematic`]: `.kicad_sch` generation from IR

pub mod erc;
pub mod netlist;
pub mod schematic;
pub mod sexpr;

pub use erc::{ErcError, ErcReport};
pub use netlist::{Component, IrError, Net, NetlistIr};
pub use schematic::{generate_schematic, Layout};
