#[allow(unused_imports)]
use {
    log::{debug, error, info, warn,log},
    anyhow::{Result,Context,bail,anyhow},
};

use std::{alloc::System, time::{Duration,Instant,SystemTime}};
use std::sync::Arc;
use std::path::Path;

use rand::Rng;

use async_std::task::{block_on,spawn_blocking};
use async_std::sync::Mutex as AsyncMutex;

use rusqlite::{Connection,backup::Backup};

pub struct RusqlMem {
	memdb: std::sync::Mutex<Connection>,
	filedb: Arc<AsyncMutex<Connection>>,
	/// Path on disk for error messages
	file_path: std::path::PathBuf,
	/// Sqlite path to open the in-memory db
	mem_path: std::path::PathBuf,
}

impl RusqlMem {
	pub fn new(p: &Path, init_schema: fn(&mut rusqlite::Transaction) -> Result<()>) -> Result<Self> {
		// 144 bit nonce keeps it private to this RusqlMem instance within the program
		let nonce = base64::encode_config(rand::thread_rng().gen::<[u8; 18]>(),
			base64::URL_SAFE);
		let mem_path = format!("file::{}?mode=memory&cache=shared", nonce).into();
		let mut memdb = Connection::open(&mem_path)?;

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

	/// Returns Ok(false) if flush is already in progress
	pub async fn flush(&self) -> Result<bool> {
		let mut f = match self.filedb.clone().try_lock_arc() {
			Some(f) => f,
			None => return Ok(false),
		};
		// open a new connection to the same in-memory db
		let m = Connection::open(&self.mem_path)?;
		// let sqlite flush on a thread
		spawn_blocking(move || {
			let b = Backup::new(&m, &mut f)?;
			// 32 * 4k pages at a time
			b.run_to_completion(32, Duration::ZERO, None)?;
			Ok(true)
		}).await
	}

	fn load_to_mem(&self) -> Result<()> {
		let f = &*block_on(self.filedb.lock());
		let mut m = self.db();
		let b = Backup::new(&f, &mut m)?;
		b.step(-1)?;
		Ok(())
	}
}

