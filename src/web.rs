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

#[derive(askama::Template,Clone,Serialize)]
#[template(path="numinput.html")]
struct NumInput {
    name: String,
    value: f32,
    title: String,
    unit: String,
    step: f32,
    digits: usize,
}


#[derive(askama::Template)]
#[template(path="set2.html")]
struct SetPage<'a> {
    params: Params,
    csrf_blob: &'a str,
    allowed: bool,
    email: &'a str,
    cookie_hash: &'a str,

    numinputs: RefCell<Vec<NumInput>>,
    yesnoinputs: RefCell<Vec<String>>,
}
#[derive(askama::Template)]
#[template(path="yesnoinput.html")]
struct YesNoInput<'a> {
    name: &'a str,
    value: bool,
    title: &'a str,
}

impl SetPage<'_> {
    fn add_numinput<'a>(&self, 
            name: &'a str,
            value: &'a f32,
            title: &'a str,
            unit: &'a str,
            step: &'a f32,
            digits: &'a usize,
        ) -> NumInput {
        let input = NumInput {
            name: name.to_string(),
            value: *value,
            title: title.to_string(),
            unit: unit.to_string(),
            step: *step,
            digits: *digits,
        };
        self.numinputs.borrow_mut().push(input.clone());
        // TODO: automatically return equiv of input|safe 
        // if https://github.com/djc/askama/issues/108 is solved
        // "recognize a template instance and don't bother escaping it"
        input
    }
    fn add_yesnoinput<'a>(&self, 
            name: &'a str,
            value: &'a bool,
            title: &'a str,
        ) -> YesNoInput<'a> {
        let input = YesNoInput {
            name,
            value: *value,
            title,
        };
        self.yesnoinputs.borrow_mut().push(name.to_string());
        // TODO: automatically return equiv of input|safe 
        input
    }
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

        let s = SetPage {
            params: p.await,
            csrf_blob: "csrfblah",
            allowed: false,
            email: "matt@ucc",
            cookie_hash: "oof",

            numinputs: RefCell::new(vec![]),
            yesnoinputs: RefCell::new(vec![]),
        };

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
