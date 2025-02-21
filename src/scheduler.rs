use crate::SpriteCollab;
use log::info;
use std::mem::take;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

const REFRESH_INTERVAL: u64 = 15 * 60;
pub struct DataRefreshScheduler(Option<JoinHandle<()>>, Sender<()>);

impl DataRefreshScheduler {
    pub fn new(sprite_collab: Arc<SpriteCollab>) -> Self {
        let (shutdown_sender, shutdown_receiver) = channel();

        let handle = thread::spawn(move || {
            info!("Starting Job Scheduler.");
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                loop {
                    if shutdown_receiver
                        .recv_timeout(Duration::from_secs(REFRESH_INTERVAL))
                        .is_ok()
                    {
                        // Sleep was interrupted
                        break;
                    }
                    SpriteCollab::refresh(sprite_collab.clone()).await
                }
            });
            info!("Stopped Job Scheduler.");
        });

        Self(Some(handle), shutdown_sender)
    }

    pub fn shutdown(&mut self) {
        self.1.send(()).unwrap();
        let jh = take(&mut self.0);
        jh.unwrap().join().ok();
    }
}
