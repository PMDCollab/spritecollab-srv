//! SpriteCollab Rust GraphQL Server.
//!
//! Access `/ for GraphiQL.
#![forbid(unused_must_use)]

use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Mutex;
use std::time::Duration;
use std::{convert::Infallible, sync::Arc};

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::http::HeaderValue;
use hyper::{service::service_fn, Method, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use juniper::{EmptyMutation, EmptySubscription, RootNode};
use log::{info, warn};
use tokio::net::TcpListener;

use crate::assets::{make_box_body, match_and_process_assets_path};
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

    let sprite_collab = SpriteCollab::new(Config::redis_config()).await;

    let scheduler = Arc::new(Mutex::new(DataRefreshScheduler::new(sprite_collab.clone())));

    let addr: SocketAddr = ([0, 0, 0, 0], PORT).into();

    let ctx = Arc::new(Context::new(sprite_collab.clone()));
    let root_node = Arc::new(RootNode::new(
        Query,
        EmptyMutation::<Context>::new(),
        EmptySubscription::<Context>::new(),
    ));

    let listener = TcpListener::bind(addr)
        .await
        .expect("expected to listen on address");
    let graceful = GracefulShutdown::new();
    let server = Arc::new(auto::Builder::new(TokioExecutor::new()));

    let mut ctrl_c = pin!(tokio::signal::ctrl_c());

    info!("GraphQL server started.");
    loop {
        let root_node = root_node.clone();
        let ctx = ctx.clone();
        let sprite_collab = sprite_collab.clone();
        let server = server.clone();

        tokio::select! {
            conn = listener.accept() => {
                let (stream, _) = match conn {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("failed to accept connection: {}", e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let io = TokioIo::new(stream);

                tokio::spawn(async move {
                    if let Err(e) = server
                        .serve_connection(
                            io,
                            service_fn(move |req| {
                                let root_node = root_node.clone();
                                let ctx = ctx.clone();
                                let sprite_collab = sprite_collab.clone();
                                async move {
                                    Ok::<_, Infallible>(match (req.method(), req.uri().path()) {
                                        (&Method::OPTIONS, _) => make_http_options_response().map(make_box_body),
                                        (&Method::GET, "/") => juniper_hyper::graphiql("/graphql", None).await.map(make_box_body),
                                        (&Method::GET, "/graphql") | (&Method::POST, "/graphql") => {
                                            let mut response = juniper_hyper::graphql(root_node, ctx, req).await;
                                            response.headers_mut().insert(
                                                "Access-Control-Allow-Origin",
                                                HeaderValue::try_from("*").unwrap(),
                                            );
                                            if response.status() != StatusCode::OK {
                                                let body = response.body();
                                                warn!(
                                                    "GraphQL request failed: {}",
                                                    body
                                                );
                                            }
                                            response.map(make_box_body)
                                        }
                                        (method, path) =>
                                            match_and_process_assets_path(
                                                method,
                                                path,
                                                sprite_collab.clone(),
                                            )
                                                .await
                                                .unwrap_or_else(|| {
                                                    let mut response = Response::new(String::from(
                                                        "<html><body><img src=\"https://http.cat/404\"></body></html>"
                                                    ));
                                                    *response.status_mut() = StatusCode::NOT_FOUND;
                                                    response.headers_mut().insert(
                                                        "content-type",
                                                        HeaderValue::from_str("text/html; charset=UTF-8").unwrap(),
                                                    );
                                                    response.map(make_box_body)
                                            })
                                    })
                                }
                            }),
                        )
                        .await
                    {
                        warn!("Error serving connection: {e}");
                    }
                });
            }

            _ = ctrl_c.as_mut() => {
                drop(listener);
                info!("Ctrl-C received, starting shutdown");
                break;
            }
        }
    }
    scheduler.lock().unwrap().shutdown();

    tokio::select! {
        _ = graceful.shutdown() => {
            info!("Gracefully shutdown!");
        },
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            warn!("Waited 10 seconds for graceful shutdown, aborting...");
        }
    }
}

/// Make a HTTP OPTIONS response.
fn make_http_options_response() -> Response<Empty<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        .header(
            "Access-Control-Allow-Headers",
            "Content-Type, Authorization, Accept",
        )
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Max-Age", "86400")
        .body(Empty::new())
        .unwrap()
}
