//! The read-only sources a scrubbed copy is made from (PLAN 22.5): a SQLite file the owner names, and a Postgres server
//! reached through a URL the hub read from the vault (`connector:<id>`, the glue tier). Both are read-only: SQLite opens
//! its file read-only with `query_only` set; Postgres runs the session `READ ONLY`, and the only statements sent are the
//! schema queries and `SELECT`s over the columns the scrubber asked for. Nothing here writes to a source.
//!
//! A source is a trait so the scrubber's rules and scan are tested over SQLite; the Postgres source is the same contract,
//! tested for its URL rules and its statements here, and against a server only on a host that runs one.

use std::path::Path;

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags};

/// A table of a source: its name and its columns in order. A copy declares the columns with their types, not their
/// constraints (a NOT NULL column would refuse the NULLs a rule leaves).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
}

/// A column: its name and its declared type (empty when the source declares none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub decl: String,
}

/// A source of rows, read in order.
pub trait Source {
    /// The tables of the source, in a stable order.
    fn tables(&mut self) -> Result<Vec<Table>, String>;
    /// Every row of `table`, each as its columns' values in order, handed to `each` one at a time.
    fn each_row(
        &mut self,
        table: &Table,
        each: &mut dyn FnMut(Vec<Value>) -> Result<(), String>,
    ) -> Result<(), String>;
}

/// Quotes an identifier for SQL: double quotes, with any double quote in it doubled.
pub fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// A SQLite file, opened read-only. A file that does not exist is an error, never a new empty database.
pub struct Sqlite {
    conn: Connection,
}

impl Sqlite {
    pub fn open(path: &Path) -> Result<Sqlite, String> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|why| {
            format!(
                "the source {} did not open read-only: {why}",
                path.display()
            )
        })?;
        conn.pragma_update(None, "query_only", true)
            .map_err(|why| format!("the source did not take query_only: {why}"))?;
        Ok(Sqlite { conn })
    }
}

impl Source for Sqlite {
    fn tables(&mut self) -> Result<Vec<Table>, String> {
        let names: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
                )
                .map_err(|why| why.to_string())?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|why| why.to_string())?;
            rows.collect::<Result<_, _>>()
                .map_err(|why| why.to_string())?
        };
        let mut tables = Vec::new();
        for name in names {
            let mut stmt = self
                .conn
                .prepare(&format!("PRAGMA table_info({})", quote(&name)))
                .map_err(|why| why.to_string())?;
            let columns = stmt
                .query_map([], |row| {
                    Ok(Column {
                        name: row.get::<_, String>(1)?,
                        decl: row.get::<_, String>(2)?,
                    })
                })
                .map_err(|why| why.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|why| why.to_string())?;
            tables.push(Table { name, columns });
        }
        Ok(tables)
    }

    fn each_row(
        &mut self,
        table: &Table,
        each: &mut dyn FnMut(Vec<Value>) -> Result<(), String>,
    ) -> Result<(), String> {
        let columns: Vec<String> = table.columns.iter().map(|c| quote(&c.name)).collect();
        let sql = format!("SELECT {} FROM {}", columns.join(", "), quote(&table.name));
        let mut stmt = self.conn.prepare(&sql).map_err(|why| why.to_string())?;
        let mut rows = stmt.query([]).map_err(|why| why.to_string())?;
        while let Some(row) = rows.next().map_err(|why| why.to_string())? {
            let mut values = Vec::with_capacity(table.columns.len());
            for index in 0..table.columns.len() {
                let value = match row.get_ref(index).map_err(|why| why.to_string())? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::Integer(n),
                    ValueRef::Real(x) => Value::Real(x),
                    ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(b) => Value::Blob(b.to_vec()),
                };
                values.push(value);
            }
            each(values)?;
        }
        Ok(())
    }
}

/// Whether a Postgres URL names only hosts this machine reaches without TLS: a loopback address or a socket. A remote
/// host needs TLS, which this build does not link, so the URL is refused before anything connects.
pub fn check_local(config: &postgres::Config) -> Result<(), String> {
    for host in config.get_hosts() {
        if let postgres::config::Host::Tcp(name) = host
            && !matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1")
        {
            return Err(format!(
                "the copy reaches a Postgres source only at a loopback host or a socket: {name} needs TLS, which this build does not link"
            ));
        }
    }
    Ok(())
}

/// A Postgres server's public schema, read through a session that is read-only.
pub struct Postgres {
    client: postgres::Client,
}

impl Postgres {
    /// Connects with a URL the hub read from the vault. The URL is never printed: its errors name the parts they refuse.
    pub fn connect(url: &str) -> Result<Postgres, String> {
        let config: postgres::Config = url
            .parse()
            .map_err(|_| "the source's URL is not a Postgres URL".to_string())?;
        check_local(&config)?;
        let mut client = config
            .connect(postgres::NoTls)
            .map_err(|_| "the Postgres source did not accept the connection".to_string())?;
        client
            .batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
            .map_err(|_| "the Postgres session did not take READ ONLY".to_string())?;
        Ok(Postgres { client })
    }
}

/// The statement that lists the public tables' columns.
pub const POSTGRES_COLUMNS: &str = "SELECT table_name, column_name FROM information_schema.columns WHERE table_schema = 'public' ORDER BY table_name, ordinal_position";

impl Source for Postgres {
    fn tables(&mut self) -> Result<Vec<Table>, String> {
        let rows = self
            .client
            .query(POSTGRES_COLUMNS, &[])
            .map_err(|_| "the Postgres schema could not be read".to_string())?;
        let mut tables: Vec<Table> = Vec::new();
        for row in rows {
            let table: String = row.get(0);
            let column: String = row.get(1);
            match tables.last_mut() {
                Some(last) if last.name == table => last.columns.push(Column {
                    name: column,
                    decl: "TEXT".into(),
                }),
                _ => tables.push(Table {
                    name: table,
                    columns: vec![Column {
                        name: column,
                        decl: "TEXT".into(),
                    }],
                }),
            }
        }
        Ok(tables)
    }

    fn each_row(
        &mut self,
        table: &Table,
        each: &mut dyn FnMut(Vec<Value>) -> Result<(), String>,
    ) -> Result<(), String> {
        // Every value is read as text: the copy declares TEXT columns, and a number is scanned as its text too.
        let columns: Vec<String> = table
            .columns
            .iter()
            .map(|c| format!("{}::text", quote(&c.name)))
            .collect();
        let sql = format!("SELECT {} FROM {}", columns.join(", "), quote(&table.name));
        let rows = self
            .client
            .query(sql.as_str(), &[])
            .map_err(|_| "the rows of a table could not be read".to_string())?;
        for row in rows {
            let mut values = Vec::with_capacity(table.columns.len());
            for index in 0..table.columns.len() {
                let text: Option<String> = row.get(index);
                values.push(text.map_or(Value::Null, Value::Text));
            }
            each(values)?;
        }
        Ok(())
    }
}
