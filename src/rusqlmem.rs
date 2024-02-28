#[allow(unused_imports)]
use {
    log::{debug, error, info, warn,log},
    anyhow::{Result,Context,bail,anyhow},
};

use std::sync::Arc;

use rusqlite::{Connection,backup::Backup};
use rand::Rng;

use async_std::task::{block_on,spawn_blocking};
use async_std::sync::Mutex as AsyncMutex;

use base64::Engine;

/// Provides a SQLite in-memory database that is automatically loaded/saved to a
/// backing database on disk.
pub struct RusqlMem {
	memdb: std::sync::Mutex<Connection>,
	filedb: Arc<AsyncMutex<Connection>>,
	/// For error messages
	file_path: std::path::PathBuf,
	/// Sqlite path to open the in-memory db
	mem_path: std::path::PathBuf,
}

impl RusqlMem {
	/// Takes the path to a SQLite file. If the file does not exist it will
	/// be created and populated calling `init_schema`. That callback
	/// should not commit the transaction, `RusqlMem` will handle it.
	pub fn new(p: &std::path::Path, init_schema: fn(&mut rusqlite::Transaction)
		-> Result<()>) -> Result<Self> {
		let nonce = base64::engine::general_purpose::URL_SAFE.encode(rand::thread_rng().gen::<[u8; 18]>());
		// The in-memory database has a unique name within this RusqlMem instance
		let mem_path = format!("file::{}?mode=memory&cache=shared", nonce).into();
		let memdb = Connection::open(&mem_path)?;

		// try opening existing first ...
		let filedb = match Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE) {
			Ok(f) => f,
			Err(_) => {
				// ... or create it and initialise the schema
				let mut f = Connection::open(p)?;
				let mut t = f.transaction()?;
				init_schema(&mut t)?;
				t.commit()?;
				f
			}
		};

		let r = Self {
			memdb: std::sync::Mutex::new(memdb),
			filedb: Arc::new(AsyncMutex::new(filedb)),
			file_path: p.into(),
			mem_path,
		};
		r.load_to_mem()?;
		Ok(r)
	}

	pub fn db(&self) -> std::sync::MutexGuard<Connection> {
		self.memdb.lock().unwrap()
	}

	/// Flushes the contents of the in-memory database to disk.
	/// Returns `Ok(false)` if flush is already in progress, 
	/// `Ok(true)` or `Err` otherwise.
	pub async fn flush(&self) -> Result<bool> {
		let mut f = match self.filedb.clone().try_lock_arc() {
			Some(f) => f,
			None => return Ok(false),
		};
		// open a new connection to the same in-memory db
		let m = Connection::open(&self.mem_path)?;
		// let sqlite flush on a thread
		let r = spawn_blocking(move || {
			let b = Backup::new(&m, &mut f)?;
			// 32 * 4k pages at a time
			b.run_to_completion(32, std::time::Duration::ZERO, None)?;
			Ok(true)
		}).await;
		// fiddly syntax, otherwise compiler can't guess the Error type
		if r.is_ok() {
			debug!("Flushed {:?}", self.file_path);
		}
		r
	}

	fn load_to_mem(&self) -> Result<()> {
		let f = &*block_on(self.filedb.lock());
		let mut m = self.db();
		let b = Backup::new(f, &mut m)?;
		b.step(-1)?;
		Ok(())
	}
}

