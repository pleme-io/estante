//! Inline-metadata parser for ad-hoc tatara-lisp scripts.
//!
//! Mirrors uv's [PEP 723](https://peps.python.org/pep-0723/) shape —
//! a script can carry its own dependency declaration in a comment
//! block at the top, and `estante run <script>` resolves + materializes
//! them ad-hoc before executing.
//!
//! ## Canonical metadata block
//!
//! ```lisp
//! ;;; --- estante
//! ;;; dependencies:
//! ;;;   - github:MichaelAquilina/zsh-you-should-use@v1.7.4
//! ;;;   - github:org/other-pkg
//! ;;;   - local:./fixture
//! ;;; provides: my-tool
//! ;;; ---
//!
//! (defload :pkg "zsh-you-should-use")
//! (defalias :name "ysu" :value "you should use")
//! ```
//!
//! Parser is intentionally tiny: scan from the top of the file until
//! the first `;;; ---` open marker, then accumulate `;;; …`-prefixed
//! lines until the next `;;; ---` close marker. The interior is hand-
//! parsed (a single-line YAML-ish dialect — no nested mappings beyond
//! the `dependencies:` list and a few top-level scalars). Choosing a
//! tiny hand parser over serde_yaml avoids a giant transitive dep at
//! a v0.1 stage; we can swap to serde_yaml when the metadata surface
//! grows.

use std::fmt;

/// Parsed inline metadata extracted from a tatara-lisp script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineMetadata {
    /// List of `source:`-style dependency specs. Each one is a string
    /// suitable for [`estante_types::Source::parse`].
    pub dependencies: Vec<String>,
    /// Optional installed-binary name. Used by `estante tool install
    /// <script>` to pick the binary name; defaults to the script's
    /// file stem when absent.
    pub provides: Option<String>,
    /// Optional `requires-frost: ">=0.1.0"` semver string. Not yet
    /// enforced — recorded for the future when frost ships versioned
    /// compatibility checks.
    pub requires_frost: Option<String>,
}

impl InlineMetadata {
    /// True if the script declared no metadata block at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty() && self.provides.is_none() && self.requires_frost.is_none()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InlineMetadataError {
    #[error("missing closing `;;; ---` marker for inline metadata block opened at line {0}")]
    UnterminatedBlock(usize),
    #[error("inline metadata: unrecognized key `{key}` at line {line}")]
    UnknownKey { key: String, line: usize },
    #[error("inline metadata: malformed list entry at line {line}: `{raw}`")]
    MalformedListEntry { line: usize, raw: String },
}

/// Comment style for the inline-metadata block. Tatara-lisp uses
/// `;;;`; POSIX shells use `#`. Both share the same body grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentStyle {
    Lisp,  // ;;; --- estante / ;;; ---
    Shell, // # === estante / # ===
}

impl CommentStyle {
    fn prefix(self) -> &'static str {
        match self {
            Self::Lisp => ";;;",
            Self::Shell => "#",
        }
    }
    fn open_marker(self) -> &'static str {
        match self {
            Self::Lisp => ";;; --- estante",
            Self::Shell => "# === estante",
        }
    }
    fn close_marker(self) -> &'static str {
        match self {
            Self::Lisp => ";;; ---",
            Self::Shell => "# ===",
        }
    }

    /// Pick the style appropriate to a script's filename / extension.
    /// Defaults to Lisp for unknown / extensionless paths since
    /// tatara-lisp is estante's primary surface.
    pub fn from_path(path: &std::path::Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "sh" | "bash" | "zsh" | "fish" => Self::Shell,
            _ => Self::Lisp,
        }
    }
}

/// Parse inline metadata using whichever comment style matches the
/// script. The function tries both — whichever opens a marker first
/// wins. Returns the parsed block (or an empty struct if no block is
/// present), the line index immediately after the block, and the
/// detected style.
pub fn parse(src: &str) -> Result<(InlineMetadata, usize), InlineMetadataError> {
    let (md, line, _) = parse_with_style(src)?;
    Ok((md, line))
}

pub fn parse_with_style(
    src: &str,
) -> Result<(InlineMetadata, usize, CommentStyle), InlineMetadataError> {
    // Try Lisp style first, then Shell. The two markers differ enough
    // that mis-detection is impossible.
    if let Some(result) = parse_for_style(src, CommentStyle::Lisp)? {
        return Ok((result.0, result.1, CommentStyle::Lisp));
    }
    if let Some(result) = parse_for_style(src, CommentStyle::Shell)? {
        return Ok((result.0, result.1, CommentStyle::Shell));
    }
    Ok((InlineMetadata::default(), 0, CommentStyle::Lisp))
}

fn parse_for_style(
    src: &str,
    style: CommentStyle,
) -> Result<Option<(InlineMetadata, usize)>, InlineMetadataError> {
    let mut metadata = InlineMetadata::default();
    let mut in_block = false;
    let mut current_key: Option<String> = None;
    let mut open_line = 0;

    for (idx, raw_line) in src.lines().enumerate() {
        let line = raw_line.trim_end();
        let line_number = idx + 1;

        if !in_block {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            // Shebang line — ignore so vanilla-shell scripts work.
            if line_number == 1 && trimmed.starts_with("#!") && style == CommentStyle::Shell {
                continue;
            }
            if !trimmed.starts_with(style.prefix()) {
                // No metadata block at the top for this style.
                return Ok(None);
            }
            if is_open(style, trimmed) {
                in_block = true;
                open_line = line_number;
                continue;
            }
            // A regular comment before the marker — no metadata block.
            return Ok(None);
        }

        let trimmed = line.trim_start();
        if is_close(style, trimmed) {
            return Ok(Some((metadata, line_number)));
        }
        if !trimmed.starts_with(style.prefix()) {
            return Err(InlineMetadataError::UnterminatedBlock(open_line));
        }
        let payload = trimmed.trim_start_matches(style.prefix()).trim_start();
        if payload.is_empty() {
            continue;
        }

        // Two shapes inside the block:
        //   - `key: value` — top-level scalar
        //   - `  - item`   — list item under current_key
        if let Some(rest) = payload.strip_prefix("- ") {
            let key = current_key.as_deref().ok_or_else(|| {
                InlineMetadataError::MalformedListEntry {
                    line: line_number,
                    raw: payload.to_owned(),
                }
            })?;
            if key == "dependencies" {
                metadata.dependencies.push(rest.trim().to_owned());
            } else {
                return Err(InlineMetadataError::UnknownKey {
                    key: key.to_owned(),
                    line: line_number,
                });
            }
            continue;
        }

        // `key: value` or `key:` (opens a list)
        let (key, value) = match payload.split_once(':') {
            Some((k, v)) => (k.trim().to_owned(), v.trim().to_owned()),
            None => {
                return Err(InlineMetadataError::MalformedListEntry {
                    line: line_number,
                    raw: payload.to_owned(),
                })
            }
        };

        match key.as_str() {
            "dependencies" => {
                current_key = Some("dependencies".to_owned());
                // Inline single-entry shape: `dependencies: [github:org/a, github:org/b]`
                if let Some(inner) = strip_list_brackets(&value) {
                    for entry in inner.split(',') {
                        let trimmed = entry.trim();
                        if !trimmed.is_empty() {
                            metadata.dependencies.push(trimmed.to_owned());
                        }
                    }
                    current_key = None;
                }
            }
            "provides" => {
                metadata.provides = Some(value);
                current_key = None;
            }
            "requires-frost" | "requires_frost" => {
                metadata.requires_frost = Some(value);
                current_key = None;
            }
            other => {
                return Err(InlineMetadataError::UnknownKey {
                    key: other.to_owned(),
                    line: line_number,
                });
            }
        }
    }

    if in_block {
        Err(InlineMetadataError::UnterminatedBlock(open_line))
    } else {
        Ok(None)
    }
}

fn is_open(style: CommentStyle, line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.to_ascii_lowercase().contains(&style.open_marker().to_ascii_lowercase())
}

fn is_close(style: CommentStyle, line: &str) -> bool {
    line.trim_end() == style.close_marker()
}

fn strip_list_brackets(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner)
}

impl fmt::Display for InlineMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "InlineMetadata {{")?;
        if !self.dependencies.is_empty() {
            writeln!(f, "  dependencies:")?;
            for d in &self.dependencies {
                writeln!(f, "    - {d}")?;
            }
        }
        if let Some(p) = &self.provides {
            writeln!(f, "  provides: {p}")?;
        }
        if let Some(r) = &self.requires_frost {
            writeln!(f, "  requires-frost: {r}")?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_metadata_block_returns_empty() {
        let src = "(defalias :name \"hi\" :value \"echo hi\")\n";
        let (md, line) = parse(src).unwrap();
        assert!(md.is_empty());
        assert_eq!(line, 0);
    }

    #[test]
    fn parses_canonical_block() {
        let src = r#";;; --- estante
;;; dependencies:
;;;   - github:org/foo@v1.0.0
;;;   - local:./bar
;;; provides: my-tool
;;; ---

(defalias :name "tool" :value "echo")
"#;
        let (md, line) = parse(src).unwrap();
        assert_eq!(
            md.dependencies,
            vec!["github:org/foo@v1.0.0", "local:./bar"]
        );
        assert_eq!(md.provides.as_deref(), Some("my-tool"));
        assert!(line > 0);
    }

    #[test]
    fn parses_inline_list_shape() {
        let src = r#";;; --- estante
;;; dependencies: [github:o/a, github:o/b]
;;; ---
"#;
        let (md, _) = parse(src).unwrap();
        assert_eq!(md.dependencies, vec!["github:o/a", "github:o/b"]);
    }

    #[test]
    fn block_without_close_is_an_error() {
        let src = r#";;; --- estante
;;; dependencies:
;;;   - github:org/foo
"#;
        let err = parse(src).unwrap_err();
        assert!(matches!(err, InlineMetadataError::UnterminatedBlock(1)));
    }

    #[test]
    fn unknown_key_errors() {
        let src = r#";;; --- estante
;;; bogus: value
;;; ---
"#;
        let err = parse(src).unwrap_err();
        assert!(matches!(err, InlineMetadataError::UnknownKey { .. }));
    }

    #[test]
    fn list_entry_without_key_errors() {
        let src = r#";;; --- estante
;;;   - github:org/foo
;;; ---
"#;
        let err = parse(src).unwrap_err();
        assert!(matches!(err, InlineMetadataError::MalformedListEntry { .. }));
    }

    #[test]
    fn requires_frost_round_trips() {
        let src = r#";;; --- estante
;;; requires-frost: ">=0.1.0"
;;; ---
"#;
        let (md, _) = parse(src).unwrap();
        assert_eq!(md.requires_frost.as_deref(), Some("\">=0.1.0\""));
    }

    #[test]
    fn comment_before_marker_means_no_metadata() {
        // A regular `;;;` comment before the open marker is just a
        // comment — we don't treat the script as having metadata.
        let src = r#";;; not a metadata block
(defalias :name "hi" :value "echo")
"#;
        let (md, _) = parse(src).unwrap();
        assert!(md.is_empty());
    }

    #[test]
    fn blank_lines_in_block_are_allowed() {
        let src = r#";;; --- estante
;;;
;;; dependencies:
;;;   - github:org/foo
;;;
;;; ---
"#;
        let (md, _) = parse(src).unwrap();
        assert_eq!(md.dependencies, vec!["github:org/foo"]);
    }
}
