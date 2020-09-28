#[macro_use]
extern crate slog;
#[macro_use]
extern crate slog_scope;
extern crate slog_term;

#[macro_use] 
extern crate lazy_static;


use crate::slog::Drain;
use anyhow::Result;

mod config;
mod sensor;
mod fridge;
mod types;
mod params;
mod web;

use riker::actors::*;

use structopt::StructOpt;

use crate::config::Config;

fn run(args: Opt) -> Result<()> {

    let mut cf = config::Config::load(&args.config)?;
    cf.debug = args.debug;
    cf.testmode = args.test;
    cf.nowait = args.nowait;
    cf.dryrun = args.dryrun;

    if cf.testmode {
        info!("Running in test mode")
    }
    if cf.dryrun {
        info!("Running in dry run mode")
    }

    // make it static
    let cf : &'static Config = Box::leak(Box::new(cf));

    let mut riker_cfg = riker::load_config();
    // default seems to be no filter, we'll override it
    if riker_cfg.get_array("log.filter").is_err() {
        //riker_cfg.set("log.filter", vec!("rustls::")).unwrap();
    }

    if riker_cfg.get::<String>("log.level").is_err() {
        if cf.debug {
            riker_cfg.set("log.level", "debug").unwrap();
            tide::log::with_level(tide::log::LevelFilter::Trace);
        } else {
            riker_cfg.set("log.level", "info").unwrap();
        }
    }

    // Start everything up
    let sys = SystemBuilder::new()
    .name("fridgeyeast")
    .log(make_logger())
    .cfg(riker_cfg)
    .create().unwrap();
    debug!("Running in debug mode");

    let fridge = sys.actor_of_args::<fridge::Fridge, _>("fridge", &cf)?;

    let w = web::listen_http(&sys, fridge.clone(), &cf);
    async_std::task::block_on(w)?;
    // Webserver waits listening forever
    Ok(())
}

#[derive(Debug, StructOpt)]
#[structopt(name = "Wort Temperature", about = "Matt Johnston 2020 matt@ucc.asn.au")]
struct Opt {
    /// Replace existing running instance
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

fn handle_args() -> Opt {
    let mut args = Opt::from_args();

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

// fn setup_log(debug: bool) {
//     let loglevel = if debug {
//        log::LevelFilter::Debug
//     } else {
//        log::LevelFilter::Info
//     };

//     let format = |record: &log::Record| {
//         let datefmt = "%Y-%m-%d %I:%M:%S %p";
//             let ts = chrono::Local::now().format(datefmt);
//         format!("{}: {} - {}", ts, record.level(), record.args())
//     };


//     let mut builder = env_logger::Builder::new();
//     builder.format(format).filter(Some("wort_templog"), loglevel);
//     builder.init().unwrap();
// }

fn setup_log() -> slog_scope::GlobalLoggerGuard {
    let log = make_logger();
    let guard = slog_scope::set_global_logger(log);
    guard
}

fn make_logger() -> slog::Logger {
    let plain = slog_term::PlainSyncDecorator::new(std::io::stdout());
    slog::Logger::root(
        slog_term::FullFormat::new(plain)
        .build().fuse(), slog_o!()
        )
}


fn main() -> Result<()> {
    // Create an initial logger to use until riker starts
    let _log_guard = setup_log();

    let args = handle_args();
    run(args)
}
