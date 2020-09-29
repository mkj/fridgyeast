
use std::time::Duration;

use crate::Config;
use tide::sessions::Session;
use anyhow::{Result,anyhow,Context};

use riker::actors::*;
use riker_patterns::ask::ask;
use futures::future::RemoteHandle;

use serde::{Serialize,Deserialize};

use tide::utils::After;
use tide::{Response, StatusCode};

use crate::fridge;
use crate::params::Params;

const AUTH_COOKIE: &str = "fridgeyeast-auth";

#[derive(Clone)]
struct WebState {
    sys: ActorSystem,
    fridge: ActorRef<fridge::FridgeMsg>,
    config: Config,
}

impl WebState {
    fn new(sys: &ActorSystem, fridge: ActorRef<fridge::FridgeMsg>, config: &Config) -> Self {
        WebState {
            sys: sys.clone(),
            fridge,
            config: config.clone(),
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
    params: Params,
    csrf_blob: &'a str,
    allowed: bool,
    email: &'a str,
    cookie_hash: &'a str,

    numinputs: Vec<NumInput>,
    yesnoinputs: Vec<YesNoInput>,
}

async fn handle_set<'a>(req: tide::Request<WebState>) -> tide::Result<Response> {
    let s = req.state();
    let p: RemoteHandle<Params> = ask(&s.sys, &s.fridge, fridge::GetParams);

    let ses: &Session = req.ext().ok_or_else(|| anyhow!("Missing session"))?;

    let mut s = SetPage {
        params: p.await,
        csrf_blob: "unused", // hopefully SameSite=Strict is enough for now
        allowed: s.config.allowed_sessions.contains(ses.id()),
        email: "matt@ucc",
        cookie_hash: ses.id(),

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

async fn handle_update(mut req: tide::Request<WebState>) -> tide::Result<> {
    #[derive(Deserialize)]
    struct Update {
        params: Params,
    }

    debug!("got handle_update");
    let update: Update = req.body_json().await.or_else(|e| {
        debug!("failed decoding {:?}", e);
        Err(e)
        })?;
    debug!("got params {:?}", update.params);

    // send the params to the fridge
    let s = req.state();
    let p: RemoteHandle<Result<(),String>> = ask(&s.sys, &s.fridge, update.params);

    // check it succeeded
    p.await
    .map(|_| "Updated".into())
    .map_err(|e| tide::http::Error::from_str(StatusCode::InternalServerError, e))

}

fn until_2038() -> Result<Duration> {
    let time_2038 = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(i32::MAX as u64))
    .ok_or(anyhow!("failed making year 2038"))?;
    let dur = time_2038.duration_since(std::time::SystemTime::now()).context("unix epoch duration")?;
    // 100 day leeway for bad clocks
    let dur = dur - Duration::from_secs(3600*24*100);
    Ok(dur)
}

pub async fn listen_http(sys: &ActorSystem, fridge: ActorRef<fridge::FridgeMsg>, config: &Config) -> Result<()> {
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
    .with_cookie_name(AUTH_COOKIE));

    // url handlers
    server.at("/").get(handle_set);
    server.at("/update").post(handle_update);

    server.listen(
        tide_rustls::TlsListener::build()
            .addrs(":::4433")
            .cert(std::env::var("TIDE_CERT_PATH").unwrap_or("testcert.pem".to_string()))
            .key(std::env::var("TIDE_KEY_PATH").unwrap_or("testkey.pem".to_string()))
        ).await?;
    Ok(())
}
