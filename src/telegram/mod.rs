mod commands;
mod handler;
mod membership;
mod moderation;
mod rate_limit;
mod state;
mod utils;

pub(crate) type BotResult<T> = Result<T, teloxide::RequestError>;

pub(crate) use handler::TelegramService;
pub(crate) use moderation::TelegramMessageModerationGateway;
