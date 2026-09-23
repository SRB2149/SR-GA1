//! Enforcement of the supported SystemVerilog subset.
//!
//! A plausible-looking wrong bitstream is far worse than a refusal, so this
//! runs in two layers:
//!
//! * a **source scan**, which catches constructs Yosys would happily accept
//!   and quietly ignore — `initial`, delays, assertions, system tasks — and
//!   can name the exact line they appear on; and
//! * a **netlist check** on the elaborated design, which is the
//!   authoritative one: it sees what the construct actually became, so an
//!   inferred latch or an asynchronous reset is caught however it was
//!   written. Yosys' `src` attributes carry the location back.
//!
//! Anything neither layer rejects is left to Yosys. If Yosys cannot map it,
//! Yosys says so; what must never happen is silently mapping something the
//! fabric cannot represent.

use crate::netlist::{src_of, Module, SrcLoc};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub loc: Option<SrcLoc>,
    /// The construct as the designer wrote it.
    pub construct: String,
    /// Why this fabric cannot have it.
    pub reason: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.loc {
            Some(loc) => write!(f, "{}: {}: {}", loc, self.construct, self.reason),
            None => write!(f, "{}: {}", self.construct, self.reason),
        }
    }
}

// ---------------------------------------------------------------------------
// Source scan

/// System functions that are compile-time constants, not simulation calls.
const ALLOWED_SYSTEM_FUNCTIONS: &[&str] = &[
    "$clog2", "$bits", "$size", "$left", "$right", "$low", "$high", "$signed", "$unsigned",
    "$increment", "$dimensions", "$unpacked_dimensions",
];

struct Token {
    line: usize,
    text: String,
}

/// Split a source file into tokens, dropping comments and string literals so
/// a keyword mentioned in a comment is never reported.
fn tokenize(src: &str) -> Vec<Token> {
    let bytes: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut line = 1usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                if bytes[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        // String literals.
        if c == '"' {
            i += 1;
            while i < bytes.len() && bytes[i] != '"' {
                if bytes[i] == '\\' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 1;
            tokens.push(Token { line, text: "\"\"".to_string() });
            continue;
        }
        // Identifiers, keywords, system tasks, compiler directives.
        if c.is_alphabetic() || c == '_' || c == '$' || c == '`' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_alphanumeric() || bytes[i] == '_' || bytes[i] == '$') {
                i += 1;
            }
            tokens.push(Token { line, text: bytes[start..i].iter().collect() });
            continue;
        }
        // Numbers, including sized literals like 4'b1010.
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_alphanumeric() || bytes[i] == '\'' || bytes[i] == '_')
            {
                i += 1;
            }
            tokens.push(Token { line, text: bytes[start..i].iter().collect() });
            continue;
        }
        tokens.push(Token { line, text: c.to_string() });
        i += 1;
    }
    tokens
}

/// Keywords rejected outright, with the hardware reason.
fn rejected_keyword(word: &str) -> Option<&'static str> {
    Some(match word {
        "initial" => {
            "initial blocks describe simulation-time behaviour; the configuration shift \
             registers have no defined power-on state in silicon"
        }
        "task" | "endtask" => "tasks are not part of the supported subset; use a combinational function",
        "assert" | "assume" | "cover" | "restrict" | "property" | "sequence" | "endproperty"
        | "endsequence" => "assertions have no hardware counterpart on this fabric",
        "interface" | "endinterface" | "modport" | "class" | "endclass" | "package"
        | "endpackage" | "program" | "endprogram" => {
            "the subset is flat modules only; interfaces, classes and packages are not elaborated"
        }
        "negedge" => {
            "every CLB flip-flop is rising-edge triggered on its column clock; there is no \
             edge select bit"
        }
        "inout" => "the fabric has no general tri-state; DDIO pins are declared in the constraints file",
        "tri" | "triand" | "trior" | "tri0" | "tri1" | "trireg" | "wand" | "wor" | "supply0"
        | "supply1" => "there are no tri-state or wired nets; every lane segment has exactly one driver",
        "always_latch" => {
            "the CLB has one edge-triggered flip-flop and no latch; level-sensitive storage \
             cannot be built"
        }
        "forever" | "fork" | "join" | "join_any" | "join_none" | "wait" => {
            "this construct has no synthesisable meaning in the supported subset"
        }
        "force" | "release" | "deassign" => "procedural continuous assignment is not synthesisable",
        "real" | "realtime" | "shortreal" => "the fabric carries single bits; there is no real type",
        _ => return None,
    })
}

const DECLARATION_KEYWORDS: &[&str] =
    &["logic", "wire", "reg", "bit", "integer", "byte", "shortint", "int", "longint"];

const DIRECTION_KEYWORDS: &[&str] = &["input", "output", "inout"];

/// Scan one source file. `display_path` is what appears in diagnostics.
pub fn scan_source(display_path: &Path, src: &str) -> Vec<Diagnostic> {
    let tokens = tokenize(src);
    let file = display_path.display().to_string();
    let mut out = Vec::new();
    let at = |line: usize| Some(SrcLoc { file: file.clone(), line });

    for (i, token) in tokens.iter().enumerate() {
        let word = token.text.as_str();

        if let Some(reason) = rejected_keyword(word) {
            out.push(Diagnostic {
                loc: at(token.line),
                construct: word.to_string(),
                reason: reason.to_string(),
            });
            continue;
        }

        // System tasks and functions, minus the constant-valued ones.
        if word.starts_with('$') && !ALLOWED_SYSTEM_FUNCTIONS.contains(&word) {
            out.push(Diagnostic {
                loc: at(token.line),
                construct: word.to_string(),
                reason: "system tasks and functions are simulation-only; the supported \
                         constant-valued ones are $clog2, $bits and friends"
                    .to_string(),
            });
            continue;
        }

        // Delays: `#` followed by a number. `#(` is a parameter override.
        if word == "#" {
            if let Some(next) = tokens.get(i + 1) {
                if next.text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    out.push(Diagnostic {
                        loc: at(token.line),
                        construct: format!("#{}", next.text),
                        reason: "delays are simulation-only; the fabric has no timing model"
                            .to_string(),
                    });
                }
            }
        }
    }

    out.extend(scan_unpacked_arrays(&tokens, &file));
    out
}

/// Flag a dimension written *after* the declared name — an unpacked array.
/// Packed dimensions, which come before the name, are supported.
fn scan_unpacked_arrays(tokens: &[Token], file: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        // Find the start of a declaration statement.
        let mut j = i;
        if DIRECTION_KEYWORDS.contains(&tokens[j].text.as_str()) {
            j += 1;
        }
        if j >= tokens.len() || !DECLARATION_KEYWORDS.contains(&tokens[j].text.as_str()) {
            i += 1;
            continue;
        }
        j += 1;
        while j < tokens.len() && matches!(tokens[j].text.as_str(), "signed" | "unsigned") {
            j += 1;
        }
        // Skip every packed dimension.
        while j < tokens.len() && tokens[j].text == "[" {
            let mut depth = 0usize;
            while j < tokens.len() {
                match tokens[j].text.as_str() {
                    "[" => depth += 1,
                    "]" => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        // The declared name, then anything but `[` is fine.
        if j + 1 < tokens.len()
            && tokens[j].text.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && tokens[j + 1].text == "["
        {
            out.push(Diagnostic {
                loc: Some(SrcLoc { file: file.to_string(), line: tokens[j].line }),
                construct: format!("{} [...]", tokens[j].text),
                reason: "unpacked arrays are not supported; the fabric has no memory and the \
                         flow maps single bits — use a packed vector"
                    .to_string(),
            });
        }
        i = j.max(i + 1);
    }
    out
}

// ---------------------------------------------------------------------------
// Netlist check

/// What an elaborated cell type means, when it means the design is outside
/// the subset. Matched by prefix against Yosys' internal cell names.
fn rejected_cell(ty: &str) -> Option<(&'static str, &'static str)> {
    let latch = (
        "inferred latch",
        "the CLB has one edge-triggered flip-flop and no latch; complete every branch of \
         the combinational block, or give the signal a default",
    );
    let async_reset = (
        "asynchronous reset",
        "reset is sampled on each column's clock; config bit 19 holds the reset value and \
         there is no asynchronous path",
    );
    let memory = (
        "memory or unpacked array",
        "the fabric has no memory: every net is a single routed bit",
    );
    let tristate = (
        "tri-state driver",
        "every lane segment has exactly one driving mux; declare DDIO pins in the \
         constraints file instead",
    );
    let arith = (
        "non-constant multiply, divide or modulo",
        "the only arithmetic primitive is the per-column carry chain, four cells long; \
         a general multiplier does not fit",
    );
    let set_reset = (
        "set/reset flip-flop",
        "the flip-flop has one synchronous reset to a constant, with no independent set",
    );

    let m = match ty {
        "$dlatch" | "$adlatch" | "$dlatchsr" | "$sr" => latch,
        "$adff" | "$adffe" | "$aldff" | "$aldffe" => async_reset,
        "$dffsr" | "$dffsre" => set_reset,
        "$mem" | "$mem_v2" | "$memrd" | "$memrd_v2" | "$memwr" | "$memwr_v2" | "$meminit"
        | "$meminit_v2" => memory,
        "$tribuf" | "$_TBUF_" => tristate,
        "$mul" | "$div" | "$mod" | "$divfloor" | "$modfloor" | "$pow" => arith,
        _ => {
            if ty.starts_with("$_DLATCH") || ty.starts_with("$_SR_") {
                latch
            } else if ty.starts_with("$_ALDFF") {
                async_reset
            } else if ty.starts_with("$_DFFSR") {
                set_reset
            } else if ty.starts_with("$_DFF_") && ty.len() > "$_DFF_N_".len() {
                // $_DFF_P_ is a plain flop; $_DFF_PP0_ has an async reset.
                async_reset
            } else if ty.starts_with("$_TBUF") {
                tristate
            } else {
                return None;
            }
        }
    };
    Some(m)
}

/// Check an elaborated module. Returns one diagnostic per offending cell or
/// port, located wherever Yosys recorded a source.
pub fn check_elaborated(module: &Module) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    for (name, port) in &module.ports {
        if port.direction == crate::netlist::Direction::Inout {
            out.push(Diagnostic {
                loc: None,
                construct: format!("inout port \"{}\"", name),
                reason: "the fabric has no general bidirectional pin; declare a DDIO pin in \
                         the constraints file"
                    .to_string(),
            });
        }
    }

    for (name, cell) in &module.cells {
        if let Some((construct, reason)) = rejected_cell(&cell.ty) {
            out.push(Diagnostic {
                loc: cell.src(),
                construct: format!("{} (cell {} of type {})", construct, display_name(name), cell.ty),
                reason: reason.to_string(),
            });
            continue;
        }
        // A falling-edge or dual-edge flop survives elaboration as a normal
        // $dff with CLK_POLARITY 0.
        if matches!(cell.ty.as_str(), "$dff" | "$dffe" | "$sdff" | "$sdffe" | "$sdffce")
            && cell.parameters.get("CLK_POLARITY").and_then(polarity) == Some(false)
        {
            out.push(Diagnostic {
                loc: cell.src(),
                construct: format!("negedge clock on register \"{}\"", display_name(name)),
                reason: "every CLB flip-flop is rising-edge triggered on its column clock"
                    .to_string(),
            });
        }
        if matches!(cell.ty.as_str(), "$sdff" | "$sdffe" | "$sdffce")
            && cell.parameters.get("SRST_POLARITY").and_then(polarity) == Some(false)
        {
            out.push(Diagnostic {
                loc: cell.src(),
                construct: format!("active-low reset on register \"{}\"", display_name(name)),
                reason: "the global reset is active high and shared by every CLB".to_string(),
            });
        }
        // Reset must win over enable, matching `if (reset) ... else if (en)`.
        if cell.ty == "$sdffce" || cell.ty.starts_with("$_SDFFCE") {
            out.push(Diagnostic {
                loc: cell.src(),
                construct: format!("enable takes priority over reset on \"{}\"", display_name(name)),
                reason: "the CLB samples `if (reset) ... else if (enable) ...`, so reset wins; \
                         rewrite the register with reset tested first"
                    .to_string(),
            });
        }
    }

    out
}

fn polarity(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::String(s) => s.chars().last().map(|c| c == '1'),
        serde_json::Value::Number(n) => n.as_u64().map(|v| v != 0),
        _ => None,
    }
}

/// Yosys autogenerated names are noise in a diagnostic; keep them short.
fn display_name(name: &str) -> String {
    let trimmed = name.trim_start_matches('\\');
    if let Some(rest) = trimmed.strip_prefix("$auto$") {
        return format!("${}", rest.split('$').next_back().unwrap_or(rest));
    }
    trimmed.to_string()
}

/// Warn about DDIO nets referenced by a design without a matching constraint.
pub fn check_ddio_references(
    module: &Module,
    ddio_nets: &[String],
    constrained: &[String],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for net in ddio_nets {
        if constrained.iter().any(|c| c == net) {
            continue;
        }
        let referenced = module.ports.keys().any(|p| p == net)
            || module.netnames.keys().any(|n| n.trim_start_matches('\\') == net);
        if referenced {
            out.push(Diagnostic {
                loc: module.netnames.get(net).and_then(|n| src_of(&n.attributes)),
                construct: format!("DDIO net \"{}\"", net),
                reason: "DDIO pins are never inferred; declare the input, output and \
                         direction nets in the constraints file or the pin stays gated off"
                    .to_string(),
            });
        }
    }
    out
}
