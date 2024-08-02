//! SpriteCollab Rust GraphQL Server.
//!
//! Access `/ for GraphiQL.
#![forbid(unused_must_use)]

use std::{convert::Infallible, sync::Arc};
use std::mem::take;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::panic::{PanicInfo, set_hook};
use std::sync::{Mutex, MutexGuard};

use backtrace::Backtrace;
use hyper::{
    Body,
    Method,
    Response, server::Server, service::{make_service_fn, service_fn}, StatusCode,
};
use hyper::body::Bytes;
use hyper::http::HeaderValue;
use juniper::{EmptyMutation, EmptySubscription, RootNode};
use juniper::futures::StreamExt;
use log::{error, info, warn};
use once_cell::sync::OnceCell;
use tokio::runtime::Handle;
use tokio::task;

use crate::assets::match_and_process_assets_path;
use crate::config::Config;
use crate::scheduler::DataRefreshScheduler;
use crate::schema::{Context, Query};
use crate::sprite_collab::SpriteCollab;

mod assets;
mod cache;
mod config;
mod datafiles;
mod scheduler;
mod schema;
mod search;
mod sprite_collab;

const PORT: u16 = 3000;

#[tokio::main]
async fn main() {
    Config::init();
    Config::check();
    pretty_env_logger::init_timed();

    GlobalShutdown::register_panic_hook();

    let sprite_collab = SpriteCollab::new(Config::redis_config()).await;

    let scheduler = Arc::new(Mutex::new(DataRefreshScheduler::new(sprite_collab.clone())));
    GlobalShutdown::add_scheduler(scheduler.clone());

    let addr: SocketAddr = ([0, 0, 0, 0], PORT).into();

    let ctx = Arc::new(Context::new(sprite_collab.clone()));
    let root_node = Arc::new(RootNode::new(
        Query,
        EmptyMutation::<Context>::new(),
        EmptySubscription::<Context>::new(),
    ));
    let sprite_collab_cln = sprite_collab.clone();

    let new_service = make_service_fn(move |_| {
        let root_node = root_node.clone();
        let ctx = ctx.clone();
        let sprite_collab_cln = sprite_collab_cln.clone();

        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let root_node = root_node.clone();
                let ctx = ctx.clone();
                let sprite_collab_cln = sprite_collab_cln.clone();
                async move {
                    Ok::<_, Infallible>(match (req.method(), req.uri().path()) {
                        (&Method::OPTIONS, _) => make_http_options_response(),
                        (&Method::GET, "/") => juniper_hyper::graphiql("/graphql", None).await,
                        (&Method::GET, "/graphql") | (&Method::POST, "/graphql") => {
                            let mut response = juniper_hyper::graphql(root_node, ctx, req).await;
                            response.headers_mut().insert(
                                "Access-Control-Allow-Origin",
                                HeaderValue::try_from("*").unwrap(),
                            );
                            if response.status() != StatusCode::OK {
                                let body: Body = take(response.body_mut());
                                let collected: Vec<Result<Bytes, hyper::Error>> =
                                    body.collect().await;
                                let collected =
                                    collected.into_iter().collect::<Result<Vec<_>, _>>();
                                if let Ok(body) = collected {
                                    let body_cnt = body.into_iter().flatten().collect::<Vec<u8>>();
                                    warn!(
                                        "GraphQL request failed: {}",
                                        String::from_utf8_lossy(&body_cnt)
                                    );
                                    *response.body_mut() = Body::from(body_cnt);
                                } else {
                                    warn!("GraphQL request failed. Failed to parse body.");
                                    *response.body_mut() = Body::from(
                                        "Internal server error while trying to display error.",
                                    );
                                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                                }
                            }
                            response
                        }
                        (method, path) => {
                            match match_and_process_assets_path(
                                method,
                                path,
                                sprite_collab_cln.clone(),
                            )
                            .await
                            {
                                Some(r) => r,
                                None => {
                                    let mut response = Response::new(Body::from(
                                        "<html><body><img src=\"https://http.cat/404\"></body></html>",
                                    ));
                                    *response.status_mut() = StatusCode::NOT_FOUND;
                                    response.headers_mut().insert(
                                        "content-type",
                                        HeaderValue::from_str("text/html; charset=UTF-8").unwrap(),
                                    );
                                    response
                                }
                            }
                        }
                    })
                }
            }))
        }
    });

    let server = Server::bind(&addr).serve(new_service);
    let graceful = server.with_graceful_shutdown(shutdown_signal());

    info!("GraphQL server started.");
    if let Err(e) = graceful.await {
        error!("server error: {}", e)
    }

    GlobalShutdown::shutdown().await;
}

/// Make a HTTP OPTIONS response.
fn make_http_options_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        .header(
            "Access-Control-Allow-Headers",
            "Content-Type, Authorization, Accept",
        )
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Max-Age", "86400")
        .body(Body::from(""))
        .unwrap()
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

#[derive(Default)]
struct GlobalShutdown(Option<Arc<Mutex<DataRefreshScheduler>>>);

static mut GLOBAL_SHUTDOWN: OnceCell<Mutex<GlobalShutdown>> = OnceCell::new();

impl GlobalShutdown {
    #[allow(clippy::await_holding_lock)]
    pub async fn shutdown() {
        let mut slf = Self::slf();
        if slf.0.is_none() {
            return;
        }

        let slf = take(slf.deref_mut());

        if let Some(scheduler) = slf.0 {
            scheduler.lock().unwrap().shutdown();
        }
    }

    fn register_panic_hook() {
        set_hook(Box::new(Self::panic));
    }

    fn add_scheduler(scheduler: Arc<Mutex<DataRefreshScheduler>>) {
        Self::slf().0 = Some(scheduler);
    }

    fn panic(info: &PanicInfo) {
        error!("{}\nBacktrace:\n{:?}", info, Backtrace::new());
        task::block_in_place(move || {
            Handle::current().block_on(async move {
                Self::shutdown().await;
            });
        });
    }

    fn slf<'a>() -> MutexGuard<'a, GlobalShutdown> {
        unsafe { GLOBAL_SHUTDOWN.get_or_init(|| Mutex::new(GlobalShutdown(None))) }
            .lock()
            .unwrap()
    }
}
