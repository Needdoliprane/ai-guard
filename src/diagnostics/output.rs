//! Output safety, built string by string.
//!
//! The tool holds no secret literal, because it reads none: protection of
//! token and credential values rests on never opening them. What this module
//! guarantees is narrower and still required: an operator-chosen string (an
//! identifier, a path, a URL, a label) cannot forge a line, and cannot make
//! the pattern filter of `crate::redaction` rewrite the report.
//!
//! Every dynamic string goes through four steps: escape control characters,
//! mask, clip outside a mask, then require a fixpoint of the filter. The final
//! guard on the whole document is therefore an assertion, not a mechanism.

use super::DiagnoseError;
use super::report::IdDisplay;
use crate::redaction::Redactor;
use std::{cell::Cell, collections::BTreeMap, path::Path};

/// Bound of one dynamic string, in bytes.
pub const MAX_DYNAMIC_BYTES: usize = 512;
/// Bound of one path display, in bytes.
pub const MAX_PATH_BYTES: usize = 1024;

pub const MASKED_VALUE: &str = "[masked value]";
pub const MASKED_PATH: &str = "[masked path]";
pub const UNPARSEABLE_URL: &str = "[unparseable URL]";
const TRUNCATED: &str = "…[truncated]";
const MASK_OPEN: &str = "[REDACTED";
/// The private-key pattern of the filter spans lines and strings. A string
/// that carries its fixed marker is replaced whole, so that two strings of one
/// text report can never form a match together.
const PRIVATE_KEY_MARKER: &str = "PRIVATE KEY-----";

/// Neutralise characters that could forge or hide a line, or that a reader
/// does not see: a backslash is doubled; control characters, bidirectional
/// controls, characters that are invisible by default and every space other
/// than U+0020 become `\u{…}`. Two values that differ only by such a
/// character are then shown differently.
pub fn escape_controls(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            character if needs_escape(character) => {
                escaped.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

fn needs_escape(character: char) -> bool {
    character.is_control()
        // Line and paragraph separators, and the spaces that look like U+0020
        // or like nothing.
        || (character.is_whitespace() && character != ' ')
        // The code points that Unicode makes invisible by default
        // (`Default_Ignorable_Code_Point`), by explicit ranges: the standard
        // library has no such predicate. They hold every bidirectional
        // control (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to
        // U+2069), the zero-width characters, the soft hyphen, the word
        // joiner and the invisible operators, the fillers, the variation
        // selectors, the byte-order mark and the tag characters.
        || matches!(
            character,
            '\u{00ad}'
                | '\u{034f}'
                | '\u{061c}'
                | '\u{115f}'..='\u{1160}'
                | '\u{17b4}'..='\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff0}'..='\u{fff8}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e0fff}'
        )
}

/// Write a dynamic string for the text report: between double quotes, with a
/// double quote written `\"`. The input is already escaped by `Safe`.
pub fn quoted(display: &str) -> String {
    let mut text = String::with_capacity(display.len() + 2);
    text.push('"');
    for character in display.chars() {
        if character == '"' {
            text.push('\\');
        }
        text.push(character);
    }
    text.push('"');
    text
}

/// Which family an identifier belongs to; placeholders are numbered per kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdKind {
    Tenant,
    ProviderProfile,
}

impl IdKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::ProviderProfile => "profile",
        }
    }
}

/// An inspected directory and the placeholder that stands for it.
#[derive(Clone, Debug)]
pub struct Root {
    pub placeholder: &'static str,
    pub path: std::path::PathBuf,
}

/// The output filter with no secret literal, and the count of the values it
/// replaced by a placeholder during one run.
pub struct Safe {
    redactor: Redactor,
    masked: Cell<u64>,
}

impl Default for Safe {
    fn default() -> Self {
        Self::new()
    }
}

impl Safe {
    pub fn new() -> Self {
        Self {
            redactor: Redactor::new(Vec::new()),
            masked: Cell::new(0),
        }
    }

    /// Number of values replaced by a placeholder so far.
    pub fn masked_values(&self) -> u64 {
        self.masked.get()
    }

    fn count_mask(&self) {
        self.masked.set(self.masked.get().saturating_add(1));
    }

    fn is_fixpoint(&self, text: &str) -> bool {
        !text.contains(PRIVATE_KEY_MARKER) && self.redactor.text(text) == text
    }

    /// A free-form operator string: a label, a model name, a revision.
    pub fn dynamic(&self, raw: &str, max_bytes: usize) -> String {
        let escaped = escape_controls(raw);
        let masked = self.redactor.text(&escaped);
        if masked != escaped {
            self.count_mask();
        }
        let clipped = clip_outside_mask(&masked, max_bytes);
        if self.is_fixpoint(&clipped) {
            clipped
        } else {
            self.count_mask();
            MASKED_VALUE.to_owned()
        }
    }

    /// Prepare the displays of every identifier of one kind. A masked
    /// identifier gets a numbered placeholder, so that distinct tenants never
    /// collapse into one mask.
    pub fn ids<'a>(&self, kind: IdKind, raw: impl IntoIterator<Item = &'a str>) -> IdTable {
        // Sorted and deduplicated by the original identifier, so the number
        // of a placeholder is stable between runs.
        let mut displays: BTreeMap<String, Option<String>> = raw
            .into_iter()
            .map(|id| {
                let escaped = escape_controls(id);
                let clipped = clip_outside_mask(&escaped, MAX_DYNAMIC_BYTES);
                let shown =
                    (self.is_fixpoint(&escaped) && self.is_fixpoint(&clipped)).then_some(clipped);
                (id.to_owned(), shown)
            })
            .collect();
        let mut rank = 0_u64;
        IdTable {
            kind,
            displays: displays
                .iter_mut()
                .map(|(id, shown)| {
                    let display = match shown.take() {
                        Some(id) => IdDisplay { id, masked: false },
                        None => {
                            rank += 1;
                            self.count_mask();
                            IdDisplay {
                                id: format!("[masked {} identifier {rank}]", kind.noun()),
                                masked: true,
                            }
                        }
                    };
                    (id.clone(), display)
                })
                .collect(),
        }
    }

    /// A path. The absolute path is masked first; a display is derived only
    /// when masking left it unchanged. Without `show_paths`, a path below an
    /// inspected root is shown relative to the root placeholder, and any other
    /// path is replaced by `opaque`, which names a role and never a file name.
    pub fn path(&self, raw: &Path, roots: &[Root], show_paths: bool, opaque: &str) -> String {
        let absolute = escape_controls(&raw.to_string_lossy());
        if !self.is_fixpoint(&absolute) {
            self.count_mask();
            return MASKED_PATH.to_owned();
        }
        let display = if show_paths {
            absolute
        } else {
            match roots.iter().find_map(|root| {
                raw.strip_prefix(&root.path)
                    .ok()
                    .map(|relative| (root.placeholder, relative))
            }) {
                Some((placeholder, relative)) if relative.as_os_str().is_empty() => {
                    placeholder.to_owned()
                }
                Some((placeholder, relative)) => format!(
                    "{placeholder}/{}",
                    escape_controls(&relative.to_string_lossy())
                ),
                None => opaque.to_owned(),
            }
        };
        let clipped = clip_outside_mask(&display, MAX_PATH_BYTES);
        if self.is_fixpoint(&clipped) {
            clipped
        } else {
            self.count_mask();
            MASKED_PATH.to_owned()
        }
    }

    /// A URL, rebuilt from its parsed value: scheme, host, port and path.
    /// User information, query and fragment are never printed.
    pub fn url(&self, raw: &str) -> String {
        let Ok(url) = reqwest::Url::parse(raw) else {
            return UNPARSEABLE_URL.to_owned();
        };
        let Some(host) = url.host_str() else {
            return UNPARSEABLE_URL.to_owned();
        };
        let mut rebuilt = format!("{}://{host}", url.scheme());
        if let Some(port) = url.port() {
            rebuilt.push_str(&format!(":{port}"));
        }
        // The parser gives the path "/" to a URL written without one; an
        // origin is shown as it is written.
        if url.path() != "/" || writes_a_path(raw) {
            rebuilt.push_str(url.path());
        }
        self.dynamic(&rebuilt, MAX_DYNAMIC_BYTES)
    }

    /// The JSON document must be a fixpoint of the structured filter.
    pub fn guard_json(&self, value: &serde_json::Value) -> Result<(), DiagnoseError> {
        if self.redactor.value_unclipped(value) == *value {
            Ok(())
        } else {
            Err(DiagnoseError::ProtectedOutput)
        }
    }

    /// The text document must be a fixpoint of the text filter.
    pub fn guard_text(&self, text: &str) -> Result<(), DiagnoseError> {
        if self.redactor.text(text) == text {
            Ok(())
        } else {
            Err(DiagnoseError::ProtectedOutput)
        }
    }
}

/// Whether the text of a URL has a path after its authority.
fn writes_a_path(raw: &str) -> bool {
    raw.split_once("://").is_some_and(|(_, rest)| {
        rest.find(['/', '?', '#'])
            .is_some_and(|index| rest[index..].starts_with('/'))
    })
}

/// Displays of the identifiers of one kind.
pub struct IdTable {
    kind: IdKind,
    displays: BTreeMap<String, IdDisplay>,
}

impl IdTable {
    /// The display prepared for an identifier. An identifier that was not
    /// given to `Safe::ids` is never printed.
    pub fn display(&self, raw: &str) -> IdDisplay {
        self.displays
            .get(raw)
            .cloned()
            .unwrap_or_else(|| IdDisplay {
                id: format!("[masked {} identifier]", self.kind.noun()),
                masked: true,
            })
    }
}

/// Cut at the last character boundary within the bound that is not inside a
/// mask, then mark the cut. Masking always precedes clipping.
fn clip_outside_mask(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    let mut end = max_bytes.saturating_sub(TRUNCATED.len());
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    for (open, _) in input.match_indices(MASK_OPEN) {
        let close = input[open..]
            .find(']')
            .map_or(input.len(), |offset| open + offset + 1);
        if open < end && end < close {
            end = open;
            break;
        }
    }
    format!("{}{TRUNCATED}", &input[..end])
}
