//! SpriteCollab Rust GraphQL Server.
//!
//! Access `/ for GraphiQL.
mod assets;
mod cache;
mod config;
mod datafiles;
mod reporting;
mod scheduler;
#[forbid(unused_must_use)]
mod schema;
mod search;
mod sprite_collab;

use std::mem::take;
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::panic::{set_hook, PanicInfo};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use std::{convert::Infallible, sync::Arc, thread};

use crate::config::Config;
use crate::reporting::{init_reporting, Reporting, ReportingEvent, ReportingJoinHandle};
use crate::scheduler::DataRefreshScheduler;
use crate::schema::{Context, Query};
use crate::sprite_collab::SpriteCollab;
use backtrace::Backtrace;
use hyper::{
    server::Server,
    service::{make_service_fn, service_fn},
    Body, Method, Response, StatusCode,
};
use juniper::{EmptyMutation, EmptySubscription, RootNode};
use log::{error, info};
use once_cell::sync::OnceCell;
use tokio::runtime::Handle;
use tokio::task;

const PORT: u16 = 3000;

#[tokio::main]
async fn main() {
    Config::init();
    Config::check();
    pretty_env_logger::init_timed();

    let (reporting, reporting_join_handle) = init_reporting().await;
    reporting.send_event(ReportingEvent::Start).await;
    GlobalShutdown::register_panic_hook(reporting.clone(), reporting_join_handle);

    let sprite_collab = SpriteCollab::new(Config::redis_config(), reporting.clone()).await;

    let scheduler = Arc::new(Mutex::new(DataRefreshScheduler::new(sprite_collab.clone())));
    GlobalShutdown::add_scheduler(scheduler.clone());

    let addr: SocketAddr = ([0, 0, 0, 0], PORT).into();

    let ctx = Arc::new(Context::new(sprite_collab, reporting.clone()));
    let root_node = Arc::new(RootNode::new(
        Query,
        EmptyMutation::<Context>::new(),
        EmptySubscription::<Context>::new(),
    ));

    let new_service = make_service_fn(move |_| {
        let root_node = root_node.clone();
        let ctx = ctx.clone();

        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                let root_node = root_node.clone();
                let ctx = ctx.clone();
                async {
                    Ok::<_, Infallible>(match (req.method(), req.uri().path()) {
                        (&Method::GET, "/") => juniper_hyper::graphiql("/graphql", None).await,
                        (&Method::GET, "/graphql") | (&Method::POST, "/graphql") => {
                            juniper_hyper::graphql(root_node, ctx, req).await
                        }
                        _ => {
                            let mut response = Response::new(Body::from(
                                "<html><body><img src=\"https://http.cat/404\"></body></html>",
                            ));
                            *response.status_mut() = StatusCode::NOT_FOUND;
                            response
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

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

#[derive(Default)]
struct GlobalShutdown(
    Option<(Arc<Reporting>, ReportingJoinHandle)>,
    Option<Arc<Mutex<DataRefreshScheduler>>>,
);

static mut GLOBAL_SHUTDOWN: OnceCell<Mutex<GlobalShutdown>> = OnceCell::new();

impl GlobalShutdown {
    #[allow(clippy::await_holding_lock)]
    pub async fn shutdown() {
        let mut slf = Self::slf();
        if slf.0.is_none() {
            return;
        }

        let slf = take(slf.deref_mut());
        let (reporting, reporting_join_handle) = slf.0.unwrap();

        reporting.send_event(ReportingEvent::Shutdown).await;

        // give everything a bit of time to send reports.
        thread::sleep(Duration::new(3, 0));

        if let Some(scheduler) = slf.1 {
            scheduler.lock().unwrap().shutdown();
        }
        reporting.shutdown().await;
        reporting_join_handle.join();
    }

    fn register_panic_hook(reporting: Arc<Reporting>, join_handle: ReportingJoinHandle) {
        Self::slf().0 = Some((reporting, join_handle));
        set_hook(Box::new(Self::panic));
    }

    fn add_scheduler(scheduler: Arc<Mutex<DataRefreshScheduler>>) {
        Self::slf().1 = Some(scheduler);
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
        unsafe { GLOBAL_SHUTDOWN.get_or_init(|| Mutex::new(GlobalShutdown(None, None))) }
            .lock()
            .unwrap()
    }
}
