#[allow(unused_imports)]
use {
    log::{debug, error, info, warn},
    anyhow::{Result,Context,bail,anyhow},
};

use simplelog::{CombinedLogger,LevelFilter,TermLogger,WriteLogger,TerminalMode,ColorChoice};

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
mod timeseries;
mod rusqlmem;

use crate::config::Config;

/// Futures return when SIGINT or SIGTERM happen, compatible with async-std
async fn wait_exit() -> Result<()> {
    // see https://github.com/stjepang/async-io/blob/master/examples/unix-signal.rs
    use async_io::Async;
    use std::os::unix::net::UnixStream;

    let (w, mut r) = Async::<UnixStream>::pair()?;
    signal_hook::low_level::pipe::register(signal_hook::consts::signal::SIGINT, w.get_ref().try_clone()?)?;
    signal_hook::low_level::pipe::register(signal_hook::consts::signal::SIGTERM, w.get_ref().try_clone()?)?;
    debug!("Waiting for exit signal");

    // Receive a byte that indicates the Ctrl-C signal occurred.
    r.read_exact(&mut [0]).await?;
    // Make sure we exit normally if something goes wrong during cleanup
    signal_hook::low_level::emulate_default_handler(signal_hook::consts::signal::SIGINT).unwrap_or_else(
        |e| warn!("Couldn't restore SIGINT handler {}", e));
    signal_hook::low_level::emulate_default_handler(signal_hook::consts::signal::SIGTERM).unwrap_or_else(
        |e| warn!("Couldn't restore SIGTERM handler {}", e));
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
    cf.testssl = args.testssl;

    debug!("Running in debug mode");
    if cf.testmode {
        info!("Running in test mode");
        if cf.testssl {
            info!("Using real ssl though");
        }
        cf.sensor_interval = 1;
    }
    if cf.dryrun {
        info!("Running in dry run mode");
    }

    let cf : &'static Config = Box::leak(Box::new(cf));
    // start actor system
    let spawner = act_zero::runtimes::async_std::Runtime;
    let fridge = Addr::new(&spawner, fridge::Fridge::try_new(&cf)?)?;
    let webserver = web::listen_http(fridge.downgrade(), &cf);

    let webserver = webserver.fuse();
    let exit = wait_exit().fuse();
    let mut fridge_done = fridge.downgrade().termination().fuse();
    futures::pin_mut!(webserver, exit);

    let allwaiting = async {
        let s = select! {
            res = webserver => res,
            _ = fridge_done => Err(anyhow!("main controller problem")),
            _ = exit => Ok(()),
        };
        s
    };
    let res = async_std::task::block_on(allwaiting);
    // make sure the fridge finishes regardless
    let final_fridge_done = fridge.termination();
    std::mem::drop(fridge);
    async_std::task::block_on(final_fridge_done);
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

    /// print default config (customise in fridgyeast.toml)
    #[argh(switch, short='e')]
    exampleconfig: bool,

    /// request ACME certificate even in test mode
    #[argh(switch)]
    testssl: bool,

    /// config file
    #[argh(option, short = 'c', default = "\"fridgyeast.toml\".to_string()")]
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
    let logconf = simplelog::ConfigBuilder::new()
    .set_time_format_str("%Y-%m-%d %H:%M:%S%.3f")
    .set_time_to_local(true)
    .build();
    CombinedLogger::init(
        vec![
            TermLogger::new(level, logconf.clone(), TerminalMode::Mixed, ColorChoice::Auto),
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

    let e = run(&args).map_err(|e| {
        error!("Bad Exit: {:?}", e);
        std::process::exit(1);
    });
    info!("Done.");
    e
}
