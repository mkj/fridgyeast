use core::cell::Ref;
use std::cell::RefCell;
use anyhow::{Result};

use riker::actors::*;
use riker_patterns::ask::ask;
use futures::future::RemoteHandle;

use serde::{Serialize,Deserialize};

use askama::Template;
use std::convert::TryInto;
use tide::{http::mime::HTML, Body, Response};

use crate::fridge;
use crate::params::Params;

#[derive(Clone)]
struct WebState {
    sys: riker::system::ActorSystem,
    fridge: ActorRef<fridge::FridgeMsg>,
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
    fn new(name: &str,
           title: & str,
           unit: & str,
           step: f32,
           digits: usize,
           ) -> Self {
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
    fn new(name: &str,
        title: &str,
        ) -> Self {

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

pub async fn listen_http(sys: &riker::system::ActorSystem,
    fridge: ActorRef<fridge::FridgeMsg>) -> Result<()> {


    let mut server = tide::with_state(WebState {
        sys: sys.clone(),
        fridge,
    });

    server.at("/").get(|req: tide::Request<WebState>| async move { 
        let s = req.state();
        let p: RemoteHandle<Params> = ask(&s.sys, &s.fridge, fridge::GetParams);

        let mut s = SetPage {
            params: p.await,
            csrf_blob: "csrfblah",
            allowed: false,
            email: "matt@ucc",
            cookie_hash: "oof",

            numinputs: vec![],
            yesnoinputs: vec![],
        };

        s.yesnoinputs.push(YesNoInput::new("running", "Running"));
        s.yesnoinputs.push(YesNoInput::new("nowort", "No wort"));
        s.numinputs.push(NumInput::new("fridge_setpoint", "Setpoint", "°", 0.1, 1));
        s.numinputs.push(NumInput::new("fridge_difference", "Difference", "°", 0.1, 1));
        s.numinputs.push(NumInput::new("fridge_range_lower", "Lower range", "°", 1.0, 0));
        s.numinputs.push(NumInput::new("fridge_range_upper", "Upper range", "°", 1.0, 0));

        Ok(s)
    });

    server.listen(
        tide_rustls::TlsListener::build()
            .addrs(":::4433")
            .cert(std::env::var("TIDE_CERT_PATH").unwrap_or("testcert.pem".to_string()))
            .key(std::env::var("TIDE_KEY_PATH").unwrap_or("testkey.pem".to_string()))
        ).await?;
    Ok(())
}
