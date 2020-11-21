#[allow(unused_imports)]
use {
    log::{debug, error, info, warn},
    anyhow::{Result,Context,bail},
};

use simplelog::{CombinedLogger,LevelFilter,TermLogger,WriteLogger,TerminalMode};

use act_zero::*;
use async_std::io::ReadExt;
use futures::FutureExt;
use futures::select;

mod config;
mod sensor;
mod fridge;
mod types;
mod params;
mod web;
mod actzero_pubsub;

use crate::config::Config;

/// Futures return when SIGINT or SIGTERM happen, compatible with async-std
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

fn run(args: &Args) -> Result<()> {
    // load config, make it static
    let mut cf = config::Config::load(&args.config)?;
    cf.debug = args.debug;
    cf.testmode = args.test;
    cf.nowait = args.nowait;
    cf.dryrun = args.dryrun;
    let cf : &'static Config = Box::leak(Box::new(cf));

    debug!("Running in debug mode");
    if cf.testmode {
        info!("Running in test mode")
    }
    if cf.dryrun {
        info!("Running in dry run mode")
    }

    // start actor system
    let spawner = act_zero::runtimes::async_std::Runtime;
    let fridge = Addr::new(&spawner, fridge::Fridge::try_new(&cf)?)?;
    let webserver = web::listen_http(fridge, &cf);

    let webserver = webserver.fuse();
    let exit = wait_exit().fuse();
    futures::pin_mut!(webserver, exit);

    let allwaiting = async {
        select! {
            w = webserver => w,
            _ = exit => Ok(()),
        }
    };

    async_std::task::block_on(allwaiting)
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

fn open_logfile() -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open("fridgyeast.log").context("Error opening logfile")?;
    Ok(f)
}

fn setup_log(debug: bool) -> Result<()> {
    let level = match debug {
        true => LevelFilter::Debug,
        false => LevelFilter::Info,
    };
    println!("log level {:?}", level);
    let logconf = simplelog::ConfigBuilder::new()
    .set_time_format_str("%Y-%m-%d %H:%M:%S%.3f")
    .set_time_to_local(true)
    .build();
    CombinedLogger::init(
        vec![
            TermLogger::new(level, logconf.clone(), TerminalMode::Mixed),
            WriteLogger::new(level, logconf, open_logfile()?),
        ]
    ).context("logging setup failed")
}

fn handle_args() -> Result<Args> {
    let mut args: Args = argh::from_env();

    if args.exampleconfig {
        println!("{}", config::Config::example_toml());
        std::process::exit(0);
    }

    setup_log(args.debug)?;

    if cfg!(not(target_os = "linux")) {
        info!("Forcing --test for non-Linux");
        args.test = true;
    }

    Ok(args)
}

fn main() -> Result<()> {
    let args = handle_args()?;
    info!("fridgyeast hg version {}. pid {}", types::get_hg_version(), std::process::id());

    match run(&args) {
        Err(e) => error!("Failed running: {:?}", e),
        Ok(_) => info!("Done."),
    }
    Ok(())
}
