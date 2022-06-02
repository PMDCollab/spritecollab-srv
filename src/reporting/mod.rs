use std::sync::Arc;
#[cfg(feature = "discord")]
use std::thread::JoinHandle;
use log::*;
use crate::datafiles::DatafilesReport;

#[cfg(feature = "discord")]
use crate::reporting::discord::{DiscordBot, DiscordSetupError};

#[cfg(feature = "discord")]
mod discord;

/// A wrapper around one or multiple thread/async join handles and/or
/// awaited futures that are used for reporting.
pub struct ReportingJoinHandle {
    #[cfg(feature = "discord")]
    discord_join_handle: Option<JoinHandle<serenity::Result<()>>>,
}

impl ReportingJoinHandle {
    pub fn join(self) {
        #[cfg(feature = "discord")]
        if let Some(discord_join_handle) = self.discord_join_handle {
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
    }
}

pub struct Reporting {
    #[cfg(feature = "discord")]
    discord_bot: Option<DiscordBot>,
}

impl Reporting {
    pub async fn send_event(&self, event: ReportingEvent) {
        event.log();
        #[cfg(feature = "discord")]
        if let Some(discord_bot) = &self.discord_bot {
            discord_bot.send_event(event).await;
        }
    }

    pub async fn shutdown(&self) {
        #[cfg(feature = "discord")]
        if let Some(discord_bot) = &self.discord_bot {
            discord_bot.shutdown().await;
        }
    }
}

pub async fn init_reporting() -> (Arc<Reporting>, ReportingJoinHandle) {
    #[cfg(feature = "discord")]
    {
        match discord::discord_main().await {
            Ok((app, join_handle)) => {
                (
                    Arc::new(Reporting { discord_bot: Some(app) }),
                    ReportingJoinHandle { discord_join_handle: Some(join_handle) }
                )
            }
            Err(DiscordSetupError::NoTokenProvided) => {
                warn!("Discord was not set up, since no bot token was provided.");
                (
                    Arc::new(Reporting { discord_bot: None }),
                    ReportingJoinHandle { discord_join_handle: None }
                )
            }
            Err(err) => {
                error!("Failed setting up Discord: {:?}", err);
                panic!("Failed setting up Discord.");
            }
        }
    }
    #[cfg(not(feature = "discord"))]
    {
        (Arc::new(Reporting { }), ReportingJoinHandle { })
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
#[allow(clippy::manual_non_exhaustive)]
pub enum ReportingEvent {
    Start,
    Shutdown,
    UpdateDatafiles(DatafilesReport),
    #[doc(hidden)]
    __Shutdown
}

impl<'a> ReportingEvent {
    pub fn log(&self) {
        match self {
            ReportingEvent::Start => info!("Started server."),
            ReportingEvent::Shutdown => info!("Shutting down..."),
            ReportingEvent::UpdateDatafiles(DatafilesReport::Ok) => {
                debug!("Data refresh finished.");
            },
            ReportingEvent::UpdateDatafiles(DatafilesReport::Ok) => {}
            ReportingEvent::UpdateDatafiles(de) => {
                error!("Error updating the data files: {}", de.format_short());
            }
            _ => {}
        }
    }
}
