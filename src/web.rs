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

#[derive(askama::Template)]
#[template(path="set2.html")]
struct SetPage<'a> {
    params: Params,
    csrf_blob: &'a str,
    allowed: bool,
    email: &'a str,
    cookie_hash: &'a str,
}

/*
impl SetPage<'_> {
    fn spinstep(&self, digits: &usize) -> f32 {
        f32::powf(0.1, *digits as f32)
    }
}
*/

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
