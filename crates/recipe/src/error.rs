//! [`RecipeError`]: what went wrong, and where in the text.

use std::fmt;
use std::ops::Range;

use mold_graph::GraphError;

/// Why a recipe was refused. Always points at a place in the text.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeError {
    /// 1-based line of the offending key or value.
    pub line: usize,
    /// 1-based column, in characters.
    pub column: usize,
    /// Byte range in the recipe text (empty for "missing" errors at the table that
    /// should have had the key).
    pub span: Range<usize>,
    pub kind: ErrorKind,
    /// One human sentence, without the position (see `Display` for both).
    pub message: String,
}

/// The class of a [`RecipeError`], for callers that react to it (tests, the editor).
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    /// Not valid TOML.
    Syntax,
    /// `[recipe] format` is not one this build reads.
    UnsupportedFormat,
    /// A key the format does not define: a typo, or a field of a newer Mold.
    UnknownKey,
    /// A required key is absent (`[recipe]`, `format`, a node's `kind`, ...).
    MissingKey,
    /// A value of the wrong shape (a table where a parameter value goes, a
    /// connection that is not `"node.port"`, `executor = 3`, ...).
    BadValue,
    /// The graph is ill-formed: a dangling connection or a cycle (always checked),
    /// or a kind/param/port/type error against a `Registry` (when one is given).
    /// Boxed to keep `Result<_, RecipeError>` small.
    Graph(Box<GraphError>),
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for RecipeError {}

impl RecipeError {
    pub(crate) fn at(
        text: &str,
        span: Range<usize>,
        kind: ErrorKind,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = line_col(text, span.start);
        Self {
            line,
            column,
            span,
            kind,
            message: message.into(),
        }
    }
}

/// 1-based (line, column) of byte `offset`; the column counts characters.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_lines_and_chars() {
        let t = "ab\ncdé\nf";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 3), (2, 1));
        assert_eq!(line_col(t, t.find('f').unwrap()), (3, 1));
        // `é` is two bytes, one column.
        assert_eq!(line_col(t, t.find('é').unwrap() + 2), (2, 4));
    }
}
