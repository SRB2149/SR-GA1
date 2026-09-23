//! Yosys JSON netlist reader.
//!
//! Two netlists matter to the flow: the *elaborated* one, checked against
//! the supported subset before anything else happens, and the *mapped* one,
//! which carries only fabric cells and flip-flops and is what the packer
//! consumes. Both are the same schema, so one reader serves both.
//!
//! Source locations are kept wherever Yosys provides them — every
//! diagnostic this tool prints about a design should name a file and line.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct NetlistError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for NetlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for NetlistError {}

/// One bit of a signal: a net, a constant, or an undriven/unknown value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bit {
    Net(u32),
    Zero,
    One,
    X,
    Z,
}

impl Bit {
    pub fn as_net(self) -> Option<u32> {
        match self {
            Bit::Net(n) => Some(n),
            _ => None,
        }
    }
    pub fn as_const(self) -> Option<bool> {
        match self {
            Bit::Zero => Some(false),
            Bit::One => Some(true),
            _ => None,
        }
    }
}

impl fmt::Display for Bit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bit::Net(n) => write!(f, "{}", n),
            Bit::Zero => write!(f, "0"),
            Bit::One => write!(f, "1"),
            Bit::X => write!(f, "x"),
            Bit::Z => write!(f, "z"),
        }
    }
}

impl<'de> Deserialize<'de> for Bit {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Bit, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        match &value {
            serde_json::Value::Number(n) => n
                .as_u64()
                .map(|n| Bit::Net(n as u32))
                .ok_or_else(|| serde::de::Error::custom(format!("bad net number {}", n))),
            serde_json::Value::String(s) => match s.as_str() {
                "0" => Ok(Bit::Zero),
                "1" => Ok(Bit::One),
                "x" => Ok(Bit::X),
                "z" => Ok(Bit::Z),
                other => Err(serde::de::Error::custom(format!("unknown bit value \"{}\"", other))),
            },
            other => Err(serde::de::Error::custom(format!("unexpected bit {}", other))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Input,
    Output,
    Inout,
}

pub type Attributes = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, Deserialize)]
pub struct Port {
    pub direction: Direction,
    pub bits: Vec<Bit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cell {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub parameters: Attributes,
    #[serde(default)]
    pub attributes: Attributes,
    #[serde(default)]
    pub port_directions: BTreeMap<String, Direction>,
    #[serde(default)]
    pub connections: BTreeMap<String, Vec<Bit>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetName {
    #[serde(default)]
    pub hide_name: u8,
    pub bits: Vec<Bit>,
    #[serde(default)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Module {
    #[serde(default)]
    pub attributes: Attributes,
    #[serde(default)]
    pub ports: BTreeMap<String, Port>,
    #[serde(default)]
    pub cells: BTreeMap<String, Cell>,
    #[serde(default)]
    pub netnames: BTreeMap<String, NetName>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Netlist {
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub modules: BTreeMap<String, Module>,
}

impl Netlist {
    pub fn load(path: &Path) -> Result<Netlist, NetlistError> {
        let text = std::fs::read_to_string(path).map_err(|e| NetlistError {
            path: path.to_path_buf(),
            message: format!("cannot read the Yosys netlist: {}", e),
        })?;
        Netlist::parse(&text, path)
    }

    pub fn parse(text: &str, path: &Path) -> Result<Netlist, NetlistError> {
        serde_json::from_str(text).map_err(|e| NetlistError {
            path: path.to_path_buf(),
            message: format!("Yosys wrote a netlist this build cannot read: {}", e),
        })
    }

    /// The design's top module. After `hierarchy -top` and `flatten` there
    /// should be exactly one; anything else means the script did not do what
    /// we asked, which is worth saying plainly.
    pub fn top<'a>(&'a self, name: &str, path: &Path) -> Result<&'a Module, NetlistError> {
        self.modules.get(name).ok_or_else(|| NetlistError {
            path: path.to_path_buf(),
            message: format!(
                "no module \"{}\" in the netlist; it holds {}",
                name,
                if self.modules.is_empty() {
                    "no modules at all".to_string()
                } else {
                    self.modules.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            ),
        })
    }
}

/// A source location as Yosys records it, e.g. `counter.sv:12.5-12.30`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrcLoc {
    pub file: String,
    pub line: usize,
}

impl fmt::Display for SrcLoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// Pull a location out of a `src` attribute. Yosys may list several,
/// separated by `|`; the first is the most specific.
pub fn src_of(attributes: &Attributes) -> Option<SrcLoc> {
    let raw = attributes.get("src")?.as_str()?;
    let first = raw.split('|').next()?;
    // "path/file.sv:12.5-12.30" — the path itself may contain colons on
    // Windows, so split at the last colon.
    let (file, span) = first.rsplit_once(':')?;
    let line = span.split(['.', '-']).next()?.parse().ok()?;
    Some(SrcLoc { file: file.to_string(), line })
}

impl Cell {
    pub fn src(&self) -> Option<SrcLoc> {
        src_of(&self.attributes)
    }
    /// Bits on a port, or an empty slice if the cell does not have it.
    pub fn port(&self, name: &str) -> &[Bit] {
        self.connections.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }
    /// Single-bit port value, for the fine cells the mapper emits.
    pub fn bit(&self, name: &str) -> Option<Bit> {
        match self.port(name) {
            [b] => Some(*b),
            _ => None,
        }
    }
}

impl NetName {
    pub fn src(&self) -> Option<SrcLoc> {
        src_of(&self.attributes)
    }
}
