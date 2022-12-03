use crate::datafiles::DatafilesReport;
use log::*;
use std::sync::Arc;
#[cfg(any(feature = "discord", feature = "activity"))]
use std::thread::JoinHandle;

#[cfg(feature = "discord")]
mod discord;

#[cfg(feature = "activity")]
mod activity;

#[cfg(feature = "discord")]
pub use self::discord::DiscordBot;
#[cfg(feature = "discord")]
use crate::reporting::discord::DiscordSetupError;
#[cfg(feature = "activity")]
use crate::sprite_collab::RepositoryUpdate;

/// A wrapper around one or multiple thread/async join handles and/or
/// awaited futures that are used for reporting.
pub struct ReportingJoinHandle {
    #[cfg(feature = "discord")]
    discord_join_handle: Option<JoinHandle<serenity::Result<()>>>,
    #[cfg(feature = "activity")]
    activity_join_handle: JoinHandle<Result<(), anyhow::Error>>,
}

impl ReportingJoinHandle {
    pub fn join(self) {
        #[cfg(feature = "discord")]
        if let Some(discord_join_handle) = self.discord_join_handle {
            debug!("Joining Discord thread...");
            match discord_join_handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    error!("The Discord client exited: {:?}", err);
                    panic!("Discord client failed.");
                }
                Err(err) => {
                    match err.downcast_ref::<String>() {
                        Some(as_string) => {
                            error!("The Discord main thread could not be joined: {}", as_string);
                        }
                        None => {
                            error!("The Discord main thread could not be joined: {:?}", err);
                        }
                    }
                    panic!("Discord client failed.");
                }
            }
        }
        #[cfg(feature = "activity")]
        {
            debug!("Joining Activity thread...");
            match self.activity_join_handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    error!("The Activity thread exited: {:?}", err);
                    panic!("Activity thread failed.");
                }
                Err(err) => {
                    match err.downcast_ref::<String>() {
                        Some(as_string) => {
                            error!(
                                "The Activity main thread could not be joined: {}",
                                as_string
                            );
                        }
                        None => {
                            error!("The Activity main thread could not be joined: {:?}", err);
                        }
                    }
                    panic!("Activity thread failed.");
                }
            }
        }
    }
}

pub struct Reporting {
    #[cfg(feature = "discord")]
    pub(crate) discord_bot: Option<Arc<DiscordBot>>,
    #[cfg(feature = "activity")]
    pub(crate) activity: Arc<activity::Activity>,
}

impl Reporting {
    pub async fn send_event(&self, event: ReportingEvent) {
        event.log();
        #[cfg(feature = "discord")]
        if let Some(discord_bot) = &self.discord_bot {
            discord_bot.send_event(event).await;
        }
    }

    #[cfg(feature = "activity")]
    pub async fn update_activity(
        &self,
        repo_update: RepositoryUpdate,
    ) -> Result<(), anyhow::Error> {
        self.activity.update(repo_update).await
    }

    pub async fn shutdown(&self) {
        #[cfg(feature = "discord")]
        if let Some(discord_bot) = &self.discord_bot {
            discord_bot.shutdown().await;
        }
        #[cfg(feature = "activity")]
        self.activity.close().await;
    }
}

pub async fn init_reporting() -> (Arc<Reporting>, ReportingJoinHandle) {
    #[cfg(feature = "discord")]
    let (discord_bot, discord_join_handle) = match discord::discord_main().await {
        Ok((app, join_handle)) => (Some(Arc::new(app)), Some(join_handle)),
        Err(DiscordSetupError::NoTokenProvided) => {
            warn!("Discord was not set up, since no bot token was provided.");
            (None, None)
        }
        Err(DiscordSetupError::NoChannelsProvided) => {
            warn!("Discord was not set up, since no channel was provided.");
            (None, None)
        }
        Err(err) => {
            error!("Failed setting up Discord: {:?}", err);
            panic!("Failed setting up Discord.");
        }
    };

    #[cfg(feature = "activity")]
    let (activity, activity_join_handle) = match activity::activity_main().await {
        Ok((activity, join_handle)) => (Arc::new(activity), join_handle),
        Err(err) => {
            error!("Failed setting up Activity: {:?}", err);
            panic!("Failed setting up Activity.");
        }
    };

    (
        Arc::new(Reporting {
            #[cfg(feature = "discord")]
            discord_bot,
            #[cfg(feature = "activity")]
            activity,
        }),
        ReportingJoinHandle {
            #[cfg(feature = "discord")]
            discord_join_handle,
            #[cfg(feature = "activity")]
            activity_join_handle,
        },
    )
}

#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(clippy::manual_non_exhaustive)]
pub enum ReportingEvent {
    Start,
    Shutdown,
    UpdateDatafiles(DatafilesReport),
    StaleDatafiles(String),
    #[doc(hidden)]
    /// Signal for all reporting threads to shut down.
    __Shutdown,
    #[doc(hidden)]
    /// This is not an actual event, but a signal that reporting handlers should wake up and
    /// potentially process some other actions.
    __Wakeup,
}

impl ReportingEvent {
    pub fn log(&self) {
        match self {
            ReportingEvent::Start => info!("Started server."),
            ReportingEvent::Shutdown => info!("Shutting down..."),
            ReportingEvent::UpdateDatafiles(DatafilesReport::Ok) => {
                debug!("Data refresh finished.");
            }
            ReportingEvent::StaleDatafiles(commit) => warn!(
                "Server running with stale datafiles from commit {}.",
                commit
            ),
            ReportingEvent::UpdateDatafiles(de) => {
                error!("Error updating the data files: {}", de.format_short());
            }
            _ => {}
        }
    }
}
