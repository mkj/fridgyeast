use std::collections::HashMap;
use std::time::{Duration,Instant};
use std::error::Error;
use std::fmt;
use std::io;
use std::cmp;
use std::cell::Cell;

use std;

use serde_json;

#[derive(Debug,Clone)]
pub struct Readings {
    pub temps: HashMap<String, f32>,
}

impl Readings {
    pub fn new() -> Readings {
        Readings {
            temps: HashMap::new(),
        }
    }

    pub fn add(&mut self, name: &str, v: f32) {
        self.temps.insert(name.to_string(), v);
    }

    pub fn get_temp(&self, name: &str) -> Option<f32> {
        self.temps.get(name).map(|f| *f)
    }
}

#[derive(Debug)]
pub enum TemplogErrorKind {
    None,
    Io(io::Error),
    ParseFloat(std::num::ParseFloatError),
    SerdeJson(serde_json::Error),
}

#[derive(Debug)]
pub struct TemplogError {
    msg: String,
    desc: String,
    kind: TemplogErrorKind,
}

impl Error for TemplogError {
    fn description(&self) -> &str { 
        &self.desc
    }

    fn cause(&self) -> Option<&dyn Error> { 
        match self.kind {
            TemplogErrorKind::None => None,
            TemplogErrorKind::Io(ref e) => Some(e),
            TemplogErrorKind::ParseFloat(ref e) => Some(e),
            TemplogErrorKind::SerdeJson(ref e) => Some(e),
        }
    }

}

impl fmt::Display for TemplogError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.kind_str())?;
        if !self.msg.is_empty() {
            write!(f, ": {}", self.msg)?;
        }
        match self.kind {
            TemplogErrorKind::None => Ok(()),
            TemplogErrorKind::Io(ref e) => write!(f, ": {}", e),
            TemplogErrorKind::SerdeJson(ref e) => write!(f, ": {}", e),
            TemplogErrorKind::ParseFloat(ref e) => write!(f, ": {}", e),
        }?;
        Ok(())
    }
}

impl TemplogError {
    pub fn new(msg: &str) -> Self {
        TemplogError::new_kind(msg, TemplogErrorKind::None)
    }

    pub fn new_io(msg: &str, e: io::Error) -> Self {
        TemplogError::new_kind(msg, TemplogErrorKind::Io(e))
    }

    pub fn new_parse_float(msg: &str, e: std::num::ParseFloatError) -> Self {
        TemplogError::new_kind(msg, TemplogErrorKind::ParseFloat(e))
    }

    pub fn new_serde_json(msg: &str, e: serde_json::Error) -> Self {
        TemplogError::new_kind(msg, TemplogErrorKind::SerdeJson(e))
    }

    pub fn kind(&self) -> &TemplogErrorKind {
        return &self.kind;
    }

    fn new_kind(msg: &str, k: TemplogErrorKind) -> Self {
        let mut s = TemplogError { 
            msg: msg.to_string(),
            desc: String::new(),
            kind: k,
        };
        s.desc = if s.msg.is_empty() {
            s.kind_str().to_string()
        } else {
            format!("{}: {}", s.kind_str(), s.msg)
        };
        s
    }

    fn kind_str(&self) -> &str {
        match self.kind {
            TemplogErrorKind::None => "Templog Error",
            TemplogErrorKind::Io(_) => "Templog IO error",
            TemplogErrorKind::SerdeJson(_) => "Templog Json decode error",
            TemplogErrorKind::ParseFloat(_) => "Templog parse error",
        }
    }
}

impl From<io::Error> for TemplogError {
    fn from(e: io::Error) -> Self {
        TemplogError::new_io("", e)
    }
}

impl From<std::num::ParseFloatError> for TemplogError {
    fn from(e: std::num::ParseFloatError) -> Self {
        TemplogError::new_parse_float("", e)
    }
}

impl From<serde_json::Error> for TemplogError {
    fn from(e: serde_json::Error) -> Self {
        TemplogError::new_serde_json("", e)
    }
}

/// Call closures with a rate limit. Useful for log message ratelimiting
#[derive(Clone)]
pub struct NotTooOften {
    last: Cell<Instant>,
    limit: Duration,
}

impl NotTooOften {
    pub fn new(limit_secs: u64) -> Self {
        NotTooOften {
            limit: Duration::new(limit_secs, 0),
            // XXX why +1?
            last: Cell::new(Instant::now() - Duration::new(limit_secs+1, 0)),
        }
    }

    pub fn and_then<F, U>(&self, op: F) -> Option<U>
        where F: Fn() -> U {
        let now = Instant::now();
        if now - self.last.get() > self.limit {
            self.last.set(now);
            Some(op())
        } else {
            None
        }
    }
}

struct Period {
    start: Instant,
    end: Option<Instant>,
}

pub struct StepIntegrator {
    on_periods: Vec<Period>,
    limit: Duration,
}

impl StepIntegrator {
    pub fn new(limit: Duration) -> Self {
        StepIntegrator {
            on_periods: Vec::new(),
            limit: limit,
        }
    }

    pub fn turn(&mut self, on: bool) {
        self.trim();

        if self.on_periods.is_empty() {
            self.on_periods.push( Period { start: Instant::now(), end: None });
            return;
        }

        let current_on = self.on_periods.last().unwrap().end.is_none();
        if on == current_on {
            // state is unchanged
            return;
        }

        if on {
            self.on_periods.push( Period { start: Instant::now(), end: None });
        } else {
            self.on_periods.last_mut().unwrap().end = Some(Instant::now());
        }
    }

    pub fn set_limit(&mut self, limit: Duration) {
        self.limit = limit;
        self.trim();
    }

    pub fn integrate(&self) -> Duration {
        let durs = self.on_periods.iter().map(|p| {
            let end = p.end.unwrap_or_else(|| Instant::now());
            end - p.start
        });
        durs.sum()
    }

    fn trim(&mut self) {
        let begin = Instant::now() - self.limit;

        self.on_periods.retain(|p| {
            // remove expired endtimes
            if let Some(e) = p.end {
                e >= begin && e != p.start
            } else {
                true
            }
        });

        for p in &mut self.on_periods {
            p.start = cmp::max(p.start, begin);
        }
    }
}

