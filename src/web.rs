
use std::time::Duration;

use crate::Config;
use tide::sessions::Session;
use anyhow::{Result,anyhow,Context};

use riker::actors::*;
use riker_patterns::ask::ask;
use futures::future::RemoteHandle;

use serde::{Serialize,Deserialize};

use tide::utils::After;
use tide::{Request,Response,StatusCode};

use crate::fridge;
use crate::params::Params;
use crate::types::DurationFormat;

#[derive(Clone)]
struct WebState {
    sys: ActorSystem,
    fridge: ActorRef<fridge::FridgeMsg>,
    config: &'static Config,
}

impl WebState {
    fn new(sys: &ActorSystem, fridge: ActorRef<fridge::FridgeMsg>, config: &'static Config) -> Self {
        WebState {
            sys: sys.clone(),
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
    recent_off_time: Option<String>,

    numinputs: Vec<NumInput>,
    yesnoinputs: Vec<YesNoInput>,
}

async fn handle_set(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    let p: RemoteHandle<fridge::Status> = ask(&s.sys, &s.fridge, fridge::GetStatus);
    let status = p.await;

    let ses: &Session = req.ext().ok_or_else(|| anyhow!("Missing session"))?;
    let allowed = s.config.allowed_sessions.contains(ses.id());

    debug!("set with session id {} {}", ses.id(), if allowed { "allowed" } else { "not allowed"} );

    let recent_off_time = if status.on {
        None
    } else {
        Some(status.off_duration.as_short_str())
    };


    let mut s = SetPage {
        status,
        csrf_blob: "unused", // hopefully SameSite=Strict is enough for now
        allowed,
        cookie_hash: ses.id(),
        debug: s.config.debug,
        recent_off_time,

        numinputs: vec![],
        yesnoinputs: vec![],
    };

    s.yesnoinputs.push(YesNoInput::new("running", "Running"));
    s.yesnoinputs.push(YesNoInput::new("nowort", "No wort"));
    s.numinputs.push(NumInput::new("fridge_setpoint", "Setpoint", "°", 0.1, 1));
    s.numinputs.push(NumInput::new("fridge_difference", "Difference", "°", 0.1, 1));
    s.numinputs.push(NumInput::new("fridge_range_lower", "Lower range", "°", 1.0, 0));
    s.numinputs.push(NumInput::new("fridge_range_upper", "Upper range", "°", 1.0, 0));

    let r = askama_tide::into_response(&s, "html");

    Ok(r)
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

async fn handle_logout(mut req: Request<WebState>) -> tide::Result {
    req.session_mut().destroy();
    Ok(tide::Redirect::new("/").into())
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
        email: &s.config.auth_email,
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

    #[derive(Deserialize)]
    struct Update {
        params: Params,
    }

    let update: Update = req.body_json().await.or_else(|e| {
        debug!("failed decoding update: {:?}", e);
        Err(e)
        })?;

    // send the params to the fridge
    let p: RemoteHandle<Result<(),String>> = ask(&s.sys, &s.fridge, update.params);

    // check it succeeded
    p.await
    .map(|_| "Updated".into())
    .map_err(|e| tide::http::Error::from_str(StatusCode::InternalServerError, e))
}

async fn handle_status(req: Request<WebState>) -> tide::Result {
    let s = req.state();
    let s: RemoteHandle<fridge::Status> = ask(&s.sys, &s.fridge, fridge::GetStatus);
    let status = s.await;
    let resp = Response::builder(200)
    .body(tide::Body::from_json(&status)?)
    .content_type(tide::http::mime::JSON)
    .build();
    Ok(resp)
}

fn until_2038() -> Result<Duration> {
    let time_2038 = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(i32::MAX as u64))
    .ok_or(anyhow!("failed making year 2038"))?;
    let dur = time_2038.duration_since(std::time::SystemTime::now()).context("unix epoch duration")?;
    // 100 day leeway for bad clocks
    let dur = dur - Duration::from_secs(3600*24*100);
    Ok(dur)
}

pub async fn listen_http(sys: &ActorSystem, fridge: ActorRef<fridge::FridgeMsg>, config: &'static Config) -> Result<()> {
    let ws = WebState::new(sys, fridge, config);
    let mut server = tide::with_state(ws);

    // Make it return a http error's string as the body.
    // https://github.com/http-rs/tide/issues/614
    // Not sure why this isn't a default?
    server.with(After(|mut res: Response| async {
        if let Some(err) = res.take_error() {
            res.set_body(err.to_string())
        }
        Ok(res)
    }));

    // just use the id for auth, cookie will be small
    server.with(tide::sessions::SessionMiddleware::new(
            tide::sessions::CookieStore::new(),
            config.session_secret.as_bytes(),
        )
        .with_session_ttl(Some(until_2038()?))
        .with_same_site_policy(tide::http::cookies::SameSite::Strict)
        .with_cookie_name(&config.auth_cookie)
        .without_save_unchanged()
    );

    // url handlers
    server.at("/").get(handle_set);
    server.at("/update").post(handle_update);
    server.at("/register").get(handle_register);
    server.at("/logout").get(handle_logout);
    server.at("/status").get(handle_status);

    let conf = tide_rustls::TlsListener::build()
            .addrs(":::4433")
            .cert(&config.cert_file)
            .key(&config.key_file);

    server.listen(conf).await?;
    Ok(())
}
