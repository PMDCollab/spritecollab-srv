//! Optional (`discord` feature) Discord status reporting for the server.

use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use log::{info, trace, warn};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::mem::{discriminant, take};
use std::ops::Deref;
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
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct ArcedAnyhowError(Arc<anyhow::Error>);

impl<E> From<E> for ArcedAnyhowError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        ArcedAnyhowError(Arc::new(e.into()))
    }
}

impl Deref for ArcedAnyhowError {
    type Target = anyhow::Error;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

type DiscordId = u64;
type DiscordUserRequestResult = (DiscordId, DiscordUserProfileResult);
pub type DiscordUserProfileResult = Result<Option<DiscordProfile>, ArcedAnyhowError>;

struct ReportReceiver;

impl TypeMapKey for ReportReceiver {
    type Value = Receiver<ReportingEvent>;
}

struct UserRequestResponder;

impl TypeMapKey for UserRequestResponder {
    type Value = (
        Arc<Mutex<Receiver<DiscordId>>>,
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
const GET_USER_CACHE_DURATION_MIN: i64 = 20;

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
        recv: Arc<Mutex<Receiver<DiscordId>>>,
        send: Arc<Sender<DiscordUserRequestResult>>,
        context: &mut Context,
    ) {
        trace!("UserReq[?]D - Checking...",);
        while let Ok(user_id) = recv.lock().await.try_recv() {
            trace!("UserReq[{}]D - Processing...", user_id);
            // Try cache first
            if let Some(user) = context.cache.user(user_id) {
                send.send((user_id, Ok(Some(user.into())))).await.ok();
            } else {
                let user_res = context.http.get_user(user_id).await;
                send.send((
                    user_id,
                    user_res
                        .map(DiscordProfile::from)
                        .map(Some)
                        .map_err(anyhow::Error::from)
                        .map_err(Arc::new)
                        .map_err(ArcedAnyhowError),
                ))
                .await
                .ok();
            }
            trace!("UserReq[{}]D - Done!", user_id);
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
struct PendingUserRequest {
    age: DateTime<Utc>,
    pending_requests: usize,
    response: Option<DiscordUserProfileResult>,
}

trait PendingUserRequestMap {
    fn gc_pending_map(&mut self);
    fn check_contains(&mut self, user_id: DiscordId) -> bool;
    fn pending_insert(&mut self, user_id: DiscordId);
    fn pending_increase(&mut self, user_id: DiscordId);
    fn take_pending_response(&mut self, user_id: DiscordId) -> Option<DiscordUserProfileResult>;
    fn pending_stop_waiting(&mut self, user_id: DiscordId);
    fn place_pending_response(&mut self, user_id: DiscordId, response: DiscordUserProfileResult);
    fn pending_still_valid(age: &DateTime<Utc>) -> bool {
        &(Utc::now() - Duration::minutes(GET_USER_CACHE_DURATION_MIN)) < age
    }
}

impl PendingUserRequestMap for HashMap<DiscordId, PendingUserRequest> {
    fn gc_pending_map(&mut self) {
        *self = take(self)
            .into_iter()
            .filter(|(_, v)| Self::pending_still_valid(&v.age))
            .collect();
    }

    fn check_contains(&mut self, user_id: DiscordId) -> bool {
        match self.entry(user_id) {
            Entry::Occupied(e) => {
                let e = Self::pending_still_valid(&e.get().age);
                if !e {
                    trace!("UserReq[{}]M - Cache timed out.", user_id);
                }
                e
            }
            Entry::Vacant(_) => false,
        }
    }

    fn pending_insert(&mut self, user_id: DiscordId) {
        self.insert(
            user_id,
            PendingUserRequest {
                age: Utc::now(),
                pending_requests: 0,
                response: None,
            },
        );
    }

    fn pending_increase(&mut self, user_id: DiscordId) {
        self.get_mut(&user_id).as_mut().unwrap().pending_requests += 1;
    }

    fn take_pending_response(&mut self, user_id: DiscordId) -> Option<DiscordUserProfileResult> {
        let mut new_count = 999;
        let mut resp = None;
        if let Some(PendingUserRequest {
            pending_requests,
            response: Some(response),
            ..
        }) = self.get_mut(&user_id)
        {
            if *pending_requests > 0 {
                *pending_requests -= 1;
            }
            new_count = *pending_requests;
            resp = Some(response.clone());
        }
        if new_count == 0 && !Self::pending_still_valid(&self.get(&user_id).unwrap().age) {
            self.remove(&user_id);
        }
        resp
    }

    fn pending_stop_waiting(&mut self, user_id: DiscordId) {
        let mut new_count = 999;
        if let Some(PendingUserRequest {
            pending_requests, ..
        }) = self.get_mut(&user_id)
        {
            if *pending_requests > 0 {
                *pending_requests -= 1;
            }
            new_count = *pending_requests;
        }
        if new_count == 0 && !Self::pending_still_valid(&self.get(&user_id).unwrap().age) {
            self.remove(&user_id);
        }
    }

    fn place_pending_response(
        &mut self,
        user_id: DiscordId,
        the_response: DiscordUserProfileResult,
    ) {
        if let Some(PendingUserRequest { response, .. }) = self.get_mut(&user_id) {
            *response = Some(the_response);
        }
    }
}

#[derive(Debug)]
pub struct DiscordBot {
    reporting_sender: Sender<ReportingEvent>,
    user_request_sender: Sender<DiscordId>,
    user_request_answer_receiver: Mutex<Receiver<DiscordUserRequestResult>>,
    pending_user_request_answers: Mutex<HashMap<DiscordId, PendingUserRequest>>,
}

impl DiscordBot {
    pub async fn new(
        client_builder: ClientBuilder,
    ) -> Result<(Self, JoinHandle<serenity::Result<()>>), DiscordSetupError> {
        let (reporting_sender, reporting_receiver) = channel(500);
        let (user_request_sender, user_request_receiver) = channel(3000);
        let (user_request_answer_sender, user_request_answer_receiver) = channel(3000);
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
                user_request_sender,
                user_request_answer_receiver: Mutex::new(user_request_answer_receiver),
                pending_user_request_answers: Mutex::new(HashMap::new()),
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
    pub async fn get_user(&self, user_id: DiscordId) -> DiscordUserProfileResult {
        if self.pre_warm_get_user(user_id).await? {
            trace!("UserReq[{}]M - Already have a pending lookup.", user_id);
        }
        self.pending_user_request_answers
            .lock()
            .await
            .pending_increase(user_id);

        // Check if we maybe have the response in the pending response list.
        if let Some(resp) = self
            .pending_user_request_answers
            .lock()
            .await
            .take_pending_response(user_id)
        {
            trace!("UserReq[{}]M - Done!", user_id);
            return resp;
        }

        let mut max_loop_count = 100;

        let response;
        loop {
            trace!("UserReq[{}]M - Waiting Response...", user_id);

            let mut user_request_answer_receiver = self.user_request_answer_receiver.lock().await;
            let answer_request = timeout(
                std::time::Duration::from_millis(100),
                user_request_answer_receiver.recv(),
            );
            let (response_request_id, lresponse) = match answer_request.await {
                Ok(v) => v,
                Err(_) => {
                    // Check if we maybe have the response in the pending response list.
                    if let Some(resp) = self
                        .pending_user_request_answers
                        .lock()
                        .await
                        .take_pending_response(user_id)
                    {
                        Some((user_id, resp))
                    } else {
                        trace!("UserReq[{}]M - Timeout.", user_id);
                        max_loop_count -= 1;
                        if max_loop_count == 0 {
                            trace!("UserReq[{}]M - Max loop count reached.", user_id);
                            warn!(
                                "Timeout while trying to get Discord user profile for {}.",
                                user_id
                            );
                            self.pending_user_request_answers
                                .lock()
                                .await
                                .pending_stop_waiting(user_id);
                            return Err(ArcedAnyhowError(Arc::new(anyhow!(
                                "Timeout while trying to get Discord user profile for {}.",
                                user_id
                            ))));
                        } else {
                            continue;
                        }
                    }
                }
            }
            .unwrap_or_else(|| {
                (
                    user_id,
                    Err(ArcedAnyhowError(Arc::new(anyhow!(
                        "Discord thread is not available."
                    )))),
                )
            });
            drop(user_request_answer_receiver);

            let mut pending_user_request_answers = self.pending_user_request_answers.lock().await;
            // Put the unexpected response on the pending list.
            pending_user_request_answers
                .place_pending_response(response_request_id, lresponse.clone());

            if response_request_id != user_id {
                // Check if we maybe have the response in the pending response list instead.
                if let Some(resp) = pending_user_request_answers.take_pending_response(user_id) {
                    trace!("UserReq[{}]M - Done!", user_id);
                    return resp;
                }
            } else {
                response = lresponse;
                break;
            }
        }

        self.pending_user_request_answers
            .lock()
            .await
            .gc_pending_map();
        trace!("UserReq[{}]M - Done!", user_id);
        response
    }

    /// Asks the Discord bot to pre-warm the user info for the requested user ID.
    /// This will not actually wait for the bot to send a reply. A future call to get_user() will
    /// collect the response.
    ///
    /// Returns whether or not a request was already pending before
    pub async fn pre_warm_get_user(&self, user_id: DiscordId) -> Result<bool, ArcedAnyhowError> {
        trace!("UserReq[{}]M - Locking...", user_id);
        let mut pending_user_request_answers = self.pending_user_request_answers.lock().await;
        let had_pending = pending_user_request_answers.check_contains(user_id);
        #[allow(clippy::map_entry)]
        if !had_pending {
            trace!("UserReq[{}]M - Sending...", user_id);

            if timeout(
                std::time::Duration::from_millis(200),
                self.user_request_sender.send(user_id),
            )
            .await
            .is_err()
            {
                // oh dear, seems like the send queue is full, wake the other thread up
                // and hope for the best.
                trace!(
                    "UserReq[{}]M - Sending: Reached timeout. Trying to wakeup first...",
                    user_id
                );
                self.reporting_sender
                    .send(ReportingEvent::__Wakeup)
                    .await
                    .ok();
                // Try again
                trace!("UserReq[{}]M - Sending...", user_id);
                timeout(
                    std::time::Duration::from_millis(200),
                    self.user_request_sender.send(user_id),
                )
                .await??;
            }
            self.user_request_sender.send(user_id).await?;

            pending_user_request_answers.pending_insert(user_id);
            drop(pending_user_request_answers);

            trace!("UserReq[{}]M - Sending Wakeup...", user_id);
            self.reporting_sender
                .send(ReportingEvent::__Wakeup)
                .await
                .ok();
        }
        Ok(had_pending)
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
