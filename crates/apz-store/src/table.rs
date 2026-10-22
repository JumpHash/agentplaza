//! Structs as typed tables (W69): `derive(Table)` and `derive(Column)`.
//!
//! A struct that derives [`Table`] is stored as one row of a `STRICT` table: every field a typed column. Its lists,
//! sets, maps and optional sub-structs are child tables whose key starts with the parent's key (with a foreign key
//! to it, `ON DELETE CASCADE`); a list's child rows keep their order in a `<field>_pos` column, a map's its key in a
//! `<field>_key` column. A field marked `flatten` stores a sub-struct (or a data-carrying enum) as columns of the
//! same row, prefixed `<field>_`. Nothing is JSON: a field that holds content as written (a model's text, a tool's
//! input) is marked `content` and stored as text in a column named `<field>_content`, never read back as state.
//!
//! Field attributes (`#[table(...)]`): `key` (part of a root table's primary key), `skip` (not stored; `Default` on
//! load), `rename = "col"`, `flatten`, `child` (a `Vec`/`VecDeque` of tables, an `Option` of one, or one table: a struct that
//! holds collections cannot be flattened, so it is a child table of one row, `Default` when a load finds none), `list` (a `Vec`
//! of columns), `set` (a set of columns), `map` (a map to tables), `map_value` (a map to columns), `content`,
//! `index`, `unique`; a child table's name can be given (`child = "name"`, likewise `list`, `set`, `map`,
//! `map_value`). On the struct: `#[table(name = "table")]`.
//!
//! A component saves its state as a [`RowSet`] (every row of its tables) and [`Saved`] turns successive row sets into
//! the upserts and deletes that changed, so a write touches only what changed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

pub use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use crate::StoreError;

/// A column's SQLite type (a `STRICT` table's declared type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlType {
    Integer,
    Real,
    Text,
    Blob,
}

impl SqlType {
    /// The type as DDL declares it.
    pub fn sql(self) -> &'static str {
        match self {
            SqlType::Integer => "INTEGER",
            SqlType::Real => "REAL",
            SqlType::Text => "TEXT",
            SqlType::Blob => "BLOB",
        }
    }
}

/// One column of a table definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub ty: SqlType,
    pub nullable: bool,
    /// A `CHECK` over the column (already naming it), when its type restricts its values.
    pub check: Option<String>,
}

impl ColumnDef {
    /// The definition of a column of type `C` named `name`.
    pub fn of<C: Column>(name: &str) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            ty: C::TYPE,
            nullable: C::NULLABLE,
            check: C::check(&quote(name)),
        }
    }

    fn renamed(&self, name: String) -> ColumnDef {
        let check = self
            .check
            .as_ref()
            .map(|c| c.replace(&quote(&self.name), &quote(&name)));
        ColumnDef {
            name,
            ty: self.ty,
            nullable: self.nullable,
            check,
        }
    }

    fn ddl(&self) -> String {
        let mut out = format!("{} {}", quote(&self.name), self.ty.sql());
        if !self.nullable {
            out.push_str(" NOT NULL");
        }
        if let Some(check) = &self.check {
            let _ = write!(out, " CHECK ({check})");
        }
        out
    }
}

/// An index over some of a table's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub columns: Vec<String>,
    pub unique: bool,
}

/// A table: its name, primary key, columns, indexes and child tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    pub name: String,
    /// The primary key's columns, in order: the parent's key first (`parent` of them), then this table's own.
    pub key: Vec<ColumnDef>,
    /// How many leading key columns are the parent's key (0 for a root table).
    pub parent: usize,
    /// The parent table and the names its key columns have there, for the foreign key.
    pub parent_table: Option<(String, Vec<String>)>,
    /// The other columns.
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    pub children: Vec<TableDef>,
    /// A root table whose rows have their own key keeps the list's order in a `pos` column (its first column).
    pub ordered: bool,
}

/// Wraps an identifier in double quotes (doubling any inside), so any field name is a valid column name.
pub fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

impl TableDef {
    /// This table and every table below it, parents before children.
    pub fn walk(&self) -> Vec<&TableDef> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.walk());
        }
        out
    }

    /// The DDL that creates this table, its indexes and every table below it.
    pub fn ddl(&self) -> String {
        let mut out = String::new();
        for table in self.walk() {
            out.push_str(&table.own_ddl());
        }
        out
    }

    fn own_ddl(&self) -> String {
        let mut lines: Vec<String> = self
            .key
            .iter()
            .chain(&self.columns)
            .map(ColumnDef::ddl)
            .collect();
        let key: Vec<String> = self.key.iter().map(|c| quote(&c.name)).collect();
        lines.push(format!("PRIMARY KEY ({})", key.join(", ")));
        if let Some((parent, cols)) = &self.parent_table {
            let ours: Vec<String> = self.key[..self.parent]
                .iter()
                .map(|c| quote(&c.name))
                .collect();
            let theirs: Vec<String> = cols.iter().map(|c| quote(c)).collect();
            lines.push(format!(
                "FOREIGN KEY ({}) REFERENCES {} ({}) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED",
                ours.join(", "),
                quote(parent),
                theirs.join(", ")
            ));
        }
        let mut out = format!(
            "CREATE TABLE {} (\n    {}\n) STRICT;\n",
            quote(&self.name),
            lines.join(",\n    ")
        );
        for index in &self.indexes {
            let cols: Vec<String> = index.columns.iter().map(|c| quote(c)).collect();
            let _ = writeln!(
                out,
                "CREATE {}INDEX {} ON {} ({});",
                if index.unique { "UNIQUE " } else { "" },
                quote(&format!("{}_{}", self.name, index.columns.join("_"))),
                quote(&self.name),
                cols.join(", ")
            );
        }
        // A child is found by its parent's key: the primary key's prefix serves it, no extra index needed.
        out
    }

    /// Checks the definition: column names unique per table, table names unique, key not empty.
    pub fn validate(&self) -> Result<(), StoreError> {
        let mut tables = BTreeSet::new();
        for table in self.walk() {
            if !tables.insert(table.name.as_str()) {
                return Err(StoreError::invalid(format!(
                    "table {} is defined twice",
                    table.name
                )));
            }
            if table.key.is_empty() {
                return Err(StoreError::invalid(format!(
                    "table {} has no key",
                    table.name
                )));
            }
            let mut names = BTreeSet::new();
            for col in table.key.iter().chain(&table.columns) {
                if !names.insert(col.name.as_str()) {
                    return Err(StoreError::invalid(format!(
                        "table {} has two columns named {}",
                        table.name, col.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn all_columns(&self) -> impl Iterator<Item = &ColumnDef> {
        self.key.iter().chain(&self.columns)
    }

    fn upsert_sql(&self) -> String {
        let cols: Vec<String> = self.all_columns().map(|c| quote(&c.name)).collect();
        let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let key: Vec<String> = self.key.iter().map(|c| quote(&c.name)).collect();
        let set: Vec<String> = self
            .columns
            .iter()
            .map(|c| format!("{0} = excluded.{0}", quote(&c.name)))
            .collect();
        let action = if set.is_empty() {
            "DO NOTHING".to_string()
        } else {
            format!("DO UPDATE SET {}", set.join(", "))
        };
        format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) {action}",
            quote(&self.name),
            cols.join(", "),
            marks.join(", "),
            key.join(", ")
        )
    }

    fn delete_sql(&self) -> String {
        let cond: Vec<String> = self
            .key
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{} = ?{}", quote(&c.name), i + 1))
            .collect();
        format!(
            "DELETE FROM {} WHERE {}",
            quote(&self.name),
            cond.join(" AND ")
        )
    }

    fn select_sql(&self) -> String {
        let cols: Vec<String> = self.all_columns().map(|c| quote(&c.name)).collect();
        let order = if self.ordered {
            quote("pos")
        } else {
            self.key
                .iter()
                .map(|c| quote(&c.name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "SELECT {} FROM {} ORDER BY {order}",
            cols.join(", "),
            quote(&self.name)
        )
    }

    /// The statements a component's code runs against this table and its children, for the test that prepares every
    /// statement against the migrated schema.
    pub fn statements(&self) -> Vec<String> {
        self.walk()
            .into_iter()
            .flat_map(|t| [t.upsert_sql(), t.delete_sql(), t.select_sql()])
            .collect()
    }

    /// The key columns a child of this table starts with: a root's key columns renamed `<table>_<column>`, a child's
    /// as they are (already unique along the lineage).
    fn key_for_children(&self) -> Vec<ColumnDef> {
        if self.parent_table.is_none() {
            self.key
                .iter()
                .map(|c| c.renamed(format!("{}_{}", self.name, c.name)))
                .collect()
        } else {
            self.key.clone()
        }
    }

    /// The context a child field of this table is defined in.
    pub fn child_ctx(&self, field: &str, table: Option<&str>) -> Ctx {
        Ctx {
            name: table.map_or_else(|| format!("{}_{field}", self.name), str::to_string),
            parent_key: self.key_for_children(),
            parent_table: Some((
                self.name.clone(),
                self.key.iter().map(|c| c.name.clone()).collect(),
            )),
            field: field.to_string(),
            place: None,
        }
    }
}

/// Where a table is being defined: its name and, for a child, its parent's key and the field it stores.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub name: String,
    pub parent_key: Vec<ColumnDef>,
    pub parent_table: Option<(String, Vec<String>)>,
    pub field: String,
    /// The column that places a child row under its parent: `<field>_pos` for a list, `<field>_key` for a map, none
    /// for an optional child (one row per parent).
    pub place: Option<ColumnDef>,
}

impl Ctx {
    /// The context of a root table named `name`.
    pub fn root(name: &str) -> Ctx {
        Ctx {
            name: name.to_string(),
            parent_key: Vec::new(),
            parent_table: None,
            field: String::new(),
            place: None,
        }
    }

    /// Whether this is a root table.
    pub fn is_root(&self) -> bool {
        self.parent_table.is_none()
    }

    /// The key a child starts with plus `extra`, named so it does not clash with the parent's key.
    fn key_with(&self, extra: ColumnDef) -> Vec<ColumnDef> {
        let mut key = self.parent_key.clone();
        let mut name = extra.name.clone();
        let mut n = 2;
        while key.iter().any(|c| c.name == name) {
            name = format!("{}_{n}", extra.name);
            n += 1;
        }
        key.push(extra.renamed(name));
        key
    }

    fn table(&self, key: Vec<ColumnDef>, columns: Vec<ColumnDef>) -> TableDef {
        TableDef {
            name: self.name.clone(),
            parent: self.parent_key.len(),
            key,
            parent_table: self.parent_table.clone(),
            columns,
            indexes: Vec::new(),
            children: Vec::new(),
            ordered: false,
        }
    }
}

/// A value stored in one typed column. Implemented for the integers, floats, `bool`, `String`, `Vec<u8>` (a blob),
/// `PathBuf`, and `Option` of any of them; derived for enums of unit variants (text with a `CHECK`) and newtypes.
pub trait Column: Sized {
    const TYPE: SqlType;
    const NULLABLE: bool = false;
    /// A `CHECK` over the column (named `quoted`), when the type restricts its values.
    fn check(quoted: &str) -> Option<String> {
        let _ = quoted;
        None
    }
    fn to_value(&self) -> Result<Value, StoreError>;
    fn from_value(value: Value) -> Result<Self, StoreError>;
}

fn mismatch(want: &str, got: &Value) -> StoreError {
    StoreError::invalid(format!("expected {want}, found {got:?}"))
}

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl Column for $t {
            const TYPE: SqlType = SqlType::Integer;
            fn to_value(&self) -> Result<Value, StoreError> {
                Ok(Value::Integer(i64::from(*self)))
            }
            fn from_value(value: Value) -> Result<Self, StoreError> {
                match value {
                    Value::Integer(i) => <$t>::try_from(i).map_err(|_| StoreError::invalid(format!("{i} does not fit {}", stringify!($t)))),
                    other => Err(mismatch("an integer", &other)),
                }
            }
        }
    )*};
}
signed!(i8, i16, i32, i64);

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl Column for $t {
            const TYPE: SqlType = SqlType::Integer;
            fn check(quoted: &str) -> Option<String> {
                Some(format!("{quoted} >= 0"))
            }
            fn to_value(&self) -> Result<Value, StoreError> {
                i64::try_from(*self)
                    .map(Value::Integer)
                    .map_err(|_| StoreError::invalid(format!("{} does not fit a column (past i64)", self)))
            }
            fn from_value(value: Value) -> Result<Self, StoreError> {
                match value {
                    Value::Integer(i) => <$t>::try_from(i).map_err(|_| StoreError::invalid(format!("{i} does not fit {}", stringify!($t)))),
                    other => Err(mismatch("an integer", &other)),
                }
            }
        }
    )*};
}
unsigned!(u8, u16, u32, u64, usize);

impl Column for isize {
    const TYPE: SqlType = SqlType::Integer;
    fn to_value(&self) -> Result<Value, StoreError> {
        Ok(Value::Integer(*self as i64))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Integer(i) => isize::try_from(i).map_err(StoreError::invalid),
            other => Err(mismatch("an integer", &other)),
        }
    }
}

impl Column for bool {
    const TYPE: SqlType = SqlType::Integer;
    fn check(quoted: &str) -> Option<String> {
        Some(format!("{quoted} IN (0, 1)"))
    }
    fn to_value(&self) -> Result<Value, StoreError> {
        Ok(Value::Integer(i64::from(*self)))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Integer(0) => Ok(false),
            Value::Integer(1) => Ok(true),
            other => Err(mismatch("0 or 1", &other)),
        }
    }
}

impl Column for f64 {
    const TYPE: SqlType = SqlType::Real;
    fn to_value(&self) -> Result<Value, StoreError> {
        if self.is_nan() {
            return Err(StoreError::invalid("NaN cannot be stored"));
        }
        Ok(Value::Real(*self))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Real(f) => Ok(f),
            // A STRICT REAL column keeps a whole number as REAL, but be lenient with integers written before.
            Value::Integer(i) => Ok(i as f64),
            other => Err(mismatch("a real", &other)),
        }
    }
}

impl Column for f32 {
    const TYPE: SqlType = SqlType::Real;
    fn to_value(&self) -> Result<Value, StoreError> {
        f64::from(*self).to_value()
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        f64::from_value(value).map(|f| f as f32)
    }
}

impl Column for String {
    const TYPE: SqlType = SqlType::Text;
    fn to_value(&self) -> Result<Value, StoreError> {
        Ok(Value::Text(self.clone()))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Text(s) => Ok(s),
            other => Err(mismatch("text", &other)),
        }
    }
}

impl Column for Vec<u8> {
    const TYPE: SqlType = SqlType::Blob;
    fn to_value(&self) -> Result<Value, StoreError> {
        Ok(Value::Blob(self.clone()))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Blob(b) => Ok(b),
            other => Err(mismatch("a blob", &other)),
        }
    }
}

impl Column for PathBuf {
    const TYPE: SqlType = SqlType::Text;
    fn to_value(&self) -> Result<Value, StoreError> {
        self.to_str()
            .map(|s| Value::Text(s.to_string()))
            .ok_or_else(|| StoreError::invalid(format!("the path {} is not UTF-8", self.display())))
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        String::from_value(value).map(PathBuf::from)
    }
}

impl<T: Column> Column for Option<T> {
    const TYPE: SqlType = T::TYPE;
    const NULLABLE: bool = true;
    fn check(quoted: &str) -> Option<String> {
        // A CHECK passes on NULL (it is not false), so the inner type's check serves as it is.
        T::check(quoted)
    }
    fn to_value(&self) -> Result<Value, StoreError> {
        match self {
            Some(v) => v.to_value(),
            None => Ok(Value::Null),
        }
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        match value {
            Value::Null => Ok(None),
            v => T::from_value(v).map(Some),
        }
    }
}

impl<T: Column> Column for Box<T> {
    const TYPE: SqlType = T::TYPE;
    const NULLABLE: bool = T::NULLABLE;
    fn check(quoted: &str) -> Option<String> {
        T::check(quoted)
    }
    fn to_value(&self) -> Result<Value, StoreError> {
        (**self).to_value()
    }
    fn from_value(value: Value) -> Result<Self, StoreError> {
        T::from_value(value).map(Box::new)
    }
}

/// Content kept as written (a message's text, a tool's input as the model sent it): stored as text in a
/// `<field>_content` column and never read back as state (W69). Implemented for `String`, `Option<String>`, and with
/// the `json` feature for `serde_json::Value` (its text exactly as serialized).
pub trait Content: Sized {
    fn to_text(&self) -> Result<Option<String>, StoreError>;
    fn from_text(text: Option<String>) -> Result<Self, StoreError>;
}

impl Content for String {
    fn to_text(&self) -> Result<Option<String>, StoreError> {
        Ok(Some(self.clone()))
    }
    fn from_text(text: Option<String>) -> Result<Self, StoreError> {
        Ok(text.unwrap_or_default())
    }
}

impl Content for Option<String> {
    fn to_text(&self) -> Result<Option<String>, StoreError> {
        Ok(self.clone())
    }
    fn from_text(text: Option<String>) -> Result<Self, StoreError> {
        Ok(text)
    }
}

#[cfg(feature = "json")]
impl Content for serde_json::Value {
    fn to_text(&self) -> Result<Option<String>, StoreError> {
        Ok(Some(
            serde_json::to_string(self).map_err(StoreError::invalid)?,
        ))
    }
    fn from_text(text: Option<String>) -> Result<Self, StoreError> {
        match text {
            None => Ok(serde_json::Value::Null),
            Some(t) => serde_json::from_str(&t).map_err(StoreError::invalid),
        }
    }
}

#[cfg(feature = "json")]
impl Content for Option<serde_json::Value> {
    fn to_text(&self) -> Result<Option<String>, StoreError> {
        match self {
            None => Ok(None),
            Some(v) => v.to_text(),
        }
    }
    fn from_text(text: Option<String>) -> Result<Self, StoreError> {
        text.map(|t| serde_json::from_str(&t).map_err(StoreError::invalid))
            .transpose()
    }
}

/// The column a `content` field is stored in.
pub fn content_column(name: &str) -> ColumnDef {
    let name = if name.ends_with("_content") {
        name.to_string()
    } else {
        format!("{name}_content")
    };
    ColumnDef {
        name,
        ty: SqlType::Text,
        nullable: true,
        check: None,
    }
}

/// Puts a content field's text.
pub fn put_content<C: Content>(value: &C, out: &mut Vec<Value>) -> Result<(), StoreError> {
    out.push(value.to_text()?.map_or(Value::Null, Value::Text));
    Ok(())
}

/// Takes a content field's text.
pub fn take_content<C: Content>(values: &mut std::vec::IntoIter<Value>) -> Result<C, StoreError> {
    match next(values)? {
        Value::Null => C::from_text(None),
        Value::Text(t) => C::from_text(Some(t)),
        other => Err(mismatch("text", &other)),
    }
}

/// The element type of a collection field (`Vec`, `VecDeque`, sets, `Option`), for child tables.
pub trait Elem {
    type Item;
}
impl<T> Elem for Vec<T> {
    type Item = T;
}
impl<T> Elem for VecDeque<T> {
    type Item = T;
}
impl<T> Elem for BTreeSet<T> {
    type Item = T;
}
impl<T, S> Elem for HashSet<T, S> {
    type Item = T;
}
impl<T> Elem for Option<T> {
    type Item = T;
}
impl<T> Elem for Box<T> {
    type Item = T;
}

/// The key and value types of a map field, for child tables.
pub trait MapElem {
    type Key;
    type Value;
}
impl<K, V> MapElem for BTreeMap<K, V> {
    type Key = K;
    type Value = V;
}
impl<K, V, S> MapElem for HashMap<K, V, S> {
    type Key = K;
    type Value = V;
}

/// A struct stored as a table (derived; see the module docs).
pub trait Table: Sized {
    /// The table's name as a root (`#[table(name = …)]`), or the struct's name in snake case.
    const NAME: &'static str;

    /// The table's definition in `ctx`.
    fn define(ctx: Ctx) -> TableDef;

    /// Appends this value's row to `out` (its key starts with `key`: the parent's key and the position or map key
    /// that places it, empty for a root) and its children's rows.
    fn put(&self, def: &TableDef, key: Vec<Value>, out: &mut RowSet) -> Result<(), StoreError>;

    /// Rebuilds a value from its row and its children's rows.
    fn take(def: &TableDef, row: &Row, loaded: &Loaded) -> Result<Self, StoreError>;

    /// The columns this value adds to its parent's row when a field holding it is `flatten`ed (prefixed `prefix`).
    fn flat_columns(prefix: &str, out: &mut Vec<ColumnDef>);

    /// The values of [`Table::flat_columns`], in order.
    fn flat_put(&self, out: &mut Vec<Value>) -> Result<(), StoreError>;

    /// Rebuilds a flattened value from its columns.
    fn flat_take(values: &mut std::vec::IntoIter<Value>) -> Result<Self, StoreError>;

    /// The definition of this type as a root table.
    fn root() -> TableDef {
        Self::define(Ctx::root(Self::NAME))
    }
}

/// The root table definition of a type, defined once.
#[macro_export]
macro_rules! root_def {
    ($t:ty) => {{
        static DEF: ::std::sync::OnceLock<$crate::TableDef> = ::std::sync::OnceLock::new();
        DEF.get_or_init(<$t as $crate::Table>::root)
    }};
}

/// One stored row: its key's values and its other columns' values.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: Vec<Value>,
    pub columns: Vec<Value>,
}

/// Splits a row into the values of the struct's own key fields and of its other columns. A root with its own key
/// has them as its key (and `pos` as its first column, skipped); a root without has none; a child stores its own key
/// fields as its first `own` columns.
pub fn split(
    def: &TableDef,
    row: &Row,
    own: usize,
) -> (std::vec::IntoIter<Value>, std::vec::IntoIter<Value>) {
    if def.parent_table.is_none() {
        if def.ordered {
            (
                row.key.clone().into_iter(),
                Vec::from(&row.columns[1..]).into_iter(),
            )
        } else {
            (Vec::new().into_iter(), row.columns.clone().into_iter())
        }
    } else {
        let own = own.min(row.columns.len());
        (
            Vec::from(&row.columns[..own]).into_iter(),
            Vec::from(&row.columns[own..]).into_iter(),
        )
    }
}

/// The next value of a row being taken apart.
pub fn next(values: &mut std::vec::IntoIter<Value>) -> Result<Value, StoreError> {
    values
        .next()
        .ok_or_else(|| StoreError::invalid("a row has fewer columns than its struct"))
}

/// Takes a column of type `C`.
pub fn take_col<C: Column>(
    values: &mut std::vec::IntoIter<Value>,
    field: &str,
) -> Result<C, StoreError> {
    C::from_value(next(values)?).map_err(|e| StoreError::invalid(format!("{field}: {e}")))
}

/// Every row of a component's tables, as its state is now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowSet {
    pub tables: BTreeMap<String, Vec<Vec<Value>>>,
}

impl RowSet {
    /// An empty row set.
    pub fn new() -> RowSet {
        RowSet::default()
    }

    /// Adds a row (its key's values, then its columns') to `table`.
    pub fn push(&mut self, table: &str, row: Vec<Value>) {
        self.tables.entry(table.to_string()).or_default().push(row);
    }

    /// Every row `def`'s tables hold now (each its key's values, then its columns', as [`RowSet::list`] lays them
    /// out). A component that loaded from these tables starts its [`Saved`] from them, so its first save writes what
    /// its state has that they do not: a single value's default root when its table is empty, what its load changed.
    pub fn stored(conn: &Connection, def: &TableDef) -> Result<RowSet, StoreError> {
        let mut out = RowSet::new();
        for table in def.walk() {
            let rows = out.tables.entry(table.name.clone()).or_default();
            let mut stmt = conn.prepare_cached(&table.select_sql())?;
            let width = table.key.len() + table.columns.len();
            let mut found = stmt.query([])?;
            while let Some(r) = found.next()? {
                let mut row = Vec::with_capacity(width);
                for i in 0..width {
                    row.push(r.get::<_, Value>(i)?);
                }
                rows.push(row);
            }
        }
        Ok(out)
    }

    /// The rows of a root list of `items`, in order.
    pub fn list<'a, T: Table + 'a>(
        def: &TableDef,
        items: impl IntoIterator<Item = &'a T>,
    ) -> Result<RowSet, StoreError> {
        let mut out = RowSet::new();
        // Every table of the definition is present, so a list that became empty deletes its rows.
        for table in def.walk() {
            out.tables.entry(table.name.clone()).or_default();
        }
        for (pos, item) in items.into_iter().enumerate() {
            item.put(def, vec![pos_value(pos)], &mut out)?;
        }
        Ok(out)
    }

    /// The rows of a single root value (stored at position 0 of its list).
    pub fn one<T: Table>(def: &TableDef, item: &T) -> Result<RowSet, StoreError> {
        RowSet::list(def, std::iter::once(item))
    }

    /// Adds another row set's rows (two components' rows saved together).
    pub fn extend(&mut self, other: RowSet) {
        for (table, rows) in other.tables {
            self.tables.entry(table).or_default().extend(rows);
        }
    }
}

fn pos_value(pos: usize) -> Value {
    Value::Integer(i64::try_from(pos).unwrap_or(i64::MAX))
}

/// Lays out a row being put: its full key and the columns that come before the struct's other columns. `key` is what
/// the caller placed it with (`[pos]` for a root; the parent's key and the placing value for a child) and `own` the
/// values of the struct's own key fields.
pub fn layout(def: &TableDef, key: Vec<Value>, own: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    if def.parent_table.is_none() {
        if def.ordered {
            // key = [pos]: the struct's own key is the table's key, and `pos` its first column.
            (own, key)
        } else {
            // A root without key fields is keyed by its position.
            (key, Vec::new())
        }
    } else {
        // A child is keyed by its parent's key and its placing column; its own key fields are its first columns.
        (key, own)
    }
}

/// Puts a struct's row: `key` and `lead` from [`layout`], then `columns`. Returns the row's full key, which its
/// children's rows start with.
pub fn put_row(
    def: &TableDef,
    key: Vec<Value>,
    lead: Vec<Value>,
    columns: Vec<Value>,
    out: &mut RowSet,
) -> Vec<Value> {
    let mut row = key.clone();
    row.extend(lead);
    row.extend(columns);
    out.push(&def.name, row);
    key
}

/// The definition of a struct's own table in `ctx`: a root keyed by its key fields (and ordered by `pos`), or by
/// `pos` alone when it has none; a child keyed by its parent's key and its placing column (`ctx.place`).
pub fn struct_table(ctx: &Ctx, own_key: Vec<ColumnDef>, mut columns: Vec<ColumnDef>) -> TableDef {
    if ctx.is_root() {
        if own_key.is_empty() {
            let key = vec![ColumnDef::of::<u64>("pos")];
            ctx.table(key, columns)
        } else {
            let mut cols = vec![ColumnDef::of::<u64>("pos")];
            cols.extend(columns);
            let mut def = ctx.table(own_key, cols);
            def.ordered = true;
            def
        }
    } else {
        let key = match &ctx.place {
            Some(place) => ctx.key_with(place.clone()),
            None => ctx.parent_key.clone(),
        };
        let mut all = own_key;
        all.append(&mut columns);
        ctx.table(key, all)
    }
}

/// The placing column of a list's child rows: `<field>_pos`.
pub fn pos_column(field: &str) -> ColumnDef {
    ColumnDef::of::<u64>(&format!("{field}_pos"))
}

/// The placing column of a map's child rows: `<field>_key`, of the map key's type.
pub fn key_column<K: Column>(field: &str) -> ColumnDef {
    ColumnDef::of::<K>(&format!("{field}_key"))
}

/// A child table of structs (`child` on a `Vec`/`VecDeque`): keyed by the parent's key and `<field>_pos`.
pub fn children_def<T: Table>(parent: &TableDef, field: &str, table: Option<&str>) -> TableDef {
    let mut ctx = parent.child_ctx(field, table);
    ctx.place = Some(pos_column(field));
    T::define(ctx)
}

/// A child table holding at most one struct (`child` on an `Option`): keyed by the parent's key alone.
pub fn option_def<T: Table>(parent: &TableDef, field: &str, table: Option<&str>) -> TableDef {
    T::define(parent.child_ctx(field, table))
}

/// A child table of a map to structs (`map`): keyed by the parent's key and `<field>_key`.
pub fn map_def<K: Column, V: Table>(
    parent: &TableDef,
    field: &str,
    table: Option<&str>,
) -> TableDef {
    let mut ctx = parent.child_ctx(field, table);
    ctx.place = Some(key_column::<K>(field));
    V::define(ctx)
}

/// A child table of a list of columns (`list`): keyed by the parent's key and `<field>_pos`, one `value` column.
pub fn list_def<V: Column>(parent: &TableDef, field: &str, table: Option<&str>) -> TableDef {
    let ctx = parent.child_ctx(field, table);
    let key = ctx.key_with(pos_column(field));
    ctx.table(key, vec![ColumnDef::of::<V>("value")])
}

/// A child table of a set of columns (`set`): keyed by the parent's key and the value itself.
pub fn set_def<V: Column>(parent: &TableDef, field: &str, table: Option<&str>) -> TableDef {
    let ctx = parent.child_ctx(field, table);
    let key = ctx.key_with(ColumnDef::of::<V>("value"));
    ctx.table(key, Vec::new())
}

/// A child table of a map to columns (`map_value`): keyed by the parent's key and `<field>_key`, one `value` column.
pub fn map_value_def<K: Column, V: Column>(
    parent: &TableDef,
    field: &str,
    table: Option<&str>,
) -> TableDef {
    let ctx = parent.child_ctx(field, table);
    let key = ctx.key_with(key_column::<K>(field));
    ctx.table(key, vec![ColumnDef::of::<V>("value")])
}

/// Puts a list of structs' rows.
pub fn put_children<'a, T: Table + 'a>(
    def: &TableDef,
    parent_key: &[Value],
    items: impl IntoIterator<Item = &'a T>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    for (pos, item) in items.into_iter().enumerate() {
        let mut key = parent_key.to_vec();
        key.push(pos_value(pos));
        item.put(def, key, out)?;
    }
    Ok(())
}

/// Puts an optional struct's row.
pub fn put_option<T: Table>(
    def: &TableDef,
    parent_key: &[Value],
    item: Option<&T>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    if let Some(item) = item {
        item.put(def, parent_key.to_vec(), out)?;
    }
    Ok(())
}

/// Puts a map to structs' rows.
pub fn put_map<'a, K: Column + 'a, V: Table + 'a>(
    def: &TableDef,
    parent_key: &[Value],
    items: impl IntoIterator<Item = (&'a K, &'a V)>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    for (k, v) in items {
        let mut key = parent_key.to_vec();
        key.push(k.to_value()?);
        v.put(def, key, out)?;
    }
    Ok(())
}

/// Puts a list of columns' rows.
pub fn put_list<'a, V: Column + 'a>(
    def: &TableDef,
    parent_key: &[Value],
    items: impl IntoIterator<Item = &'a V>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    for (pos, v) in items.into_iter().enumerate() {
        let mut row = parent_key.to_vec();
        row.push(pos_value(pos));
        row.push(v.to_value()?);
        out.push(&def.name, row);
    }
    Ok(())
}

/// Puts a set of columns' rows.
pub fn put_set<'a, V: Column + 'a>(
    def: &TableDef,
    parent_key: &[Value],
    items: impl IntoIterator<Item = &'a V>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    for v in items {
        let mut row = parent_key.to_vec();
        row.push(v.to_value()?);
        out.push(&def.name, row);
    }
    Ok(())
}

/// Puts a map to columns' rows.
pub fn put_map_values<'a, K: Column + 'a, V: Column + 'a>(
    def: &TableDef,
    parent_key: &[Value],
    items: impl IntoIterator<Item = (&'a K, &'a V)>,
    out: &mut RowSet,
) -> Result<(), StoreError> {
    for (k, v) in items {
        let mut row = parent_key.to_vec();
        row.push(k.to_value()?);
        row.push(v.to_value()?);
        out.push(&def.name, row);
    }
    Ok(())
}

/// Takes a list of structs.
pub fn take_children<T: Table>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Vec<T>, StoreError> {
    loaded
        .children(def, parent_key)
        .iter()
        .map(|row| T::take(def, row, loaded))
        .collect()
}

/// Takes an optional struct.
pub fn take_option<T: Table>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Option<T>, StoreError> {
    loaded
        .children(def, parent_key)
        .first()
        .map(|row| T::take(def, row, loaded))
        .transpose()
}

/// Takes a map to structs, in key order.
pub fn take_map<K: Column, V: Table>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Vec<(K, V)>, StoreError> {
    loaded
        .children(def, parent_key)
        .iter()
        .map(|row| {
            let k = K::from_value(row.key[def.parent].clone())?;
            Ok((k, V::take(def, row, loaded)?))
        })
        .collect()
}

/// Takes a list of columns.
pub fn take_list<V: Column>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Vec<V>, StoreError> {
    loaded
        .children(def, parent_key)
        .iter()
        .map(|row| V::from_value(row.columns[0].clone()))
        .collect()
}

/// Takes a set of columns.
pub fn take_set<V: Column>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Vec<V>, StoreError> {
    loaded
        .children(def, parent_key)
        .iter()
        .map(|row| V::from_value(row.key[def.parent].clone()))
        .collect()
}

/// Takes a map to columns, in key order.
pub fn take_map_values<K: Column, V: Column>(
    def: &TableDef,
    parent_key: &[Value],
    loaded: &Loaded,
) -> Result<Vec<(K, V)>, StoreError> {
    loaded
        .children(def, parent_key)
        .iter()
        .map(|row| {
            Ok((
                K::from_value(row.key[def.parent].clone())?,
                V::from_value(row.columns[0].clone())?,
            ))
        })
        .collect()
}

/// Encodes a key's values as bytes, so keys can be compared and looked up whatever their types.
fn encode(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        match v {
            Value::Null => out.push(0),
            Value::Integer(i) => {
                out.push(1);
                out.extend_from_slice(&i.to_be_bytes());
            }
            Value::Real(f) => {
                out.push(2);
                out.extend_from_slice(&f.to_bits().to_be_bytes());
            }
            Value::Text(s) => {
                out.push(3);
                out.extend_from_slice(&(s.len() as u64).to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Value::Blob(b) => {
                out.push(4);
                out.extend_from_slice(&(b.len() as u64).to_be_bytes());
                out.extend_from_slice(b);
            }
        }
    }
    out
}

fn hash_row(row: &[Value]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    encode(row).hash(&mut h);
    h.finish()
}

/// Every row of a component's tables as read back, grouped by parent key for its children.
#[derive(Debug, Default)]
pub struct Loaded {
    /// table → parent key (encoded) → rows in key order.
    tables: HashMap<String, BTreeMap<Vec<u8>, Vec<Row>>>,
    roots: Vec<Row>,
}

impl Loaded {
    /// Reads every table of `def` through `conn`.
    pub fn read(conn: &Connection, def: &TableDef) -> Result<Loaded, StoreError> {
        let mut loaded = Loaded::default();
        for table in def.walk() {
            let mut stmt = conn.prepare_cached(&table.select_sql())?;
            let width = table.key.len() + table.columns.len();
            let mut rows = stmt.query([])?;
            let mut grouped: BTreeMap<Vec<u8>, Vec<Row>> = BTreeMap::new();
            while let Some(r) = rows.next()? {
                let mut values = Vec::with_capacity(width);
                for i in 0..width {
                    values.push(r.get::<_, Value>(i)?);
                }
                let columns = values.split_off(table.key.len());
                let row = Row {
                    key: values,
                    columns,
                };
                if table.parent_table.is_none() {
                    loaded.roots.push(row);
                } else {
                    grouped
                        .entry(encode(&row.key[..table.parent]))
                        .or_default()
                        .push(row);
                }
            }
            loaded.tables.insert(table.name.clone(), grouped);
        }
        Ok(loaded)
    }

    /// The rows of child table `def` under the parent whose full key is `parent_key`.
    pub fn children(&self, def: &TableDef, parent_key: &[Value]) -> &[Row] {
        self.tables
            .get(&def.name)
            .and_then(|t| t.get(&encode(parent_key)))
            .map_or(&[], Vec::as_slice)
    }

    /// The root rows, in their list's order.
    pub fn roots(&self) -> &[Row] {
        &self.roots
    }
}

/// Reads a root list of `T`.
pub fn load_list<T: Table>(conn: &Connection, def: &TableDef) -> Result<Vec<T>, StoreError> {
    let loaded = Loaded::read(conn, def)?;
    loaded
        .roots()
        .iter()
        .map(|row| T::take(def, row, &loaded))
        .collect()
}

/// Reads a single root value (the first of its list), if one is stored.
pub fn load_one<T: Table>(conn: &Connection, def: &TableDef) -> Result<Option<T>, StoreError> {
    Ok(load_list(conn, def)?.into_iter().next())
}

/// What a component last saved: each row's hash by table and key, so the next save writes only what changed.
#[derive(Debug, Default, Clone)]
pub struct Saved {
    rows: HashMap<String, HashMap<Vec<u8>, u64>>,
}

/// The writes that bring the stored rows to a new row set.
#[derive(Debug, Default)]
pub struct Changes {
    /// (table, row) to insert or update.
    pub upserts: Vec<(String, Vec<Value>)>,
    /// (table, key) to delete.
    pub deletes: Vec<(String, Vec<Value>)>,
    next: HashMap<String, HashMap<Vec<u8>, u64>>,
}

impl Changes {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.deletes.is_empty()
    }
}

impl Saved {
    /// What `rows` (as stored now) means as a saved state.
    pub fn of(def: &TableDef, rows: &RowSet) -> Saved {
        let mut saved = Saved::default();
        for table in def.walk() {
            let mut hashes = HashMap::new();
            for row in rows.tables.get(&table.name).into_iter().flatten() {
                hashes.insert(encode(&row[..table.key.len()]), hash_row(row));
            }
            saved.rows.insert(table.name.clone(), hashes);
        }
        saved
    }

    /// The writes from the saved state to `rows`. Apply them with [`apply`], then [`Saved::commit`] them.
    pub fn diff(&self, def: &TableDef, rows: &RowSet) -> Changes {
        let mut changes = Changes::default();
        for table in def.walk() {
            let before = self.rows.get(&table.name);
            let mut now = HashMap::new();
            for row in rows.tables.get(&table.name).into_iter().flatten() {
                let key = encode(&row[..table.key.len()]);
                let hash = hash_row(row);
                if before.and_then(|b| b.get(&key)) != Some(&hash) {
                    changes.upserts.push((table.name.clone(), row.clone()));
                }
                now.insert(key, hash);
            }
            if let Some(before) = before {
                let gone = before.keys().filter(|k| !now.contains_key(*k));
                for key in gone {
                    changes.deletes.push((table.name.clone(), decode(key)));
                }
            }
            changes.next.insert(table.name.clone(), now);
        }
        changes
    }

    /// Records `changes` as saved, once their write committed.
    pub fn commit(&mut self, changes: Changes) {
        self.rows = changes.next;
    }
}

/// Decodes a key [`encode`] wrote.
fn decode(mut bytes: &[u8]) -> Vec<Value> {
    fn take<const N: usize>(bytes: &mut &[u8]) -> [u8; N] {
        let (head, rest) = bytes.split_at(N);
        *bytes = rest;
        head.try_into().unwrap_or([0; N])
    }
    let mut out = Vec::new();
    while let Some((&tag, rest)) = bytes.split_first() {
        bytes = rest;
        match tag {
            1 => out.push(Value::Integer(i64::from_be_bytes(take::<8>(&mut bytes)))),
            2 => out.push(Value::Real(f64::from_bits(u64::from_be_bytes(take::<8>(
                &mut bytes,
            ))))),
            3 | 4 => {
                let len = u64::from_be_bytes(take::<8>(&mut bytes)) as usize;
                let (data, rest) = bytes.split_at(len);
                bytes = rest;
                out.push(if tag == 3 {
                    Value::Text(String::from_utf8_lossy(data).into_owned())
                } else {
                    Value::Blob(data.to_vec())
                });
            }
            _ => out.push(Value::Null),
        }
    }
    out
}

/// Applies `changes` to the tables of `def` through `conn` (inside the writer's transaction): deletes, then upserts.
/// Foreign keys are deferred, so the order of tables does not matter within the transaction.
pub fn apply(conn: &Connection, def: &TableDef, changes: &Changes) -> Result<(), StoreError> {
    let tables: HashMap<&str, &TableDef> = def
        .walk()
        .into_iter()
        .map(|t| (t.name.as_str(), t))
        .collect();
    for (table, key) in &changes.deletes {
        let t = tables[table.as_str()];
        conn.prepare_cached(&t.delete_sql())?
            .execute(params_from_iter(key.iter()))?;
    }
    for (table, row) in &changes.upserts {
        let t = tables[table.as_str()];
        conn.prepare_cached(&t.upsert_sql())?
            .execute(params_from_iter(row.iter()))?;
    }
    Ok(())
}

/// Rewrites every table of `def` to hold exactly `rows` (a first save, or a restore): deletes every row, then inserts.
pub fn replace(conn: &Connection, def: &TableDef, rows: &RowSet) -> Result<(), StoreError> {
    for table in def.walk().into_iter().rev() {
        conn.execute(&format!("DELETE FROM {}", quote(&table.name)), [])?;
    }
    for table in def.walk() {
        let sql = table.upsert_sql();
        let mut stmt = conn.prepare_cached(&sql)?;
        for row in rows.tables.get(&table.name).into_iter().flatten() {
            stmt.execute(params_from_iter(row.iter()))?;
        }
    }
    Ok(())
}

/// An optional struct flattened into its parent's row: a `<prefix>present` column (0 or 1) and the struct's columns,
/// all nullable. Only flattening is meaningful for it; as a table of its own, use `child` on the `Option` field.
impl<T: Table> Table for Option<T> {
    const NAME: &'static str = T::NAME;

    fn define(ctx: Ctx) -> TableDef {
        let mut columns = Vec::new();
        Self::flat_columns("", &mut columns);
        struct_table(&ctx, Vec::new(), columns)
    }

    fn put(&self, def: &TableDef, key: Vec<Value>, out: &mut RowSet) -> Result<(), StoreError> {
        let (key, lead) = layout(def, key, Vec::new());
        let mut columns = Vec::new();
        self.flat_put(&mut columns)?;
        put_row(def, key, lead, columns, out);
        Ok(())
    }

    fn take(def: &TableDef, row: &Row, _loaded: &Loaded) -> Result<Self, StoreError> {
        let (_, mut columns) = split(def, row, 0);
        Self::flat_take(&mut columns)
    }

    fn flat_columns(prefix: &str, out: &mut Vec<ColumnDef>) {
        out.push(ColumnDef::of::<bool>(&format!("{prefix}present")));
        let mut inner = Vec::new();
        T::flat_columns(prefix, &mut inner);
        out.extend(inner.into_iter().map(|mut c| {
            c.nullable = true;
            c
        }));
    }

    fn flat_put(&self, out: &mut Vec<Value>) -> Result<(), StoreError> {
        match self {
            Some(inner) => {
                out.push(Value::Integer(1));
                inner.flat_put(out)
            }
            None => {
                out.push(Value::Integer(0));
                let mut inner = Vec::new();
                T::flat_columns("", &mut inner);
                out.extend(std::iter::repeat_n(Value::Null, inner.len()));
                Ok(())
            }
        }
    }

    fn flat_take(values: &mut std::vec::IntoIter<Value>) -> Result<Self, StoreError> {
        let present: bool = take_col(values, "present")?;
        if present {
            T::flat_take(values).map(Some)
        } else {
            let mut inner = Vec::new();
            T::flat_columns("", &mut inner);
            for _ in 0..inner.len() {
                next(values)?;
            }
            Ok(None)
        }
    }
}
