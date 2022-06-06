//! Optional (`discord` feature) Discord status reporting for the server.

use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use log::{info, trace, warn};
use std::mem::{discriminant, ManuallyDrop};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

use crate::datafiles::DatafilesReport;
use crate::reporting::ReportingEvent;
use crate::Config;
use serenity::client::bridge::gateway::ShardManager;
use serenity::client::ClientBuilder;
use serenity::http::CacheHttp;
use serenity::model::channel::{Channel, GuildChannel};
use serenity::model::prelude::{Ready, User};
use serenity::prelude::*;
use serenity::utils::Colour;
use serenity::{async_trait, Error};
use thiserror::Error;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::MutexGuard;
use tokio::time::timeout;

type RequestId = u64;
type DiscordId = u64;
type DiscordUserRequestResult = (RequestId, DiscordUserProfileResult);
type DiscordUserProfileResult = Result<Option<DiscordProfile>, anyhow::Error>;

struct ReportReceiver;

impl TypeMapKey for ReportReceiver {
    type Value = Receiver<ReportingEvent>;
}

struct UserRequestResponder;

impl TypeMapKey for UserRequestResponder {
    type Value = (
        Arc<Mutex<Receiver<(RequestId, DiscordId)>>>,
        Arc<Sender<DiscordUserRequestResult>>,
    );
}

struct ReadySender;

impl TypeMapKey for ReadySender {
    type Value = Sender<Result<(), DiscordSetupError>>;
}

struct ShardManagerShared;

impl TypeMapKey for ShardManagerShared {
    type Value = Arc<Mutex<ShardManager>>;
}

struct DatafilesFailedLastTypeAndTime;

impl TypeMapKey for DatafilesFailedLastTypeAndTime {
    type Value = (Option<DatafilesReport>, DateTime<Utc>);
}

const REPORT_DATAFILES_COOLDOWN_H: i64 = 12;

#[derive(Error, Debug)]
pub enum DiscordSetupError {
    #[error("No Discord token was provided.")]
    NoTokenProvided,
    #[error("No Discord channel was provided.")]
    NoChannelsProvided,
    #[error("{0}")]
    SerenityError(#[from] serenity::Error),
    #[error("Invalid Discord channel ID in configuration: {0}")]
    InvalidChannelIdFormat(String),
    #[error("Channel could not be retrieved (maybe not on the server?): {0} -> {1}")]
    ChannelNotFound(u64, Error),
    #[error("Server could not be retrieved (maybe not on the server?): {0} -> {1}")]
    GuildNotFound(u64, Error),
    #[error("The channel has an invalid type. It must be a guild text channel: {0}")]
    InvalidChannelType(Channel),
}

impl ReportingEvent {
    fn metadata_discord(&self) -> Option<(Option<&'static str>, Colour, String)> {
        match self {
            ReportingEvent::Start => Some((
                None,
                Colour::DARK_GREEN,
                "The server has started.".to_string(),
            )),
            ReportingEvent::Shutdown => Some((
                None,
                Colour::DARK_GOLD,
                "The server has been shut down.".to_string(),
            )),
            ReportingEvent::UpdateDatafiles(de) => {
                let (title, description) = de.format_for_discord();
                let colour = match de {
                    DatafilesReport::Ok => Colour::DARK_GREEN,
                    _ => Colour::RED,
                };
                Some((Some(title), colour, description))
            }
            _ => None,
        }
    }
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, mut ctx: Context, _ready: Ready) {
        // Collect and test channel IDs
        let data = ctx.data.write().await;
        let sender = data.get::<ReadySender>().unwrap();
        let channels_str = Config::DiscordChannels.get();
        let mut channels: Vec<GuildChannel> = Vec::new();
        for channel_id in channels_str.split(',') {
            let channel_id = match channel_id.trim().parse::<u64>() {
                Ok(v) => v,
                Err(_) => {
                    sender
                        .send(Err(DiscordSetupError::InvalidChannelIdFormat(
                            channel_id.to_owned(),
                        )))
                        .await
                        .unwrap();
                    return;
                }
            };
            let channel = match ctx.http().get_channel(channel_id).await {
                Ok(v) => v,
                Err(e) => {
                    sender
                        .send(Err(DiscordSetupError::ChannelNotFound(channel_id, e)))
                        .await
                        .unwrap();
                    return;
                }
            };
            match channel {
                Channel::Guild(channel) => {
                    let guild_id = channel.guild_id.0;
                    let guild = match ctx.http().get_guild(guild_id).await {
                        Ok(v) => v,
                        Err(e) => {
                            sender
                                .send(Err(DiscordSetupError::GuildNotFound(guild_id, e)))
                                .await
                                .unwrap();
                            return;
                        }
                    };
                    info!(
                        "Discord reporting set up for channel '{}' on server '{}'",
                        channel.name, guild.name
                    );
                    channels.push(channel);
                }
                _ => {
                    sender
                        .send(Err(DiscordSetupError::InvalidChannelType(channel)))
                        .await
                        .unwrap();
                    return;
                }
            }
        }
        sender.send(Ok(())).await.ok();
        drop(data);

        // Main reporting loop.
        loop {
            let mut data = ctx.data.write().await;
            let recv = data.get_mut::<ReportReceiver>().unwrap();
            let event = recv.recv().await;
            let (ur_recv, ur_send) = data.get_mut::<UserRequestResponder>().unwrap();
            let ur_recv = ur_recv.clone();
            let ur_send = ur_send.clone();
            drop(data);
            Self::process_user_requests(ur_recv, ur_send, &mut ctx).await;
            match event {
                None => {
                    let mut data = ctx.data.write().await;
                    let manager = data.get_mut::<ShardManagerShared>().unwrap();
                    manager.lock().await.shutdown_all().await;
                    return;
                }
                Some(ReportingEvent::__Wakeup) => { /* continue */ }
                Some(ReportingEvent::__Shutdown) => {
                    let mut data = ctx.data.write().await;
                    let manager = data.get_mut::<ShardManagerShared>().unwrap();
                    manager.lock().await.shutdown_all().await;
                    return;
                }
                Some(ReportingEvent::UpdateDatafiles(DatafilesReport::Ok)) => {
                    // only report if there was a previous failure
                    let mut data = ctx.data.write().await;
                    let (last_evt, _last_time) =
                        data.get_mut::<DatafilesFailedLastTypeAndTime>().unwrap();
                    if last_evt.is_some() {
                        self.report(
                            ReportingEvent::UpdateDatafiles(DatafilesReport::Ok),
                            &ctx,
                            &mut channels,
                        )
                        .await;
                        *last_evt = None;
                    }
                }
                Some(ReportingEvent::UpdateDatafiles(event)) => {
                    // only report if != previous failure within the last
                    // REPORT_DATAFILES_COOLDOWN_H hours.
                    let mut data = ctx.data.write().await;
                    let (last_evt, last_time) =
                        data.get_mut::<DatafilesFailedLastTypeAndTime>().unwrap();
                    if last_evt.is_none()
                        || discriminant(last_evt.as_ref().unwrap()) == discriminant(&event)
                    {
                        let now = Utc::now();
                        if &(now - Duration::hours(REPORT_DATAFILES_COOLDOWN_H)) >= last_time {
                            self.report(
                                ReportingEvent::UpdateDatafiles(event.clone()),
                                &ctx,
                                &mut channels,
                            )
                            .await
                        }
                        *last_time = now;
                        *last_evt = Some(event);
                    }
                }
                Some(event) => self.report(event, &ctx, &mut channels).await,
            }
        }
    }
}

impl Handler {
    async fn report(&self, event: ReportingEvent, ctx: &Context, channels: &mut Vec<GuildChannel>) {
        if let Some((title, color, description)) = event.metadata_discord() {
            for channel in channels {
                let send = channel
                    .send_message(ctx.http(), |msg| {
                        msg.add_embed(|embed| {
                            if let Some(title) = title {
                                embed.title(title);
                            }
                            embed.color(color);
                            embed.description(&description);
                            embed.footer(|footer| {
                                footer.text(Config::Address.get());
                                footer
                            });
                            embed
                        });
                        msg
                    })
                    .await;
                if let Err(send_err) = send {
                    warn!(
                        "Discord reporting in channel '{}' failed: {:?}",
                        channel.name, send_err
                    );
                }
            }
        }
    }

    async fn process_user_requests(
        recv: Arc<Mutex<Receiver<(RequestId, DiscordId)>>>,
        send: Arc<Sender<DiscordUserRequestResult>>,
        context: &mut Context,
    ) {
        trace!("UserReq[?]D - Checking...",);
        while let Ok((req, user_id)) = recv.lock().await.try_recv() {
            trace!("UserReq[{}]D - Processing...", req);
            // Try cache first
            if let Some(user) = context.cache.user(user_id) {
                send.send((req, Ok(Some(user.into())))).await.ok();
            } else {
                let user_res = context.http.get_user(user_id).await;
                send.send((
                    req,
                    user_res
                        .map(DiscordProfile::from)
                        .map(Some)
                        .map_err(anyhow::Error::from),
                ))
                .await
                .ok();
            }
            trace!("UserReq[{}]D - Done!", req);
        }
    }
}

/// Most basic information about a Discord user.
#[derive(Clone, Debug)]
pub struct DiscordProfile {
    pub id: DiscordId,
    pub name: String,
    pub discriminator: String,
}

impl From<User> for DiscordProfile {
    fn from(u: User) -> Self {
        Self {
            id: u.id.0,
            name: u.name,
            discriminator: u.discriminator.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct DiscordBot {
    reporting_sender: Sender<ReportingEvent>,
    user_request_id_increment: Mutex<u64>,
    user_request_sender: Sender<(RequestId, DiscordId)>,
    user_request_answer_receiver: Mutex<Receiver<DiscordUserRequestResult>>,
}

impl DiscordBot {
    pub async fn new(
        client_builder: ClientBuilder,
    ) -> Result<(Self, JoinHandle<serenity::Result<()>>), DiscordSetupError> {
        let (reporting_sender, reporting_receiver) = channel(50);
        let (user_request_sender, user_request_receiver) = channel(1000);
        let (user_request_answer_sender, user_request_answer_receiver) = channel(50);
        let (ready_sender, mut ready_receiver) = channel(1);
        let mut client = client_builder.event_handler(Handler).await?;

        let mut data = client.data.write().await;
        data.insert::<ReportReceiver>(reporting_receiver);
        data.insert::<UserRequestResponder>((
            Arc::new(Mutex::new(user_request_receiver)),
            Arc::new(user_request_answer_sender),
        ));
        data.insert::<ReadySender>(ready_sender);
        data.insert::<ShardManagerShared>(client.shard_manager.clone());
        data.insert::<DatafilesFailedLastTypeAndTime>((None, Utc::now()));
        drop(data);

        let handle = thread::spawn(move || {
            info!("Starting Discord Reporter.");
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let r = rt.block_on(async { client.start().await });
            info!("Stopped Discord Reporter.");
            r
        });

        // Wait for ready status and propagate errors.
        ready_receiver.recv().await.unwrap()?;

        Ok((
            DiscordBot {
                reporting_sender,
                user_request_id_increment: Mutex::new(0),
                user_request_sender,
                user_request_answer_receiver: Mutex::new(user_request_answer_receiver),
            },
            handle,
        ))
    }

    pub async fn send_event(&self, event: ReportingEvent) {
        self.reporting_sender
            .send(event)
            .await
            .expect("Failed to send event to Discord");
    }

    pub async fn shutdown(&self) {
        self.reporting_sender
            .send(ReportingEvent::__Shutdown)
            .await
            .expect("Failed to send event to Discord");
    }

    /// Returns the profile of the user with the given ID, if the bot is able to access it,
    /// and if it's a valid profile. Else None is returned.
    /// On API errors or other state errors, an error is returned.
    pub async fn get_user(
        &self,
        user_id: DiscordId,
    ) -> Result<Option<DiscordProfile>, anyhow::Error> {
        // We make sure there are no async race conditions, by forcing to hold this guard
        // until the end of the function, since messages are expected to be received sequentially
        // for each request.
        // As a fallback we have request timeouts and tag the messages with request IDs.
        let mut locked_increment = ManuallyDrop::new(self.user_request_id_increment.lock().await);
        // We need to do this in a sub-function since `?` would otherwise prevent the drop code below
        // to be skipped. Alternatively we could activate the try block feature...
        let response = self.get_user_inner(user_id, &mut locked_increment).await;
        // Explicitly drop guard after everything is done.
        drop(ManuallyDrop::into_inner(locked_increment));
        response
    }

    async fn get_user_inner<'a, 'b>(
        &'a self,
        user_id: DiscordId,
        locked_increment: &'b mut ManuallyDrop<MutexGuard<'a, u64>>,
    ) -> Result<Option<DiscordProfile>, anyhow::Error> {
        let request_id = ***locked_increment;
        ***locked_increment += 1;

        trace!("UserReq[{}]M - Sending...", request_id);
        self.user_request_sender.send((request_id, user_id)).await?;

        trace!("UserReq[{}]M - Sending Wakeup...", request_id);
        self.reporting_sender
            .send(ReportingEvent::__Wakeup)
            .await
            .ok();

        let response;
        loop {
            trace!("UserReq[{}]M - Waiting Response...", request_id);

            let (response_request_id, lresponse) = timeout(
                std::time::Duration::from_secs(3),
                self.user_request_answer_receiver.lock().await.recv(),
            )
            .await
            .map_err(|e| {
                warn!(
                    "Got timeout while waiting for Discord user request answer: {:?}",
                    e
                );
                e
            })?
            .unwrap_or_else(|| (request_id, Err(anyhow!("Discord thread is not available."))));

            if response_request_id != request_id {
                // This shouldn't happen, but oh well.
                trace!(
                    "UserReq[{}]M - Unexpected response ID: {}",
                    request_id,
                    response_request_id
                );
                continue;
            }
            response = lresponse;
            break;
        }

        trace!("UserReq[{}]M - Done!", request_id);
        response
    }
}

pub(crate) async fn discord_main(
) -> Result<(DiscordBot, JoinHandle<serenity::Result<()>>), DiscordSetupError> {
    if Config::DiscordChannels.get().is_empty() {
        return Err(DiscordSetupError::NoChannelsProvided);
    }
    match Config::DiscordToken.get_or_none() {
        None => Err(DiscordSetupError::NoTokenProvided),
        Some(token) => Ok(DiscordBot::new(Client::builder(token, GatewayIntents::empty())).await?),
    }
}
