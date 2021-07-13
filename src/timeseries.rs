#[allow(unused_imports)]
use {
    log::{debug, error, info, warn,log},
    anyhow::{Result,Context,bail,anyhow},
};

use std::{alloc::System, time::{Duration,Instant,SystemTime}};
use std::sync::Mutex as SyncMutex;
use std::sync::Arc;
use std::path::Path;

use act_zero::*;
use act_zero::runtimes::async_std::Timer;
use act_zero::timer::Tick;

use async_trait::async_trait;

use async_std::task::{block_on,spawn_blocking};
use async_std::sync::Mutex as AsyncMutex;

use rusqlite::{Connection,OptionalExtension,params,backup::Backup};


pub struct TimeSeries {
	quant_secs: u64,
	history: Duration,
	memdb: Arc<SyncMutex<Connection>>,
	filedb: Arc<AsyncMutex<Connection>>,
	dbpath: std::path::PathBuf,

    prune_timer: Timer,
    flush_timer: Timer,
}

impl TimeSeries {
	pub fn new(p: &Path, quant_secs: u64, history: Duration) -> Result<Self> {
		let mut memdb = Connection::open_in_memory()?;
		let mut filedb = Connection::open(p)?;
		Self::init_schema(&mut memdb)?;
		Self::init_schema(&mut filedb)?;

		let ts = TimeSeries {
			quant_secs,
			history,
			memdb: Arc::new(SyncMutex::new(memdb)),
			filedb: Arc::new(AsyncMutex::new(filedb)),
			dbpath: p.into(),
			prune_timer: Timer::default(),
			flush_timer: Timer::default(),
		};
		ts.load_to_mem()?;
		Ok(ts)
	}

	pub fn add(&self, time: SystemTime, value: f32) -> Result<()> {
		let mut conn = self.mdb()?;
		let t = conn.transaction()?;
		let dif = Self::time_to_int(time)?;
		let quant_time = dif - (dif % self.quant_secs);
		let (oldval, count): (f32, u32) = t.query_row("select value, count from points where time = ?", [quant_time],
			|r| Ok((r.get(0)?, r.get(1)?)))
			.optional()?
			.unwrap_or((0.0, 0));
		// scale by existing accumulated values
		let c = count as f32;
		let v = oldval*c/(c+1.0) + value/(c+1.0);
		if count > 0 {
			t.execute("delete from points where time = ?", [&quant_time])?;
		}
		t.execute("insert into points values (?, ?, ?)", params![quant_time, v, count+1])?;
		t.commit()?;
		Ok(())
	}

	pub fn get(&self) -> Result<Vec<(SystemTime, f32)>> {
		self.mdb()?
		.prepare("select time, value from points where time >= ?")?
		.query_map(params![self.earliest()], |r| {
			let t: SystemTime = Self::int_to_time(r.get(0)?);
			let v: f32 = r.get(1)?;
			Ok((t, v))
		})?
		.map(|r| r.context("SQL query"))
		.collect()
	}

	pub async fn flush(&self) -> Result<()> {
		let mut f = match self.filedb.clone().try_lock_arc() {
			Some(f) => f,
			None => bail!("{} was locked, not flushing", self.dbpath.display()),
		};
		let md = self.memdb.clone();
		// let sqlite flush on a thread
		spawn_blocking(move || {
			// Have to rewrite the error since closure returns Result: 'static
			let m = md.lock().map_err(|e| anyhow!("Bad memdb lock{}", e))?;
			let b = Backup::new(&m, &mut f)?;
			b.step(-1)?;
			Ok(())
		}).await
	}

	fn time_to_int(time: SystemTime) -> Result<u64> {
		Ok(time.duration_since(SystemTime::UNIX_EPOCH).context("Time before 1970")?.as_secs())
	}

	fn int_to_time(i: u64) -> SystemTime {
		SystemTime::UNIX_EPOCH + Duration::from_secs(i)
	}

	fn mdb(&self) -> Result<std::sync::MutexGuard<Connection>> {
		// we want to discard the context in the Result since MutexGuard isn't Send
		self.memdb.lock().map_err(|e| anyhow!("Bad memdb lock{}", e))
	}


	fn prune(&self) -> Result<()> {
		self.mdb()?.execute("delete from points where time < ?", params![self.earliest()])
		.map(|_| ())
		.context("pruning")
	}

	fn earliest(&self) -> u64 {
		Self::time_to_int(SystemTime::now() - self.history)
		.unwrap_or(0)
	}

	fn load_to_mem(&self) -> Result<()> {
		let f = &*block_on(self.filedb.lock());
		let mut m = self.mdb()?;
		let b = Backup::new(&f, &mut m)?;
		b.step(-1)
		.map(|_| ())
		.context("loading db from disk")
	}

	fn init_schema(conn: &mut Connection) -> Result<()> {
		const CURR_VERSION: u32 = 11;

		let t = conn.transaction()?;
		let vers: u32 = t.query_row("select * from pragma_user_version", [], |r| r.get(0))?;
		match vers {
			CURR_VERSION => return Ok(()),
			0 => t.pragma_update(None, "user_version", &CURR_VERSION)?,
			_ => bail!("Old DB version {} expected {}", vers, CURR_VERSION)
		};

		t.execute("create table points (time, value, count)", [])?;
		t.execute("create index points_time on points (time)", [])?;
		t.commit()?;
		Ok(())
	}
}

const FLUSH_INTERVAL: Duration = Duration::from_secs(60 * 15);

#[async_trait]
impl Actor for TimeSeries {
    async fn started(&mut self, addr: Addr<Self>) -> ActorResult<()> {
        self.prune_timer.set_interval_weak(addr.downgrade(), self.history * 2);
        self.flush_timer.set_interval_weak(addr.downgrade(), FLUSH_INTERVAL);
        Produces::ok(())
    }

    async fn error(&mut self, error: ActorError) -> bool {
        warn!("Ignoring error from TimeSeries actor: {:?}", error);
        false
    }
}

#[async_trait]
impl Tick for TimeSeries {
    async fn tick(&mut self) -> ActorResult<()> {
        if self.prune_timer.tick() {
            if let Err(e) = self.prune() {
            	warn!("{}", e)
            }
        }
        if self.flush_timer.tick() {
            if let Err(e) = self.flush().await {
            	warn!("{}", e)
            }
        }
        Produces::ok(())
    }
}

#[cfg(test)]
mod tests {
use super::*;

#[test]
fn new_timeseries() -> Result<()> {
	let t = TimeSeries::new(Path::new("ff.db"), 60, Duration::from_secs(60*60*24*3))?;
	t.add(SystemTime::now(), 3.2f32)?;
	block_on(t.flush())?;
	Ok(())
}

}
