#[macro_use]
extern crate slog;
use async_std::io::ReadExt;
use futures::FutureExt;
use slog::{Drain,Logger};
// let us use normal logging macros
#[macro_use]
extern crate slog_scope;
use std::io;

use futures::select;

#[macro_use] 
extern crate lazy_static;

use anyhow::{Result,Context,bail};

mod config;
mod sensor;
mod fridge;
mod types;
mod params;
mod web;

use riker::actors::*;

use crate::config::Config;

fn open_logfile() -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open("fridgyeast.log").context("Error opening logfile")?;
    Ok(f)
}

/// Set up logging, either to a logfile or terminal.
/// When `also_term` is set logging will always be duplicated to a terminal.
///
/// Beware that this will leak a global logger, do not call many times with `global` set.
fn setup_logging(debug: bool, to_term: bool, to_file: bool, global: bool) -> Result<slog::Logger> {

    let level = if debug {
        slog::Level::Debug
    } else {
        slog::Level::Info
    };

    fn ts(io: &mut dyn io::Write) -> io::Result<()> {
        write!(io, "{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"))
    }

    let term_drain = if to_term {
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        Some(slog_term::FullFormat::new(decorator).use_custom_timestamp(ts).build())
    } else {
        None
    };

    let file_drain = if to_file {
        let decorator = slog_term::PlainSyncDecorator::new(open_logfile()?);
        Some(slog_term::FullFormat::new(decorator).use_custom_timestamp(ts).build())
    } else {
        None
    };

    // .ignore_res() because we don't want to panic in ENOSPC etc
    let logger = match (file_drain, term_drain) {
        (Some(f), Some(t)) =>  {
            let drain = slog::Duplicate(t, f)
            .filter_level(level)
            .ignore_res();
            slog::Logger::root(drain, o!())
        },
        (Some(f), None) => {
            let drain = f.filter_level(level)
            .ignore_res();
            slog::Logger::root(drain, o!())
        }
        (None, Some(t)) => {
            let drain = t.filter_level(level)
            .ignore_res();
            slog::Logger::root(drain, o!())
        }
        _default => bail!("Logger needs file or term")
    };

    if global {
        let scope_guard = slog_scope::set_global_logger(logger.clone());
        slog_stdlog::init().ok();
        Box::leak(Box::new(scope_guard));
    }

    Ok(logger)
}

async fn wait_exit() -> Result<()> {
    // see https://github.com/stjepang/async-io/blob/master/examples/unix-signal.rs
    use async_io::Async;
    use std::os::unix::net::UnixStream;

    let (w, mut r) = Async::<UnixStream>::pair()?;
    signal_hook::pipe::register(signal_hook::SIGINT, w.get_ref().try_clone()?)?;
    signal_hook::pipe::register(signal_hook::SIGTERM, w.get_ref().try_clone()?)?;
    debug!("Waiting for exit signal");

    // Receive a byte that indicates the Ctrl-C signal occurred.
    r.read_exact(&mut [0]).await?;
    info!("Exiting with signal");
    Ok(())
}

fn run(args: &Args, logger: &Logger) -> Result<()> {
    // load config, make it static
    let mut cf = config::Config::load(&args.config)?;
    cf.debug = args.debug;
    cf.testmode = args.test;
    cf.nowait = args.nowait;
    cf.dryrun = args.dryrun;
    let cf : &'static Config = Box::leak(Box::new(cf));

    info!("Started fridgyeast. pid {}", std::process::id());
    debug!("Running in debug mode");
    if cf.testmode {
        info!("Running in test mode")
    }
    if cf.dryrun {
        info!("Running in dry run mode")
    }

    // start actor system
    let sys = SystemBuilder::new()
    .name("fridgyeast")
    .log(logger.clone())
    .create().unwrap();
    let fridge = sys.actor_of_args::<fridge::Fridge, _>("fridge", &cf)?;

    let webserver = web::listen_http(&sys, fridge.clone(), &cf);

    let webserver = webserver.fuse();
    let exit = wait_exit().fuse();
    futures::pin_mut!(webserver, exit);

    let allwaiting = async {
        select! {
            w = webserver => w,
            _ = exit => Ok(()),
        }
    };

    let res = async_std::task::block_on(allwaiting);

    async_std::task::block_on(sys.shutdown())?;

    res
}

#[derive(argh::FromArgs)]
/** Wort Temperature
Matt Johnston 2020 matt@ucc.asn.au */
struct Args {
    #[argh(switch, short='v')]
    /// verbose debug logging
    debug: bool,

    /// use fake sensors etc
    #[argh(switch)]
    test: bool,

    /// skip initial fridge wait
    #[argh(switch)]
    nowait: bool,

    /// read real sensors but don't touch the fridge
    #[argh(switch, short='n')]
    dryrun: bool,

    /// print default config (customise in local.toml)
    #[argh(switch, short='e')]
    exampleconfig: bool,

    /// config file
    #[argh(option, short = 'c', default = "\"local.toml\".to_string()")]
    config: String,
}

fn handle_args() -> Args {
    let mut args: Args = argh::from_env();

    if args.exampleconfig {
        println!("{}", config::Config::example_toml());
        std::process::exit(0);
    }

    if cfg!(not(target_os = "linux")) {
        info!("Forcing --test for non-Linux");
        args.test = true;
    }

    args
}

fn main() -> Result<()> {
    let args = handle_args();
    let logger = setup_logging(args.debug, true, true, true)?;
    if let Err(e) = run(&args, &logger) {
        println!("Failed starting: {:?}", e);
    }
    Ok(())
}
