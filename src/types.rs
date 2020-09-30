/// Various helpers

use std::collections::HashMap;
use std::time::{Duration,Instant};

use std::cmp;
use std::cell::Cell;

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
            limit,
        }
    }

    pub fn turn(&mut self, on: bool) {
        self.trim();

        let currently_on = match self.on_periods.last() {
            Some(l) => l.end.is_none(),
            None => {
                self.on_periods.push( Period { start: Instant::now(), end: None });
                return;
            },

        };

        if on == currently_on {
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

