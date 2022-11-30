use anyhow::anyhow;
use bitflags::bitflags;
use serde::{Serialize, Deserialize };
use num::{ FromPrimitive };
use num_derive::{ToPrimitive, FromPrimitive};

#[derive(Serialize, Deserialize)]
pub struct GatewayHello {
    pub heartbeat_interval: i64
}

#[derive(Serialize)]
pub struct GatewayProperties {
    pub os: String,
    pub browser: String,
    pub device: String
}

#[derive(Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub discriminator: String
}

#[derive(Deserialize)]
pub struct DiscordMember {
    pub user: Option<DiscordUser>
}

#[derive(Deserialize)]
pub struct Integration {
    pub id: String,
    pub token: String,
    pub member: Option<DiscordMember>,
    pub channel_id: String
}

pub struct GatewayDispatchRaw {
    pub event: String,
    pub value: serde_json::Value
}

pub enum GatewayDispatch {
    InteractionCreate(Integration)
}

impl GatewayDispatchRaw {
    pub fn parse(&self) -> anyhow::Result<GatewayDispatch> {
        Ok(match &self.event as &str {
            "INTERACTION_CREATE" => {
                GatewayDispatch::InteractionCreate(serde_json::from_value(self.value.clone())?)
            }
            _ => return Err(anyhow!("Unhandled event code {}.", self.event))
        })
    }
}

bitflags! {
    #[derive(Serialize)]
    pub struct GatewayIntents: u32 {
        const GUILDS = 1 << 0;
        const GUILD_MESSAGES = 1 << 9;
        const GUILD_MESSAGE_REACTIONS = 1 << 10;
        const MESSAGE_CONTENT = 1 << 15;
    }
}

impl GatewayIntents {
    pub fn get(&self) -> u32 {
        return self.bits;
    }
}

#[derive(Serialize)]
pub struct GatewayIdentify {
    pub token: String,
    pub intents: u32,
    pub properties: GatewayProperties
}

#[derive(ToPrimitive, FromPrimitive)]
pub enum GatewayOp {
    Dispatch = 0,
    Heartbeat = 1,
    Identify = 2,
    Hello = 10,
    HeartbeatAck = 11,
}

#[derive(Serialize, Deserialize)]
pub struct GatewayMessageRaw {
    pub op: i32,
    #[serde(rename = "s")]
    pub sequence: Option<i32>,
    #[serde(rename = "t")]
    pub event: Option<String>,
    #[serde(rename = "d")]
    pub data: serde_json::Value
}

pub enum GatewayMessage {
    Dispatch(GatewayDispatchRaw),
    Heartbeat,
    Identify,
    Hello(GatewayHello),
    HeartbeatAck,
}

impl GatewayMessageRaw {
    pub fn data<T>(mut self, data: T) -> Result<GatewayMessageRaw, serde_json::Error>
        where T: serde::Serialize {
        self.data = serde_json::to_value(data)?;

        Ok(self)
    }

    pub fn parse(self) -> anyhow::Result<GatewayMessage> {
        let Some(value) = FromPrimitive::from_i32(self.op) as Option<GatewayOp> else {
            return Err(anyhow!("Unknown op-code {}.", self.op));
        };

        Ok(match value {
            GatewayOp::Dispatch => {
                let Some(event) = self.event else {
                    return Err(anyhow!("Missing event for dispatch event."));
                };

                GatewayMessage::Dispatch(GatewayDispatchRaw { event, value: self.data })
            },
            GatewayOp::Heartbeat => GatewayMessage::Heartbeat,
            GatewayOp::Identify => GatewayMessage::Identify,
            GatewayOp::Hello => {
                GatewayMessage::Hello(serde_json::from_value(self.data)?)
            },
            GatewayOp::HeartbeatAck => GatewayMessage::HeartbeatAck
        })
    }
}
