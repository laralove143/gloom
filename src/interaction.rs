use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
    sync::Arc,
};

use tokio::sync::Mutex;
use twilight_http::client::InteractionClient;
use twilight_model::{
    application::{command::CommandOptionChoice, interaction::Interaction},
    channel::message::{
        component::{ActionRow, TextInput},
        Component, MessageFlags,
    },
    guild::Permissions,
    http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType},
    id::{marker::InteractionMarker, Id},
};

use crate::{error::UserError, reply::Reply, Bot};

/// Extracting data from interactions
pub mod extract;

/// Allows convenient interaction-related methods
///
/// Created from [`Bot::interaction_handle`]
#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct InteractionHandle<'bot> {
    /// The bot data to make requests with
    bot: &'bot Bot,
    /// The interaction's ID
    id: Id<InteractionMarker>,
    /// The interaction's token
    token: String,
    /// The bot's permissions
    app_permissions: Permissions,
    /// Whether the interaction was already responded to
    responded: Arc<Mutex<bool>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::module_name_repetitions)]
/// An error returned by the crate when responding to interactions
pub enum InteractionError {
    /// A response that has to be the first was called on a responded
    /// interaction
    AlreadyResponded,
}

impl Display for InteractionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("a response that has to be the first was called on a responded interaction")
    }
}

impl Error for InteractionError {}

impl Bot {
    /// Return an interaction's handle
    #[must_use]
    pub fn interaction_handle(&self, interaction: &Interaction) -> InteractionHandle<'_> {
        InteractionHandle {
            bot: self,
            id: interaction.id,
            token: interaction.token.clone(),
            app_permissions: interaction.app_permissions.unwrap_or(Permissions::all()),
            responded: Arc::new(Mutex::new(false)),
        }
    }

    /// Return the interaction client for this bot
    #[must_use]
    pub const fn interaction_client(&self) -> InteractionClient<'_> {
        self.http.interaction(self.application.id)
    }
}

impl InteractionHandle<'_> {
    /// Check that the bot has the required permissions
    ///
    /// Always returns `Ok` in DM channels, make sure the command can actually
    /// run in DMs
    ///
    /// # Errors
    ///
    /// Returns [`UserError::MissingPermissions`] if the bot doesn't have the
    /// required permissions, the wrapped permissions are the permissions
    /// the bot is missing
    pub fn check_permissions(&self, required_permissions: Permissions) -> Result<(), UserError> {
        let missing_permissions = required_permissions - self.app_permissions;
        if !missing_permissions.is_empty() {
            return Err(UserError::MissingPermissions(Some(missing_permissions)));
        }

        Ok(())
    }

    /// Defer the interaction
    ///
    /// The `ephemeral` parameter only affects the first [`Self::reply`]
    ///
    /// # Errors
    ///
    /// Returns [`InteractionError::AlreadyResponded`] if this is not the first
    /// response to the interaction
    ///
    /// Returns [`twilight_http::error::Error`] if deferring the interaction
    /// fails
    pub async fn defer(&self, ephemeral: bool) -> Result<(), anyhow::Error> {
        let mut responded = self.responded.lock().await;

        if *responded {
            return Err(InteractionError::AlreadyResponded.into());
        }

        let defer_response = InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: Some(InteractionResponseData {
                flags: ephemeral.then_some(MessageFlags::EPHEMERAL),
                ..Default::default()
            }),
        };

        self.bot
            .interaction_client()
            .create_response(self.id, &self.token, &defer_response)
            .await?;

        *responded = true;

        Ok(())
    }

    /// Reply to this command
    ///
    /// Discord gives 3 seconds of deadline to respond to an interaction, if the
    /// reply might take longer, consider using [`Self::defer`] before this
    /// method
    ///
    /// # Errors
    ///
    /// Returns an error if the reply is invalid (Refer to
    /// [`twilight_http::request::application::interaction::CreateFollowup`])
    ///
    /// Returns [`twilight_http::error::Error`] if creating the followup
    /// response fails
    pub async fn reply(&self, reply: Reply) -> Result<(), anyhow::Error> {
        let mut responded = self.responded.lock().await;

        if *responded {
            let client = self.bot.interaction_client();
            let mut followup = client.create_followup(&self.token);

            if !reply.content.is_empty() {
                followup = followup.content(&reply.content)?;
            }
            if let Some(allowed_mentions) = &reply.allowed_mentions {
                followup = followup.allowed_mentions(allowed_mentions.as_ref());
            }

            followup
                .embeds(&reply.embeds)?
                .components(&reply.components)?
                .attachments(&reply.attachments)?
                .flags(reply.flags)
                .tts(reply.tts)
                .await?;
        } else {
            self.bot
                .interaction_client()
                .create_response(
                    self.id,
                    &self.token,
                    &InteractionResponse {
                        kind: InteractionResponseType::ChannelMessageWithSource,
                        data: Some(InteractionResponseData {
                            content: Some(reply.content),
                            embeds: Some(reply.embeds),
                            components: Some(reply.components),
                            attachments: Some(reply.attachments),
                            flags: Some(reply.flags),
                            tts: Some(reply.tts),
                            allowed_mentions: reply.allowed_mentions.flatten(),
                            choices: None,
                            custom_id: None,
                            title: None,
                        }),
                    },
                )
                .await?;

            *responded = true;
        }

        Ok(())
    }

    /// # Deprecated
    ///
    /// This function simply calls [`Self::reply`], which is safe to use both as
    /// a first reply and a followup, it will be removed at the next major
    /// version
    ///
    /// # Errors
    ///
    /// Refer to the documentation of [`Self::reply`]
    #[deprecated = "This function simply calls `Self::reply`, which is safe to use both as a first \
                    reply and a followup, it will be removed at the next major version"]
    pub async fn followup(&self, reply: Reply) -> Result<(), anyhow::Error> {
        self.reply(reply).await?;

        Ok(())
    }

    /// Respond to this command with autocomplete suggestions
    ///
    /// # Errors
    ///
    /// Returns [`InteractionError::AlreadyResponded`] if this is not the first
    /// response to the interaction
    ///
    /// Returns [`twilight_http::error::Error`] if creating the response fails
    pub async fn autocomplete(
        &self,
        choices: Vec<CommandOptionChoice>,
    ) -> Result<(), anyhow::Error> {
        let mut responded = self.responded.lock().await;

        if *responded {
            return Err(InteractionError::AlreadyResponded.into());
        }

        self.bot
            .interaction_client()
            .create_response(
                self.id,
                &self.token,
                &InteractionResponse {
                    kind: InteractionResponseType::ApplicationCommandAutocompleteResult,
                    data: Some(InteractionResponseData {
                        choices: Some(choices),
                        allowed_mentions: None,
                        attachments: None,
                        components: None,
                        content: None,
                        custom_id: None,
                        embeds: None,
                        flags: None,
                        title: None,
                        tts: None,
                    }),
                },
            )
            .await?;

        *responded = true;

        Ok(())
    }

    /// Respond to this command with a modal
    ///
    /// # Errors
    ///
    /// Returns [`InteractionError::AlreadyResponded`] if this is not the first
    /// response to the interaction
    ///
    /// Returns [`twilight_http::error::Error`] if creating the response fails
    pub async fn modal(
        &self,
        custom_id: String,
        title: String,
        text_inputs: Vec<TextInput>,
    ) -> Result<(), anyhow::Error> {
        let mut responded = self.responded.lock().await;

        if *responded {
            return Err(InteractionError::AlreadyResponded.into());
        }

        self.bot
            .interaction_client()
            .create_response(
                self.id,
                &self.token,
                &InteractionResponse {
                    kind: InteractionResponseType::Modal,
                    data: Some(InteractionResponseData {
                        custom_id: Some(custom_id),
                        title: Some(title),
                        components: Some(
                            text_inputs
                                .into_iter()
                                .map(|text_input| {
                                    Component::ActionRow(ActionRow {
                                        components: vec![Component::TextInput(text_input)],
                                    })
                                })
                                .collect(),
                        ),
                        allowed_mentions: None,
                        attachments: None,
                        choices: None,
                        content: None,
                        embeds: None,
                        flags: None,
                        tts: None,
                    }),
                },
            )
            .await?;

        *responded = true;

        Ok(())
    }
}
