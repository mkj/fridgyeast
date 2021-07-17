use std::time::Duration;

#[allow(unused_imports)]
use log::{debug, info, warn, error};

use std::net::ToSocketAddrs;


use crate::Config;
use tide::sessions::Session;
use anyhow::{Result,anyhow,Context};

use act_zero::*;

use serde::{Serialize,Deserialize};

use tide::utils::After;
use tide::{Request,Response,StatusCode};

use tide_acme::{AcmeConfig, TideRustlsExt};
use tide::listener::Listener;

use plotters::prelude::*;

use crate::fridge;
use crate::params::Params;
use crate::types::DurationFormat;
use crate::timeseries::Seq;

#[derive(Clone)]
struct WebState {
    fridge: WeakAddr<fridge::Fridge>,
    config: &'static Config,
}

impl WebState {
    fn new(fridge: WeakAddr<fridge::Fridge>, config: &'static Config) -> Self {
        WebState {
            fridge,
            config,
        }
    }
}

#[derive(askama::Template,Serialize)]
#[template(path="numinput.html")]
struct NumInput {
    name: String,
    title: String,
    unit: String,
    step: f32,
    digits: usize,
}

const COOKIE_NAME: &str = "fridgyeast-moreauth";
const CSRF_NAME: &str = "real-fridgyeast";

impl NumInput {
    fn new(name: &str, title: & str, unit: & str, step: f32, digits: usize, ) -> Self {
        NumInput {
            name: name.to_string(),
            title: title.to_string(),
            unit: unit.to_string(),
            step,
            digits,
        }
    }
}

#[derive(askama::Template,Serialize)]
#[template(path="yesnoinput.html")]
struct YesNoInput {
    name: String,
    title: String,
}

impl YesNoInput {
    fn new(name: &str, title: &str, ) -> Self {
        YesNoInput {
            name: name.to_string(),
            title: title.to_string(),
        }
    }
}

#[derive(askama::Template)]
#[template(path="set2.html")]
struct SetPage<'a> {
    status: fridge::Status,
    csrf_blob: &'a str,
    allowed: bool,
    cookie_hash: &'a str,
    debug: bool,
    testmode: bool,
    recent_off_time: Option<String>,

    numinputs: Vec<NumInput>,
    yesnoinputs: Vec<YesNoInput>,
    worts: Seq,
    svg: String,
}

async fn handle_set(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    let worts = call!(s.fridge.history("wort".into())).await?;
    let status = call!(s.fridge.get_status()).await?;

    let ses: &Session = req.ext().ok_or_else(|| anyhow!("Missing session"))?;
    let allowed = s.config.allowed_sessions.contains(ses.id());

    debug!("set with session id {} {}", ses.id(), if allowed { "allowed" } else { "not allowed"} );

    let recent_off_time = if status.on {
        None
    } else {
        Some(status.off_duration.as_short_str())
    };

    debug!("cookies are {:?}", req.cookie("fridgyeast-moreauth"));

    let svg = svg(s).await?;

    let mut s = SetPage {
        status,
        csrf_blob: "unused", // hopefully SameSite=Strict is enough for now
        allowed,
        cookie_hash: ses.id(),
        debug: s.config.debug,
        testmode: s.config.testmode,
        recent_off_time,

        numinputs: vec![],
        yesnoinputs: vec![],

        worts,
        svg,
    };

    s.yesnoinputs.push(YesNoInput::new("running", "Running"));
    s.yesnoinputs.push(YesNoInput::new("nowort", "No wort"));
    s.numinputs.push(NumInput::new("fridge_setpoint", "Setpoint", "°", 0.1, 1));
    s.numinputs.push(NumInput::new("fridge_difference", "Difference", "°", 0.1, 1));
    s.numinputs.push(NumInput::new("overshoot_factor", "Inertia", "°", 0.1, 1));
    s.numinputs.push(NumInput::new("fridge_range_lower", "Lower range", "°", 1.0, 0));
    s.numinputs.push(NumInput::new("fridge_range_upper", "Upper range", "°", 1.0, 0));

    let mut r = askama_tide::into_response(&s, "html");

    // set a different samesite cookie for CSRF protection
    r.insert_cookie(tide::http::cookies::Cookie::build(CSRF_NAME, "yeah")
        .secure(true)
        .http_only(true)
        .same_site(tide::http::cookies::SameSite::Strict)
        .finish());

    Ok(r)
}

async fn handle_logout(mut req: Request<WebState>) -> tide::Result {
    req.session_mut().destroy();
    Ok(tide::Redirect::new("/").into())
}

async fn svg(state: &WebState) -> Result<String> {
    let worts = call!(state.fridge.history("wort".into())).await?;
    let mut out = String::new();
    {
        let w = 300f32;
        let ratio = (1f32+5f32.powf(0.5)) / 2f32;
        let h = w / (ratio);
        let area = plotters_svg::SVGBackend::with_string(&mut out, (w as u32, h as u32)) .into();
        let mut plot = ChartBuilder::on(&area)
        .build_cartesian_2d(worts.first().unwrap().0..worts.last().unwrap().0, 0f32..30f32)?;

        // plot.configure_mesh().draw();

        plot.draw_series(
            LineSeries::new(
                worts,
                &BLUE,
            )
        )?;
    }
    Ok(out)
}


async fn handle_history(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    let out = svg(s).await?;

    let resp = Response::builder(200)
    .body(out)
    .content_type(tide::http::mime::SVG)
    .build();
    Ok(resp)
}

#[derive(askama::Template)]
#[template(path="register.html")]
struct Register<'a> {
    allowed: bool,
    debug: bool,
    known: bool,
    email: &'a str,
    cookie_hash: &'a str,
}

async fn handle_register(mut req: Request<WebState>) -> tide::Result {
    // This complication is because Android Firefox doesn't send samesite: Strict 
    // cookies when you navigate to an url in the location bar.
    // https://bugzilla.mozilla.org/show_bug.cgi?id=1573860
    let s = req.state().clone();
    let ses: &mut Session = req.session_mut();
    let known = ses.get_raw("known");
    ses.insert("known", true)?;
    let allowed = s.config.allowed_sessions.contains(ses.id());

    let r = Register {
        email: &s.config.owner_email,
        cookie_hash: ses.id(),
        known: known.is_some(),
        debug: s.config.debug,
        allowed,
    };
    let r = askama_tide::into_response(&r, "html");
    Ok(r)
}

async fn handle_update(mut req: Request<WebState>) -> tide::Result {
    let s = req.state().clone();
    let ses: &mut Session = req.session_mut();
    let allowed = s.config.allowed_sessions.contains(ses.id());

    if !allowed {
        return Err(tide::http::Error::from_str(403, "Not registered"))
    }

    if req.cookie(CSRF_NAME).is_none() {
        return Err(tide::http::Error::from_str(403, "Bad CSRF"))
    }

    #[derive(Deserialize)]
    struct Update {
        params: Params,
    }

    let update: Update = req.body_json().await.map_err(|e| {
        debug!("failed decoding update: {:?}", e);
        e
        })?;

    // send the params to the fridge
    // note the extra ? is to unwrap the call! itself
    call!(s.fridge.set_params(update.params)).await?
    .map(|_| "Updated".into())
    .map_err(|e| tide::http::Error::from_str(StatusCode::InternalServerError, e))
}

async fn handle_status(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    let status = call!(s.fridge.get_status()).await?;
    let resp = Response::builder(200)
    .body(tide::Body::from_json(&status)?)
    .content_type(tide::http::mime::JSON)
    .build();
    Ok(resp)
}

fn until_2038() -> Duration {
    let time_2038 = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(i32::MAX as u64))
        .expect("failed making year 2038");
    let dur = time_2038.duration_since(std::time::SystemTime::now()).expect("Failed unix epoch duration");
    // 100 day leeway for bad clocks
    dur - Duration::from_secs(3600*24*100)
}

async fn listen_test(server: tide::Server<WebState>, 
    _config: &'static Config, 
    addrs: Vec<std::net::SocketAddr>) -> Result<()> {
    let mut listener = server.bind(addrs).await.context("binding web server failed")?;
    listener.accept().await.context("web server failed")?;
    Ok(())
}

async fn listen_ssl(server: tide::Server<WebState>, 
    config: &'static Config, 
    addrs: Vec<std::net::SocketAddr>) -> Result<()> {
    let conf = tide_rustls::TlsListener::build()
            .addrs(&*addrs)
            .acme(
                AcmeConfig::new()
                .domains(config.ssl_domain.clone())
                .cache_dir(&config.params_dir)
                .contact_email(&config.owner_email)
                .production()
                );

    let mut listener = server.bind(conf).await.context("binding web server failed")?;
    listener.accept().await.context("web server failed")?;
    Ok(())
}

pub async fn listen_http(fridge: WeakAddr<fridge::Fridge>, config: &'static Config) -> Result<()> {
    let ws = WebState::new(fridge, config);
    let mut server = tide::with_state(ws);

    // Make it return a http error's string as the body.
    // https://github.com/http-rs/tide/issues/614
    // Not sure why this isn't a default?
    server.with(After(|mut res: Response| async {
        debug!("response {:?}", res);

        let st = res.status();
        if st == tide::StatusCode::NotFound {
            res.set_body(format!("{} {}", st, st.canonical_reason()));
        }

        if let Some(err) = res.take_error() {
            res.set_body(err.to_string());
        }

        Ok(res)
    }));

    // just use the id for auth, cookie will be small
    server.with(tide::sessions::SessionMiddleware::new(
            tide::sessions::CookieStore::new(),
            config.session_secret.as_bytes(),
        )
        .with_session_ttl(Some(until_2038()))
        .with_same_site_policy(tide::http::cookies::SameSite::Lax)
        .with_cookie_name(COOKIE_NAME)
        .without_save_unchanged()
    );

    server.with(tide_compress::CompressMiddleware::new());

    // url handlers
    server.at("/").get(handle_set);
    server.at("/history.svg").get(handle_history);
    server.at("/update").post(handle_update);
    server.at("/register").get(handle_register);
    server.at("/logout").get(handle_logout);
    server.at("/status").get(handle_status);

    let mut addrs = vec![];
    for l in &config.listen {
        addrs.extend(l.to_socket_addrs().with_context(|| format!("Can't listen on '{}'", l))?);
    };

    if config.testmode && !config.testssl{
        listen_test(server, config, addrs).await?;
    } else {
        listen_ssl(server, config, addrs).await?;
    }

    Ok(())
}
