//! Source positions and diagnostics. Every error the engine reports carries `file:line:col`.

use std::fmt;

/// A byte range in one source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub fn new(file: u32, lo: usize, hi: usize) -> Span {
        Span { file, lo: lo as u32, hi: hi as u32 }
    }
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }
}

/// One source file as loaded.
#[derive(Clone, Debug)]
pub struct Source {
    pub path: String,
    pub text: String,
    line_starts: Vec<usize>,
}

impl Source {
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Source {
        let text = text.into();
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Source { path: path.into(), text, line_starts }
    }

    /// 1-based line and column (in characters) of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line];
        let col = self.text[start..offset.min(self.text.len())].chars().count() + 1;
        (line + 1, col)
    }
}

/// All loaded sources, indexed by `Span::file`.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    pub files: Vec<Source>,
}

impl SourceMap {
    pub fn add(&mut self, path: impl Into<String>, text: impl Into<String>) -> u32 {
        self.files.push(Source::new(path, text));
        (self.files.len() - 1) as u32
    }

    pub fn locate(&self, span: Span) -> String {
        match self.files.get(span.file as usize) {
            Some(src) => {
                let (l, c) = src.line_col(span.lo as usize);
                format!("{}:{}:{}", src.path, l, c)
            }
            None => "<unknown>".into(),
        }
    }

    pub fn snippet(&self, span: Span) -> &str {
        match self.files.get(span.file as usize) {
            Some(src) => src.text.get(span.lo as usize..span.hi as usize).unwrap_or(""),
            None => "",
        }
    }
}

/// An error or warning tied to a place in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Diag {
    pub span: Option<Span>,
    pub msg: String,
    pub warning: bool,
}

impl Diag {
    pub fn error(span: Span, msg: impl Into<String>) -> Diag {
        Diag { span: Some(span), msg: msg.into(), warning: false }
    }
    pub fn bare(msg: impl Into<String>) -> Diag {
        Diag { span: None, msg: msg.into(), warning: false }
    }
    pub fn warn(span: Span, msg: impl Into<String>) -> Diag {
        Diag { span: Some(span), msg: msg.into(), warning: true }
    }
    pub fn render(&self, sm: &SourceMap) -> String {
        let kind = if self.warning { "warning" } else { "error" };
        match self.span {
            Some(s) => format!("{}: {kind}: {}", sm.locate(s), self.msg),
            None => format!("{kind}: {}", self.msg),
        }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}
