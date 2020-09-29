#[macro_use]
extern crate slog;
use slog::{Drain,Logger};
// let us use normal logging macros
#[macro_use]
extern crate slog_scope;
use std::io;


#[macro_use] 
extern crate lazy_static;

use anyhow::{Result,Context};
use daemonize::Daemonize;

mod config;
mod sensor;
mod fridge;
mod types;
mod params;
mod web;

use riker::actors::*;

use structopt::StructOpt;

use crate::config::Config;

fn open_logfile() -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open("fridgyeast.log").context("Error opening logfile")?;
    Ok(f)
}

fn setup_logging(args: &Args) -> Result<(slog::Logger, slog_scope::GlobalLoggerGuard)> {

    let level = if args.debug {
        slog::Level::Debug
    } else {
        slog::Level::Info
    };

    fn ts(io: &mut dyn io::Write) -> io::Result<()> {
        write!(io, "{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))
    }

    let logger = if args.daemon {
        // log to file
        let decorator = slog_term::PlainSyncDecorator::new(open_logfile()?);
        let drain = slog_term::FullFormat::new(decorator).use_custom_timestamp(ts).build()
        .filter_level(level)
        // .ignore_res() because we don't want to panic in ENOSPC etc
        .ignore_res();
        slog::Logger::root(drain, o!())
    } else {
        // log to terminal
        let decorator = slog_term::PlainSyncDecorator::new(std::io::stdout());
        let drain = slog_term::FullFormat::new(decorator).use_custom_timestamp(ts).build()
        .filter_level(level)
        .ignore_res();
        slog::Logger::root(drain, o!())
    };

    let scope_guard = slog_scope::set_global_logger(logger.clone());
    slog_stdlog::init().unwrap();
    Ok((logger, scope_guard))
}

fn run(args: &Args, logger: Logger) -> Result<()> {
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

    // daemon before threads are created
    if args.daemon {
        let errfile = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("fridgyeast.log").context("Error opening logfile")?;
        let outfile = errfile.try_clone().context("Error opening err logfile")?;
        let dize = Daemonize::new()
        // param file is relative for now
        .working_directory(std::env::current_dir()?)
        .pid_file("fridgyeast.pid")
        .stdout(outfile)
        .stderr(errfile)
        .exit_action(|| {
            //daemon_channel.read();
            println!("Running as a daemon");
        });

        dize.start()
        .context("Failed daemonising")?;
        info!("Running daemonised, pid {}", std::process::id());
    }

    // start actor system
    let sys = SystemBuilder::new()
    .name("fridgyeast")
    .log(logger.clone())
    .create().unwrap();
    let fridge = sys.actor_of_args::<fridge::Fridge, _>("fridge", &cf)?;


    // webserver waits listening forever
    let w = web::listen_http(&sys, fridge.clone(), &cf);
    async_std::task::block_on(w).context("https listener failed")
}

#[derive(Debug, StructOpt)]
#[structopt(name = "Wort Temperature", about = "Matt Johnston 2020 matt@ucc.asn.au")]
struct Args {
    /// Replace any existing running instance
    #[structopt(long)]
    new: bool,

    /// Run in background
    #[structopt(short = "D", long)]
    daemon: bool,

    #[structopt(short, long)]
    debug: bool,

    /// Use fake sensors etc
    #[structopt(long)]
    test: bool,

    /// Skip initial fridge wait
    #[structopt(long)]
    nowait: bool,

    /// Read real sensors but don't touch the fridge
    #[structopt(long)]
    dryrun: bool,

    /// Print default config (customise in local.toml)
    #[structopt(long)]
    exampleconfig: bool,

    /// Config file
    #[structopt(short = "c", long, default_value = "local.toml")]
    config: String,
}

fn handle_args() -> Args {
    let mut args = Args::from_args();

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
    let (logger, _scope_guard) = setup_logging(&args)?;
    if let Err(e) = run(&args, logger) {
        println!("Failed starting: {:?}", e);
        if args.daemon {
            crit!("Failed starting: {:?}", e);
            crit!("Exited.");
        }
    }
    Ok(())
}
