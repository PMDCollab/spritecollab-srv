use crate::sprite_collab::RepositoryUpdate;
use log::{debug, info};
use std::thread;
use std::thread::JoinHandle;
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub struct Activity {
    update_sender: Sender<Option<RepositoryUpdate>>,
}

impl Activity {
    async fn start(mut update_receiver: Receiver<Option<RepositoryUpdate>>) {
        debug!("Thread running.");
        while let Some(update) = update_receiver.recv().await {
            match update {
                None => {
                    debug!("Closing...");
                    break;
                }
                Some(_update) => {
                    debug!("New update.");
                }
            }
        }
    }

    pub async fn update(&self, repo_update: RepositoryUpdate) -> Result<(), anyhow::Error> {
        Ok(self.update_sender.send(Some(repo_update)).await?)
    }

    pub async fn close(&self) {
        let _ = self.update_sender.send(None).await;
    }
}

pub async fn activity_main(
) -> Result<(Activity, JoinHandle<Result<(), anyhow::Error>>), anyhow::Error> {
    let (update_sender, update_receiver) = channel(50);

    let handle = thread::spawn(move || -> Result<(), anyhow::Error> {
        info!("Starting Activity Thread.");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        #[allow(clippy::let_unit_value)]
        let r = rt.block_on(async { Activity::start(update_receiver).await });
        info!("Stopped Activity Thread.");
        Ok(r)
    });

    Ok((Activity { update_sender }, handle))
}
