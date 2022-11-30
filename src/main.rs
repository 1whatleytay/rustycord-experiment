mod messages;

use std::borrow::Borrow;
use std::cmp::max;
use std::collections::HashMap;
use std::fs;
use std::net::TcpStream;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use num::FromPrimitive;
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use serde::Serialize;
use anyhow::{Result, anyhow};
use num_traits::ToPrimitive;
use serde_json::Value;
use serde_json::Value::Null;
use crate::messages::{
    GatewayDispatch,
    GatewayHello,
    GatewayIdentify,
    GatewayIntents,
    GatewayMessage,
    GatewayMessageRaw,
    GatewayOp,
    GatewayProperties,
    Integration
};

#[derive(Deserialize)]
struct Config {
    token: String,
    commands: Vec<Value>
}

impl Config {
    pub fn new(path: &str) -> Option<Config> {
        let to_config = |text: String| Config::from(&text);

        fs::read_to_string(path).ok().map_or(None, to_config)
    }

    pub fn from(string: &str) -> Option<Config> {
        serde_json::from_str(string.borrow()).ok()
    }

    pub fn default() -> Option<Config> {
        return Config::new("config.json");
    }
}

#[derive(Deserialize)]
struct GatewayConnection {
    url: String
}

struct Discord {
    base_url: Url,
    token: String,
    client: reqwest::Client
}

impl Discord {
    fn url(&self, to: &str) -> Option<Url> {
        self.base_url.join(to).ok()
    }

    fn headers_with_token(token: &str) -> HeaderMap {
        let auth = format!("Bot {}", token);

        HeaderValue::from_str(&auth).ok()
            .map_or(None, |value|
                (&HashMap::from([("Authorization".to_string(), value)])).try_into().ok())
            .unwrap_or_else(|| HeaderMap::default())
    }

    pub fn new(base_url: &str, token: String) -> Option<Discord> {
        let headers = Discord::headers_with_token(&token);

        let Some(url) = Url::parse(base_url).ok() else { return None };
        let Some(client) = reqwest::ClientBuilder::new()
            .default_headers(headers)
            .build()
            .ok() else { return None };

        Some(Discord { base_url: url, token, client })
    }

    pub fn default(token: String) -> Option<Discord> {
        Discord::new("https://discord.com/api/v10/", token)
    }

    pub async fn create_command(&self, command: Value) -> Result<()> {
        let Some(url) = self.url("applications/1043405013793382430/commands") else {
            return Err(anyhow!("Url Failed to Parse"));
        };

        let bytes = serde_json::to_vec(&command)?;
        println!("Sending: {}", String::from_utf8(bytes.clone()).unwrap());

        let response = self.client.post(url)
            .header("Content-Type", "application/json")
            .body(bytes).send().await?;
        println!("Response: {}", String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap());

        Ok(())
    }

    pub async fn connect(self) -> Result<DiscordGateway> {
        let Some(url) = self.url("gateway/bot") else {
            return Err(anyhow!("Url Failed to Parse"))
        };

        let response = self.client.get(url).send().await?;
        let gateway: GatewayConnection = response.json().await?;
        let gateway_url = Url::parse(&gateway.url)?;

        let mut gateway_url = gateway_url.clone();

        gateway_url.query_pairs_mut()
            .append_pair("v", "10")
            .append_pair("encoding", "json");

        let (mut client, _) = tungstenite::connect(gateway_url.to_string())?;

        let hello = client.read_message()?;
        println!("{}", hello.to_string());

        let gateway_message: GatewayMessageRaw = serde_json::from_slice(&hello.into_data())?;

        match FromPrimitive::from_i32(gateway_message.op) {
            Some(GatewayOp::Hello) => { },
            _ => { return Err(anyhow!("Initiating event is not a Hello event.")); }
        }

        let hello_data: GatewayHello = serde_json::from_value(gateway_message.data)?;

        // let jitter = rand::random::<f64>();
        let jitter = 0.0;
        let heartbeat = hello_data.heartbeat_interval as u64;
        let wait = ((heartbeat as f64) * jitter) as u64;

        println!("Waiting {} milliseconds before heartbeat...", wait);

        let duration = Duration::from_millis(wait);
        tokio::time::sleep(duration).await;

        let mut gateway = DiscordGateway {
            // connect_url: gateway_url,
            discord: self,
            socket: client,
            heartbeat: hello_data.heartbeat_interval,
            has_identified: false,
            sequence: None
        };

        gateway.send_heartbeat();

        Ok(gateway)
    }
}

struct DiscordGateway {
    discord: Discord,
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    heartbeat: i64,
    has_identified: bool,
    sequence: Option<i32>
}

#[derive(Serialize)]
struct MessageResponse {
    content: String
}

#[derive(Serialize)]
struct IntegrationResponse {
    #[serde(rename="type")]
    response_type: i32,
    data: Value
}

impl DiscordGateway {
    pub fn send(&mut self, value: &GatewayMessageRaw) {
        if let Ok(text) = serde_json::to_string(value) {
            if let Err(_) = self.socket.write_message(Message::from(text)) {
                println!("Failed to send message.");
            }
        } else {
            println!("Failed to serialize object.");
        }
    }

    pub fn read(&mut self) -> Result<GatewayMessage> {
        let message = self.socket.read_message()?;
        let text = message.to_string();
        let raw: GatewayMessageRaw = serde_json::from_slice(message.into_data().as_slice())?;

        println!("Received: {}", text);

        if let Some(sequence) = raw.sequence {
            self.sequence = Some(sequence)
        }

        raw.parse()
    }

    pub fn send_heartbeat(&mut self) {
        let heartbeat_message = GatewayMessageRaw {
            op: ToPrimitive::to_i32(&GatewayOp::Heartbeat).unwrap(),
            sequence: self.sequence,
            event: None,
            data: Default::default()
        };

        self.send(&heartbeat_message);
    }

    pub async fn make_heartbeat(mutex: Weak<Mutex<DiscordGateway>>) {
        if let Some(mutex) = mutex.upgrade() {
            let heartbeat = max(0, mutex.lock().unwrap().heartbeat as u64 - 2000);
            println!("Got heartbeat ACK, waiting {} milliseconds...", heartbeat);

            tokio::time::sleep(Duration::from_millis(heartbeat)).await;
        } else {
            return;
        }

        if let Some(mutex) = mutex.upgrade() {
            mutex.lock().unwrap().send_heartbeat()
        }
    }

    pub async fn handle_integration(&mut self, integration: Integration) -> Result<()> {
        println!("Got integration event!");

        {
            // let responseType = "CHANNEL_MESSAGE_WITH_SOURCE";
            let message_response = 4;

            let url_end = format!(
                "interactions/{}/{}/callback",
                integration.id, integration.token
            );

            let response = IntegrationResponse {
                response_type: message_response,
                data: serde_json::to_value(MessageResponse {
                    content: format!("Nice stuff {}, this is channel {}", integration.member
                        .map_or(None, |m| m.user)
                        .map(|u| u.username)
                        .unwrap_or("user guy".to_string()),
                        integration.channel_id
                    )
                })?,
            };

            let url = self.discord.url(&url_end).unwrap();

            let value = self.discord.client.post(url).json(&response).send().await?;

            println!(
                "From integration response: {}",
                String::from_utf8(value.bytes().await?.to_vec())?
            );
        }

        Ok(())
    }

    pub async fn handle(mutex: &Arc<Mutex<DiscordGateway>>, message: &GatewayMessage) {
        match message {
            GatewayMessage::Dispatch(dispatch) => {
                let dispatch = match dispatch.parse() {
                    Ok(dispatch) => dispatch,
                    Err(error) => {
                        println!("Failed to parse raw dispatch event {} with error {}.", dispatch.event, error);

                        return
                    }
                };

                match dispatch {
                    GatewayDispatch::InteractionCreate(integration) => {
                        mutex.lock().unwrap().handle_integration(integration).await.unwrap()
                    }
                }
            }
            GatewayMessage::Heartbeat => {}
            GatewayMessage::Identify => {}
            GatewayMessage::Hello(_) => {
                panic!("What the heck! Double hello!");
            }
            GatewayMessage::HeartbeatAck => {
                println!("Ack!");

                {
                    let mut gateway = mutex.lock().unwrap();

                    if !gateway.has_identified {
                        gateway.has_identified = true;

                        let token = gateway.discord.token.clone();

                        gateway.identify(&token).unwrap();
                    }
                }

                let weak = Arc::downgrade(mutex);

                tokio::spawn(async move {
                    DiscordGateway::make_heartbeat(weak).await;
                });
            }
        }
    }

    pub fn identify(&mut self, token: &str) -> Result<(), serde_json::Error> {
        let device = "rcord".to_string();

        println!("Sending identify...");
        let message = GatewayMessageRaw {
            op: ToPrimitive::to_i32(&GatewayOp::Identify).unwrap(),
            sequence: None,
            event: None,
            data: Null,
        }
            .data(GatewayIdentify {
                token: token.to_string(),
                intents: (GatewayIntents::MESSAGE_CONTENT
                    | GatewayIntents::GUILD_MESSAGES
                    | GatewayIntents::GUILDS).get(),
                properties: GatewayProperties {
                    os: std::env::consts::OS.to_string(),
                    browser: device.clone(),
                    device
                }
            })?;

        println!("text: {}", serde_json::to_string(&message.data).unwrap());

        self.send(&message);

        println!("Sent!");

        return Ok(())
    }
}

async fn dispatch() -> Option<String> {
    let config = Config::default().unwrap();
    let discord = Discord::default(config.token).unwrap();

    for command in config.commands {
        discord.create_command(command).await.unwrap();
    }

    let mutex = Arc::new(Mutex::new(discord.connect().await.unwrap()));

    loop {
        let message = if let Ok(mut gateway) = mutex.lock() {
            if !gateway.socket.can_read() {
                break
            }

            let message = gateway.read().unwrap();

            Some(message)
        } else {
            None
        };

        DiscordGateway::handle(&mutex, &message.unwrap()).await;
    }

    None
}

#[tokio::main]
async fn main() {
    let Some(text) = dispatch().await else { return };

    println!("{}", text);
}
