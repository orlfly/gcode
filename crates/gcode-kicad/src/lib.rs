//! gcode-kicad: KiCad engine for the Gcode agent.
//!
//! Layers:
//! - [`sexpr`]: s-expression parser/writer for KiCad file formats
//! - [`netlist`]: netlist IR (the LLM ↔ deterministic boundary)
//! - [`symbols`]: KiCad symbol library resolution (real pin definitions)
//! - [`schematic`]: deterministic `.kicad_sch` generation with real connectivity
//! - [`erc`]: `kicad-cli` ERC bridge, interpretation and self-check loop
//! - [`catalog`]: the 10 typical circuits used for KR1 acceptance
//! - [`edit`]: IR-level diff for conversational modification
//! - [`reader`]: recover a netlist from an existing `.kicad_sch`

pub mod catalog;
pub mod edit;
pub mod erc;
pub mod netlist;
pub mod reader;
pub mod schematic;
pub mod sexpr;
pub mod symbols;

pub use catalog::{all as catalog_all, by_id as catalog_by_id, CircuitEntry};
pub use edit::{diff as diff_ir, IrDiff};
pub use erc::{interpret_report, ErcError, ErcReport, ErcViolation, KicadToolchain, ToolchainError};
pub use netlist::{Component, IrError, Net, NetlistIr};
pub use reader::{read_netlist, read_netlist_file, ReadError, ReadFileError};
pub use schematic::{generate_schematic, generate_with_report, GenerationReport, Layout};
pub use symbols::{PinInfo, SymbolLibrary};
