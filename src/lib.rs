use pyo3::prelude::*;
use salsa::Setter as _;
use std::collections::HashMap;

// ─── Exception ───────────────────────────────────────────────────────────────

pyo3::create_exception!(_syster, ParseError, pyo3::exceptions::PyException);

// ─── ParseDiagnostic ─────────────────────────────────────────────────────────
// Wraps syster::parser::SyntaxError with byte offsets resolved to line/col.

#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct ParseDiagnostic {
    pub message: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub start_offset: u32,
    pub end_offset: u32,
}

#[pymethods]
impl ParseDiagnostic {
    fn __repr__(&self) -> String {
        format!(
            "ParseDiagnostic(message={:?}, start={}:{}, end={}:{})",
            self.message, self.start_line, self.start_col, self.end_line, self.end_col,
        )
    }
}

fn make_diagnostic(
    e: &syster::parser::SyntaxError,
    li: &syster::base::LineIndex,
) -> ParseDiagnostic {
    let start = li.line_col(e.range.start());
    let end = li.line_col(e.range.end());
    ParseDiagnostic {
        message: e.message.clone(),
        start_line: start.line,
        start_col: start.col,
        end_line: end.line,
        end_col: end.col,
        start_offset: u32::from(e.range.start()),
        end_offset: u32::from(e.range.end()),
    }
}

// ─── SyntaxFile ──────────────────────────────────────────────────────────────

#[pyclass]
pub struct SyntaxFile {
    inner: syster::syntax::SyntaxFile,
}

#[pymethods]
impl SyntaxFile {
    fn is_sysml(&self) -> bool {
        self.inner.is_sysml()
    }

    fn is_kerml(&self) -> bool {
        self.inner.is_kerml()
    }

    fn has_errors(&self) -> bool {
        self.inner.has_errors()
    }

    fn source_text(&self) -> String {
        self.inner.source_text()
    }

    fn extract_imports(&self) -> Vec<String> {
        self.inner.extract_imports()
    }

    /// Returns all parse diagnostics with resolved line/column positions.
    fn errors(&self) -> Vec<ParseDiagnostic> {
        let li = self.inner.line_index();
        self.inner
            .errors()
            .iter()
            .map(|e| make_diagnostic(e, &li))
            .collect()
    }

    /// Extract semantic symbols from this file. `file_id` is an arbitrary
    /// integer you assign to identify this file within a workspace.
    fn symbols(&self, file_id: u32) -> Vec<HirSymbol> {
        let fid = syster::base::FileId::new(file_id);
        syster::hir::file_symbols(fid, &self.inner)
            .into_iter()
            .map(HirSymbol::from)
            .collect()
    }

    fn __repr__(&self) -> String {
        let lang = if self.inner.is_sysml() { "SysML" } else { "KerML" };
        format!(
            "SyntaxFile(language={:?}, errors={})",
            lang,
            self.inner.errors().len(),
        )
    }
}

// ─── Free parse functions ─────────────────────────────────────────────────────

/// Parse a SysML v2 source string. Always returns a (possibly error-bearing)
/// `SyntaxFile`; the parser is error-recovering so partial results are
/// available even when `has_errors()` is True.
#[pyfunction]
fn parse_sysml(text: &str) -> SyntaxFile {
    SyntaxFile {
        inner: syster::syntax::SyntaxFile::sysml(text),
    }
}

/// Parse a KerML source string. Same partial-parse semantics as `parse_sysml`.
#[pyfunction]
fn parse_kerml(text: &str) -> SyntaxFile {
    SyntaxFile {
        inner: syster::syntax::SyntaxFile::kerml(text),
    }
}

// ─── HirSymbol ───────────────────────────────────────────────────────────────

#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct HirSymbol {
    pub name: String,
    pub qualified_name: String,
    pub element_id: String,
    /// String display name for the symbol kind (e.g. `"PartUsage"`, `"Package"`).
    pub kind: String,
    pub file_id: u32,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub doc: Option<String>,
    pub supertypes: Vec<String>,
    pub is_public: bool,
    pub is_abstract: bool,
    pub is_variation: bool,
    pub is_readonly: bool,
    pub is_derived: bool,
}

impl From<syster::hir::HirSymbol> for HirSymbol {
    fn from(s: syster::hir::HirSymbol) -> Self {
        HirSymbol {
            name: s.name.to_string(),
            qualified_name: s.qualified_name.to_string(),
            element_id: s.element_id.to_string(),
            kind: s.kind.display().to_string(),
            file_id: s.file.index(),
            start_line: s.start_line,
            start_col: s.start_col,
            end_line: s.end_line,
            end_col: s.end_col,
            doc: s.doc.as_deref().map(str::to_owned),
            supertypes: s.supertypes.iter().map(|t| t.to_string()).collect(),
            is_public: s.is_public,
            is_abstract: s.is_abstract,
            is_variation: s.is_variation,
            is_readonly: s.is_readonly,
            is_derived: s.is_derived,
        }
    }
}

#[pymethods]
impl HirSymbol {
    fn __repr__(&self) -> String {
        format!(
            "HirSymbol(name={:?}, kind={:?}, file_id={})",
            self.name, self.kind, self.file_id,
        )
    }
}

// ─── Database ────────────────────────────────────────────────────────────────
// Wraps syster::hir::RootDatabase (Salsa incremental engine).
// Files are tracked by an integer ID that the caller controls.

#[pyclass(unsendable)]
pub struct Database {
    db: syster::hir::RootDatabase,
    files: HashMap<u32, syster::hir::FileText>,
}

#[pymethods]
impl Database {
    #[new]
    fn new() -> Self {
        Database {
            db: syster::hir::RootDatabase::new(),
            files: HashMap::new(),
        }
    }

    /// Register a new file. Raises `KeyError` if `file_id` is already in use;
    /// use `update_file()` to change an existing file's text.
    fn add_file(&mut self, file_id: u32, text: String) -> PyResult<()> {
        if self.files.contains_key(&file_id) {
            return Err(PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "file_id {} already registered; use update_file() to change its text",
                file_id
            )));
        }
        let fid = syster::base::FileId::new(file_id);
        let ft = syster::hir::FileText::new(&mut self.db, fid, text);
        self.files.insert(file_id, ft);
        Ok(())
    }

    /// Update the source text of an already-registered file.
    /// Salsa automatically invalidates any cached query results that depend
    /// on this file.
    fn update_file(&mut self, file_id: u32, text: String) -> PyResult<()> {
        match self.files.get(&file_id).copied() {
            Some(ft) => {
                ft.set_text(&mut self.db).to(text);
                Ok(())
            }
            None => Err(PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "file_id {} not registered; call add_file() first",
                file_id
            ))),
        }
    }

    /// Parse the registered file, returning a `SyntaxFile`.
    /// Raises `ParseError` only on a catastrophic failure where no CST could
    /// be produced at all; recoverable errors are accessible via
    /// `SyntaxFile.errors()`.
    fn parse_file(&self, file_id: u32) -> PyResult<SyntaxFile> {
        let ft = self.files.get(&file_id).copied().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "file_id {} not found",
                file_id
            ))
        })?;
        let result = syster::hir::parse_file(&self.db, ft);
        match result.get_syntax_file() {
            Some(sf) => Ok(SyntaxFile { inner: sf.clone() }),
            None => Err(ParseError::new_err(result.errors.join("; "))),
        }
    }

    /// Extract semantic symbols from the registered file.
    /// Parsing is served from Salsa's cache when the text hasn't changed;
    /// symbol extraction runs on the (possibly cached) parse result.
    fn file_symbols(&self, file_id: u32) -> PyResult<Vec<HirSymbol>> {
        let ft = self.files.get(&file_id).copied().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyKeyError, _>(format!(
                "file_id {} not found",
                file_id
            ))
        })?;
        let result = syster::hir::parse_file(&self.db, ft);
        match result.get_syntax_file() {
            Some(sf) => {
                let fid = syster::base::FileId::new(file_id);
                Ok(syster::hir::file_symbols(fid, sf)
                    .into_iter()
                    .map(HirSymbol::from)
                    .collect())
            }
            None => Err(ParseError::new_err(result.errors.join("; "))),
        }
    }

    /// Return all registered file IDs, sorted.
    fn file_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.files.keys().copied().collect();
        ids.sort();
        ids
    }
}

// ─── Module ──────────────────────────────────────────────────────────────────

#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[pymodule]
fn _syster(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("ParseError", m.py().get_type::<ParseError>())?;
    m.add_class::<ParseDiagnostic>()?;
    m.add_class::<SyntaxFile>()?;
    m.add_class::<HirSymbol>()?;
    m.add_class::<Database>()?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(parse_sysml, m)?)?;
    m.add_function(wrap_pyfunction!(parse_kerml, m)?)?;
    Ok(())
}
