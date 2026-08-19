//! Minimal read-only wrapper around the system SQLite.
//!
//! Ported from the upstream Windows monitor (`src/winsqlite.rs`), which imports
//! the `winsqlite3.dll` that ships with Windows. The Linux counterpart is
//! `libsqlite3.so.0`, resolved with `dlopen` at run time rather than linked:
//! that keeps the build free of a `libsqlite3-dev` requirement and lets the
//! Cursor provider degrade to "no credentials" on a box without SQLite instead
//! of refusing to start.
//!
//! Deliberately narrow — the monitor only needs one text value out of one
//! application-owned database.

use std::ffi::{c_char, c_int, c_uchar, c_void, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr;
use std::sync::OnceLock;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_OPEN_READ_ONLY: c_int = 0x0000_0001;
const RTLD_NOW: c_int = 2;
const BUSY_TIMEOUT_MS: c_int = 1_000;

/// Tried in order; the versioned name is what a runtime-only install provides.
const LIBRARY_NAMES: [&str; 2] = ["libsqlite3.so.0", "libsqlite3.so"];

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _private: [u8; 0],
}

extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type OpenV2 = unsafe extern "C" fn(*const c_char, *mut *mut Sqlite3, c_int, *const c_char) -> c_int;
type Close = unsafe extern "C" fn(*mut Sqlite3) -> c_int;
type ErrMsg = unsafe extern "C" fn(*mut Sqlite3) -> *const c_char;
type BusyTimeout = unsafe extern "C" fn(*mut Sqlite3, c_int) -> c_int;
type PrepareV2 = unsafe extern "C" fn(
    *mut Sqlite3,
    *const c_char,
    c_int,
    *mut *mut Sqlite3Stmt,
    *mut *const c_char,
) -> c_int;
type BindText = unsafe extern "C" fn(
    *mut Sqlite3Stmt,
    c_int,
    *const c_char,
    c_int,
    Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int;
type Step = unsafe extern "C" fn(*mut Sqlite3Stmt) -> c_int;
type ColumnText = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> *const c_uchar;
type ColumnBytes = unsafe extern "C" fn(*mut Sqlite3Stmt, c_int) -> c_int;
type Finalize = unsafe extern "C" fn(*mut Sqlite3Stmt) -> c_int;

struct Library {
    open_v2: OpenV2,
    close: Close,
    errmsg: ErrMsg,
    busy_timeout: BusyTimeout,
    prepare_v2: PrepareV2,
    bind_text: BindText,
    step: Step,
    column_text: ColumnText,
    column_bytes: ColumnBytes,
    finalize: Finalize,
}

// The handle is never freed and every entry point is a stateless C function, so
// the resolved table is safe to share between threads.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Library {
    fn load() -> Result<&'static Self, Error> {
        static LIBRARY: OnceLock<Option<Library>> = OnceLock::new();
        LIBRARY
            .get_or_init(Self::resolve)
            .as_ref()
            .ok_or_else(|| Error("system SQLite (libsqlite3.so.0) is not available".into()))
    }

    fn resolve() -> Option<Self> {
        let handle = LIBRARY_NAMES.iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            // SAFETY: `name` is a valid NUL-terminated string for the call.
            let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
            (!handle.is_null()).then_some(handle)
        })?;

        // SAFETY: each symbol is looked up in a live handle and transmuted to
        // the signature SQLite documents for it.
        unsafe {
            Some(Self {
                open_v2: std::mem::transmute::<*mut c_void, OpenV2>(symbol(
                    handle,
                    "sqlite3_open_v2",
                )?),
                close: std::mem::transmute::<*mut c_void, Close>(symbol(handle, "sqlite3_close")?),
                errmsg: std::mem::transmute::<*mut c_void, ErrMsg>(symbol(
                    handle,
                    "sqlite3_errmsg",
                )?),
                busy_timeout: std::mem::transmute::<*mut c_void, BusyTimeout>(symbol(
                    handle,
                    "sqlite3_busy_timeout",
                )?),
                prepare_v2: std::mem::transmute::<*mut c_void, PrepareV2>(symbol(
                    handle,
                    "sqlite3_prepare_v2",
                )?),
                bind_text: std::mem::transmute::<*mut c_void, BindText>(symbol(
                    handle,
                    "sqlite3_bind_text",
                )?),
                step: std::mem::transmute::<*mut c_void, Step>(symbol(handle, "sqlite3_step")?),
                column_text: std::mem::transmute::<*mut c_void, ColumnText>(symbol(
                    handle,
                    "sqlite3_column_text",
                )?),
                column_bytes: std::mem::transmute::<*mut c_void, ColumnBytes>(symbol(
                    handle,
                    "sqlite3_column_bytes",
                )?),
                finalize: std::mem::transmute::<*mut c_void, Finalize>(symbol(
                    handle,
                    "sqlite3_finalize",
                )?),
            })
        }
    }
}

fn symbol(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
    let name = CString::new(name).ok()?;
    // SAFETY: `handle` came from a successful `dlopen` and is still open.
    let symbol = unsafe { dlsym(handle, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

struct Connection {
    library: &'static Library,
    raw: *mut Sqlite3,
}

impl Connection {
    fn open_read_only(path: &Path) -> Result<Self, Error> {
        let library = Library::load()?;
        let filename = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| Error("SQLite database path contains a NUL byte".into()))?;

        let mut raw = ptr::null_mut();
        // SAFETY: `filename` outlives the call and `raw` is a valid out-pointer.
        let result = unsafe {
            (library.open_v2)(
                filename.as_ptr(),
                &mut raw,
                SQLITE_OPEN_READ_ONLY,
                ptr::null(),
            )
        };

        if result != SQLITE_OK || raw.is_null() {
            let error = error_message(library, raw, "unable to open SQLite database", result);
            if !raw.is_null() {
                // SAFETY: a non-null handle must be closed even on failure.
                unsafe { (library.close)(raw) };
            }
            return Err(error);
        }

        let connection = Self { library, raw };
        // SAFETY: `raw` is a live connection.
        let result = unsafe { (library.busy_timeout)(connection.raw, BUSY_TIMEOUT_MS) };
        if result != SQLITE_OK {
            return Err(error_message(
                library,
                connection.raw,
                "unable to set SQLite busy timeout",
                result,
            ));
        }
        Ok(connection)
    }

    /// Run a single-parameter query and return the first row's first column.
    fn query_optional_text(&self, sql: &str, parameter: &str) -> Result<Option<String>, Error> {
        let sql_text =
            CString::new(sql).map_err(|_| Error("SQLite statement contains a NUL byte".into()))?;
        let parameter_text = CString::new(parameter)
            .map_err(|_| Error("SQLite parameter contains a NUL byte".into()))?;

        let mut statement = ptr::null_mut();
        // SAFETY: `sql_text` outlives the call and `statement` is a valid
        // out-pointer.
        let result = unsafe {
            (self.library.prepare_v2)(
                self.raw,
                sql_text.as_ptr(),
                -1,
                &mut statement,
                ptr::null_mut(),
            )
        };
        if result != SQLITE_OK || statement.is_null() {
            return Err(error_message(
                self.library,
                self.raw,
                "unable to prepare SQLite statement",
                result,
            ));
        }
        let statement = Statement {
            library: self.library,
            raw: statement,
        };

        // SAFETY: `parameter_text` outlives the statement, so SQLite may borrow
        // it (the `None` destructor is SQLITE_STATIC).
        let result = unsafe {
            (self.library.bind_text)(
                statement.raw,
                1,
                parameter_text.as_ptr(),
                parameter_text.as_bytes().len() as c_int,
                None,
            )
        };
        if result != SQLITE_OK {
            return Err(error_message(
                self.library,
                self.raw,
                "unable to bind SQLite parameter",
                result,
            ));
        }

        // SAFETY: `statement` is prepared and not yet finalized.
        let result = unsafe { (self.library.step)(statement.raw) };
        if result != SQLITE_ROW {
            return Ok(None);
        }

        // SAFETY: the step above returned a row, so column 0 is readable. The
        // text is copied out before the statement is finalized.
        unsafe {
            let text = (self.library.column_text)(statement.raw, 0);
            if text.is_null() {
                return Ok(None);
            }
            let length = (self.library.column_bytes)(statement.raw, 0).max(0) as usize;
            let bytes = std::slice::from_raw_parts(text, length);
            Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: `raw` is a live connection created by `open_read_only`.
        unsafe { (self.library.close)(self.raw) };
    }
}

struct Statement {
    library: &'static Library,
    raw: *mut Sqlite3Stmt,
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: `raw` is a prepared statement that has not been finalized.
        unsafe { (self.library.finalize)(self.raw) };
    }
}

fn error_message(library: &Library, database: *mut Sqlite3, context: &str, code: c_int) -> Error {
    if database.is_null() {
        return Error(format!("{context} (code {code})"));
    }
    // SAFETY: `database` is a live handle, and `sqlite3_errmsg` returns a
    // NUL-terminated string owned by SQLite.
    let message = unsafe {
        let raw = (library.errmsg)(database);
        if raw.is_null() {
            String::new()
        } else {
            CStr::from_ptr(raw).to_string_lossy().into_owned()
        }
    };
    if message.is_empty() {
        Error(format!("{context} (code {code})"))
    } else {
        Error(format!("{context}: {message} (code {code})"))
    }
}

/// Read one text value out of a read-only SQLite database.
pub fn query_optional_text(
    path: &Path,
    sql: &str,
    parameter: &str,
) -> Result<Option<String>, Error> {
    Connection::open_read_only(path)?.query_optional_text(sql, parameter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture is a real Cursor-shaped `state.vscdb`, so this exercises the
    /// whole path: `dlopen`, prepare, bind, step, and the text copy-out.
    fn fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("cursor-state.vscdb")
    }

    #[test]
    fn reads_a_bound_value_out_of_a_real_database() {
        let value = query_optional_text(
            &fixture(),
            "SELECT value FROM ItemTable WHERE key = ?1",
            "cursorAuth/accessToken",
        )
        .expect("system SQLite should be available for this test");
        assert_eq!(
            value.as_deref(),
            Some("header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature")
        );
    }

    #[test]
    fn a_query_that_matches_nothing_is_not_an_error() {
        let value = query_optional_text(
            &fixture(),
            "SELECT value FROM ItemTable WHERE key = ?1",
            "cursorAuth/absent",
        )
        .unwrap();
        assert_eq!(value, None);
    }

    #[test]
    fn a_broken_statement_reports_the_sqlite_error() {
        let error = query_optional_text(&fixture(), "SELECT nope FROM Missing WHERE key = ?1", "x")
            .unwrap_err();
        assert!(error.to_string().contains("unable to prepare"));
    }

    #[test]
    fn missing_databases_report_an_error_rather_than_panicking() {
        let result = query_optional_text(
            Path::new("/nonexistent/state.vscdb"),
            "SELECT value FROM ItemTable WHERE key = ?1",
            "key",
        );
        match result {
            Err(error) => assert!(!error.to_string().is_empty()),
            Ok(value) => panic!("expected an error, got {value:?}"),
        }
    }

    #[test]
    fn paths_with_interior_nul_bytes_are_rejected() {
        // Only reachable when SQLite is present; otherwise the load error wins,
        // which is also a rejection.
        let path = std::path::PathBuf::from(unsafe {
            String::from_utf8_unchecked(b"/tmp/a\0b.db".to_vec())
        });
        assert!(query_optional_text(&path, "SELECT 1", "x").is_err());
    }
}
