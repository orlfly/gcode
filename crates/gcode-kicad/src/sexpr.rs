//! S-expression parser and writer for KiCad file formats.
//!
//! KiCad files (`.kicad_sch`, `.kicad_pcb`) are text s-expressions:
//!
//! ```text
//! (kicad_sch (version 20250114) (generator "eeschema")
//!   (symbol (lib_id "Device:R") (at 100 50 0) ...))
//! ```

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Sexpr {
    /// Bare atom: `123`, `3.5`, `true`, `abcd`
    Atom(String),
    /// Quoted string: `"hello world"`
    Str(String),
    /// List: `(name child1 child2)`
    List(Vec<Sexpr>),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected end of input at byte {0}")]
    Eof(usize),
    #[error("unexpected character {ch:?} at byte {pos}")]
    Unexpected { ch: char, pos: usize },
    #[error("unterminated string starting at byte {0}")]
    UnterminatedString(usize),
    #[error("unbalanced close paren at byte {0}")]
    UnbalancedClose(usize),
}

impl Sexpr {
    /// Parse a full document containing one or more top-level s-expressions.
    pub fn parse(input: &str) -> Result<Vec<Sexpr>, ParseError> {
        let bytes = input.as_bytes();
        let mut pos = 0usize;
        let mut out = Vec::new();
        while pos < bytes.len() {
            skip_ws(bytes, &mut pos);
            if pos >= bytes.len() {
                break;
            }
            out.push(Self::parse_one(input, &mut pos)?);
        }
        Ok(out)
    }

    /// Parse a single s-expression starting at `pos`.
    pub fn parse_one(input: &str, pos: &mut usize) -> Result<Sexpr, ParseError> {
        let bytes = input.as_bytes();
        skip_ws(bytes, pos);
        if *pos >= bytes.len() {
            return Err(ParseError::Eof(*pos));
        }
        match bytes[*pos] {
            b'(' => {
                *pos += 1;
                let mut items = Vec::new();
                loop {
                    skip_ws(bytes, pos);
                    if *pos >= bytes.len() {
                        return Err(ParseError::Eof(*pos));
                    }
                    match bytes[*pos] {
                        b')' => {
                            *pos += 1;
                            return Ok(Sexpr::List(items));
                        }
                        _ => items.push(Self::parse_one(input, pos)?),
                    }
                }
            }
            b')' => Err(ParseError::UnbalancedClose(*pos)),
            b'"' => {
                *pos += 1;
                let start = *pos;
                let mut s = String::new();
                while *pos < bytes.len() {
                    match bytes[*pos] {
                        b'"' => {
                            // caller handles trailing; take raw slice for escapes
                            let raw = &input[start..*pos];
                            s.push_str(&unescape(raw));
                            *pos += 1;
                            return Ok(Sexpr::Str(s));
                        }
                        b'\\' => {
                            *pos += 2; // skip escape; raw slice handles the pair
                        }
                        _ => *pos += 1,
                    }
                }
                Err(ParseError::UnterminatedString(start))
            }
            _ => {
                let start = *pos;
                while *pos < bytes.len()
                    && !bytes[*pos].is_ascii_whitespace()
                    && bytes[*pos] != b'('
                    && bytes[*pos] != b')'
                    && bytes[*pos] != b'"'
                {
                    *pos += 1;
                }
                if start == *pos {
                    return Err(ParseError::Unexpected {
                        ch: input[*pos..].chars().next().unwrap_or('?'),
                        pos: *pos,
                    });
                }
                Ok(Sexpr::Atom(input[start..*pos].to_string()))
            }
        }
    }

    /// First list item if this is a list with items (the "tag").
    pub fn tag(&self) -> Option<&str> {
        match self {
            Sexpr::List(items) => items.first().and_then(|s| s.as_atom()),
            _ => None,
        }
    }

    /// Atom value if this is an atom.
    pub fn as_atom(&self) -> Option<&str> {
        match self {
            Sexpr::Atom(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Sexpr::Str(s) => Some(s),
            Sexpr::Atom(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Sexpr]> {
        match self {
            Sexpr::List(l) => Some(l),
            _ => None,
        }
    }

    /// Find first child list with the given tag: `(sym (lib_id "x") ...)` → find "lib_id".
    pub fn find(&self, tag: &str) -> Option<&Sexpr> {
        self.as_list()?
            .iter()
            .find(|c| c.tag() == Some(tag))
    }

    /// All child lists with the given tag.
    pub fn find_all(&self, tag: &str) -> Vec<&Sexpr> {
        self.as_list()
            .map(|l| l.iter().filter(|c| c.tag() == Some(tag)).collect())
            .unwrap_or_default()
    }

    /// Value of `(tag value)` — the first argument after the tag.
    pub fn get(&self, tag: &str) -> Option<&Sexpr> {
        self.find(tag).and_then(|l| l.as_list()).and_then(|a| a.get(1))
    }

    /// Write this s-expression in KiCad style.
    pub fn write(&self, out: &mut String, indent: usize) {
        match self {
            Sexpr::Atom(a) => out.push_str(a),
            Sexpr::Str(s) => {
                out.push('"');
                out.push_str(&escape(s));
                out.push('"');
            }
            Sexpr::List(items) => {
                let pad = "  ".repeat(indent);
                let child_pad = "  ".repeat(indent + 1);
                out.push('(');
                // tag on same line
                if let Some(first) = items.first() {
                    first.write(out, 0);
                }
                // short lists stay on one line
                let short = items.len() <= 2
                    || items.iter().all(|i| !matches!(i, Sexpr::List(l) if l.len() > 3));
                if short {
                    for i in &items[1.min(items.len())..] {
                        out.push(' ');
                        i.write(out, 0);
                    }
                } else {
                    for i in &items[1..] {
                        let _ = write!(out, "\n{child_pad}");
                        i.write(out, indent + 1);
                    }
                    let _ = write!(out, "\n{pad}");
                }
                out.push(')');
            }
        }
    }

    pub fn to_string_kicad(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0);
        s
    }
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_atom_and_list() {
        let sexprs = Sexpr::parse("(a 1 2.5 (b \"hello world\") true)").unwrap();
        assert_eq!(sexprs.len(), 1);
        let l = &sexprs[0];
        assert_eq!(l.tag(), Some("a"));
        assert_eq!(l.get("b").and_then(|s| s.as_str()), Some("hello world"));
    }

    #[test]
    fn parse_nested_kicad_style() {
        let doc = r#"(kicad_sch (version 20250114) (generator "eeschema")
  (symbol (lib_id "Device:R") (at 100 50 0) (property "Reference" "R1")))"#;
        let sexprs = Sexpr::parse(doc).unwrap();
        let root = &sexprs[0];
        assert_eq!(root.tag(), Some("kicad_sch"));
        let syms = root.find_all("symbol");
        assert_eq!(syms.len(), 1);
        assert_eq!(
            syms[0].get("lib_id").and_then(|s| s.as_str()),
            Some("Device:R")
        );
        assert_eq!(
            syms[0]
                .find("property")
                .and_then(|p| p.as_list())
                .and_then(|a| a.get(2))
                .and_then(|s| s.as_str()),
            Some("R1")
        );
    }

    #[test]
    fn roundtrip() {
        let doc = r#"(a (b "x\"y") (c 1 2) (d (e 3)))"#;
        let parsed = Sexpr::parse(doc).unwrap();
        let written = parsed[0].to_string_kicad();
        let reparsed = Sexpr::parse(&written).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn escaped_quotes_in_string() {
        let doc = r#"(property "Reference" "R1" (at 0 0 0))"#;
        let l = &Sexpr::parse(doc).unwrap()[0];
        assert_eq!(l.as_list().unwrap()[2].as_str(), Some("R1"));
    }

    #[test]
    fn multiple_top_level() {
        let sexprs = Sexpr::parse("(a 1)\n(b 2)").unwrap();
        assert_eq!(sexprs.len(), 2);
    }

    #[test]
    fn errors() {
        assert!(Sexpr::parse("(a").is_err());
        assert!(Sexpr::parse("a)").is_err());
        assert!(Sexpr::parse("(a \"unterminated)").is_err());
    }
}
