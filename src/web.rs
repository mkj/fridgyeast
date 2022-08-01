use std::time::Duration;

#[allow(unused_imports)]
use log::{debug, info, warn, error};
use plotters::coord::ranged1d::{NoDefaultFormatting, ValueFormatter};

use std::net::ToSocketAddrs;


use crate::Config;
use tide::sessions::Session;
use anyhow::{Result,anyhow,Context};

use act_zero::*;

use serde::{Serialize,Deserialize};

use tide::utils::After;
use tide::{Request,Response,StatusCode};

use tide_acme::{AcmeConfig, TideRustlsExt, rustls_acme::caches::DirCache};
use tide::listener::Listener;

use plotters::prelude::*;
use plotters::coord::ranged1d::KeyPointHint;

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
    svg: String,
}

impl<'a> SetPage<'a> {
    fn format_degrees(&self, t: &Option<f32>) -> String {
        match t {
            Some(t) => format!("{:.1}°", t),
            None => "?".into(),
        }
    }
}

async fn handle_set(req: Request<WebState>) -> tide::Result {
    let s = req.state();
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

    let svg = svg(s).await.unwrap_or_default();

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

struct DegreeValue {
    lower: f32,
    upper: f32,
}

impl Ranged for DegreeValue {
    type ValueType = f32;
    type FormatOption = NoDefaultFormatting;
    fn map(&self, value: &Self::ValueType, limit: (i32, i32)) -> i32 {
        let pix: f32 = (limit.1 - limit.0) as f32;
        limit.0 + (pix * (value - self.lower) / (self.upper - self.lower)) as i32
    }

    fn range(&self) -> std::ops::Range<Self::ValueType> {
        self.lower..self.upper
    }

    fn key_points<Hint: KeyPointHint>(&self, _hint: Hint) -> Vec<Self::ValueType> {
        let s = (self.lower as i32) / 10 * 10;
        let e = (self.upper as i32) / 10 * 10;
        (s..=e).step_by(10).map(|i| i as f32).collect()
    }
}

impl ValueFormatter<f32> for DegreeValue {
    fn format(v: &f32) -> String {
        format!("{:.0}°", v)
    }
}

async fn svg(state: &WebState) -> Result<String> {

    let (time1, time_desc) = if state.config.testmode {
        (chrono::Utc::now() - chrono::Duration::minutes(10), "10 minutes")
    } else {
        (chrono::Utc::now() - chrono::Duration::hours(8), "8 hours")
    };
    let time2 = chrono::Utc::now();
    let time_range = time1..time2;
    let temp_range = DegreeValue { lower: -2f32, upper: 32f32 };

    let worts = call!(state.fridge.history("wort".into(), time1)).await?;
    let fridges = call!(state.fridge.history("fridge".into(), time1)).await?;
    let setpoints = call!(state.fridge.history_step("setpoint".into(), time1)).await?;
    println!("setpoints {setpoints:?}");
    let mut out = String::new();
    let w = 300f32;
    // golden ratio is as good as any I guess
    let ratio = (1f32 + 5f32.powf(0.5)) / 2f32;
    let h = w / (ratio);
    let area = plotters_svg::SVGBackend::with_string(&mut out, (w as u32, h as u32)) .into();

    let amber = RGBColor(0xff, 0xa8, 0);
    let fridgeblue = RGBColor(0x93, 0xc8, 0xff);
    let green = RGBColor(0x9a, 0xd7, 0x51);
    let ruler = RGBColor(0xaa,0xaa,0xaa).stroke_width(1);

    let mut plot = ChartBuilder::on(&area)
    .y_label_area_size(40)
    .x_label_area_size(10)
    .build_cartesian_2d(time_range, temp_range)?;

    plot.configure_mesh()
    .disable_x_mesh()
    .disable_x_axis()
    .axis_style(ruler.stroke_width(0))
    .bold_line_style(ruler)
    .set_all_tick_mark_size(1)
    .x_desc(time_desc)
    .draw()?;

    plot.draw_series(
        LineSeries::new(
            fridges,
            fridgeblue.stroke_width(3),
        )
    )?;
    plot.draw_series(
        LineSeries::new(
            worts,
            amber.stroke_width(3),
        )
    )?;
    println!("setpoints {setpoints:?}");
    plot.draw_series(
        LineSeries::new(
            setpoints,
            green.stroke_width(1),
        )
    )?;

    // take back 'out'
    drop(plot);
    drop(area);
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

async fn handle_panic(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    if s.config.testmode {
        panic!("seeing what happens");
    }
    let resp = Response::builder(200)
    .body("Real fridges won't panic")
    .content_type(tide::http::mime::PLAIN)
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
    for a in &addrs {
        info!("Listening on http://{}", a);
    }
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
                AcmeConfig::new(&config.ssl_domain)
                .cache(DirCache::new(&config.params_dir))
                .contact_push(&config.owner_email)
                .directory_lets_encrypt(true)
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
    server.at("/panic").get(handle_panic);

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
