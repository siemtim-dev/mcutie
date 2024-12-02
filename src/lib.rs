#![no_std]
#![doc = include_str!("../README.md")]

use core::{fmt::Display, ops::Deref, str};

use embassy_futures::select::{select, Either};
use embassy_net::{HardwareAddress, Stack};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::WaitResult,
};
use embassy_time::Timer;
use heapless::String;
pub use mqttrs::QoS;
use mqttrs::{Packet, Pid, Publish, QosPid, SubscribeReturnCodes};
use once_cell::sync::OnceCell;

pub use buffer::Buffer;
use io::send_packet;
pub use io::McutieTask;
pub use topic::Topic;

use crate::io::{assign_pid, subscribe};

mod buffer;
#[cfg(feature = "homeassistant")]
pub mod homeassistant;
mod io;
mod topic;

// This really needs to match that used by mqttrs.
const TOPIC_LENGTH: usize = 256;
const PAYLOAD_LENGTH: usize = 2048;

pub type TopicString = String<TOPIC_LENGTH>;
pub type Payload = Buffer<PAYLOAD_LENGTH>;

// By default in the event of an error connecting to the broker we will wait for 5s.
const DEFAULT_BACKOFF: u64 = 5000;
// If the connection dropped then re-connect more quickly.
const RESET_BACKOFF: u64 = 200;
// How long to wait for the broker to confirm actions.
const CONFIRMATION_TIMEOUT: u64 = 2000;

static DATA_CHANNEL: Channel<CriticalSectionRawMutex, MqttMessage, 10> = Channel::new();

static DEVICE_TYPE: OnceCell<String<32>> = OnceCell::new();
static DEVICE_ID: OnceCell<String<32>> = OnceCell::new();

fn device_id() -> &'static str {
    DEVICE_ID.get().unwrap()
}

fn device_type() -> &'static str {
    DEVICE_TYPE.get().unwrap()
}

/// Various errors
pub enum Error {
    IOError,
    TimedOut,
    TooLarge,
    PacketError,
}

#[allow(clippy::large_enum_variant)]
pub enum MqttMessage {
    /// The broker has been connected to successfully. Generally in response to this message a
    /// device should subscribe to topics of interest and send out any device state.
    Connected,
    /// New data received from the broker.
    Publish(Topic<TopicString>, Payload),
    /// The connection to the broker has been dropped.
    Disconnected,
    /// Home Assistant has come online and you should send any discovery messages.
    #[cfg(feature = "homeassistant")]
    HomeAssistantOnline,
}

#[derive(Clone)]
enum ControlMessage {
    Published(Pid),
    Subscribed(Pid, SubscribeReturnCodes),
    Unsubscribed(Pid),
}

pub trait Publishable {
    type TopicError;
    type PayloadError;

    fn write_topic(&self, topic: &mut TopicString) -> Result<(), Self::TopicError>;

    fn write_payload(&self, payload: &mut Payload) -> Result<(), Self::PayloadError>;
}

pub struct State<T: Deref<Target = str>, S: Display> {
    pub topic: Topic<T>,
    pub state: S,
}

impl<T: Deref<Target = str>, S: Display> Publishable for State<T, S> {
    type TopicError = ();
    type PayloadError = core::fmt::Error;

    fn write_topic(&self, topic: &mut TopicString) -> Result<(), Self::TopicError> {
        self.topic.to_string(topic)
    }

    fn write_payload(&self, payload: &mut Payload) -> Result<(), Self::PayloadError> {
        use core::fmt::Write;

        write!(payload, "{}", self.state)
    }
}

pub struct Receiver;

impl Receiver {
    pub async fn receive(&self) -> MqttMessage {
        DATA_CHANNEL.receive().await
    }
}

#[derive(Clone)]
pub struct Mcutie {}

impl Mcutie {
    /// A receiver for messages about state and publications from the broker. While you can obtain
    /// multiple receivers only one will receive a message.
    pub fn receiver(&self) -> Receiver {
        Receiver
    }

    async fn publish_packet(
        &self,
        topic_name: &str,
        payload: &[u8],
        qos: QoS,
    ) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let (qospid, pid) = match qos {
            QoS::AtMostOnce => (QosPid::AtMostOnce, None),
            QoS::AtLeastOnce => {
                let pid = assign_pid();
                (QosPid::AtLeastOnce(pid), Some(pid))
            }
            QoS::ExactlyOnce => {
                let pid = assign_pid();
                (QosPid::ExactlyOnce(pid), Some(pid))
            }
        };

        let packet = Packet::Publish(Publish {
            dup: false,
            qospid,
            retain: false,
            topic_name,
            payload,
        });

        send_packet(packet).await?;

        if let Some(expected_pid) = pid {
            match select(
                async {
                    loop {
                        match subscriber.next_message().await {
                            WaitResult::Lagged(_) => {
                                // Maybe we missed the message?
                            }
                            WaitResult::Message(ControlMessage::Published(published_pid)) => {
                                if published_pid == expected_pid {
                                    return Ok(());
                                }
                            }
                            _ => {}
                        }
                    }
                },
                Timer::after_millis(CONFIRMATION_TIMEOUT),
            )
            .await
            {
                Either::First(r) => r,
                Either::Second(_) => Err(Error::TimedOut),
            }
        } else {
            Ok(())
        }
    }

    pub async fn publish<P: Publishable>(&self, data: &P, qos: QoS) -> Result<(), Error> {
        let mut topic_str = TopicString::new();
        if data.write_topic(&mut topic_str).is_err() {
            return Err(Error::PacketError);
        }

        let mut buffer = Payload::new();
        if data.write_payload(&mut buffer).is_err() {
            return Err(Error::PacketError);
        }

        self.publish_packet(&topic_str, buffer.as_ref(), qos).await
    }

    /// Publishes a slice of data to the MQTT broker.
    ///
    /// If the level is anything other than `QoS::AtMostOnce` then this will wait until the broker
    /// has confirmed that it has received the message.
    pub async fn publish_data<T, P>(
        &self,
        topic: &Topic<T>,
        payload: &P,
        qos: QoS,
    ) -> Result<(), Error>
    where
        T: Deref<Target = str>,
        P: AsRef<[u8]> + ?Sized,
    {
        let mut topic_str = TopicString::new();
        if topic.to_string(&mut topic_str).is_err() {
            return Err(Error::TooLarge);
        }

        self.publish_packet(&topic_str, payload.as_ref(), qos).await
    }
}

/// A builder to configure the MQTT stack.
pub struct McutieBuilder<'t, T, L, const S: usize>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    network: Stack<'t>,
    device_type: &'t str,
    device_id: Option<&'t str>,
    broker: &'t str,
    last_will: Option<L>,
    username: Option<&'t str>,
    password: Option<&'t str>,
    subscriptions: [Topic<T>; S],
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't> McutieBuilder<'t, T, L, 0> {
    /// Creates a new builder with the initial required configuration.
    ///
    /// `device_type` is expected to be the same for all devices of the same type.
    /// `broker` may be an IP address or a DNS name for the broker to connect to.
    pub fn new(network: Stack<'t>, device_type: &'t str, broker: &'t str) -> Self {
        Self {
            network,
            device_type,
            broker,
            device_id: None,
            last_will: None,
            username: None,
            password: None,
            subscriptions: [],
        }
    }
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't, const S: usize>
    McutieBuilder<'t, T, L, S>
{
    /// Add some default topics to subscribe to.
    pub fn with_subscriptions<const N: usize>(
        self,
        subscriptions: [Topic<T>; N],
    ) -> McutieBuilder<'t, T, L, N> {
        McutieBuilder {
            network: self.network,
            device_type: self.device_type,
            broker: self.broker,
            device_id: self.device_id,
            last_will: self.last_will,
            username: self.username,
            password: self.password,
            subscriptions,
        }
    }
}

impl<'t, T: Deref<Target = str> + 't, L: Publishable + 't, const S: usize>
    McutieBuilder<'t, T, L, S>
{
    /// Adds authentication for the broker.
    pub fn with_authentication(self, username: &'t str, password: &'t str) -> Self {
        Self {
            username: Some(username),
            password: Some(password),
            ..self
        }
    }

    /// Sets a last will message to be published in the event of disconnection.
    pub fn with_last_will(self, last_will: L) -> Self {
        Self {
            last_will: Some(last_will),
            ..self
        }
    }

    /// Sets a custom unique device identifier. If none is set then the network
    /// MAC address is used.
    pub fn with_device_id(self, device_id: &'t str) -> Self {
        Self {
            device_id: Some(device_id),
            ..self
        }
    }

    /// Initialises the MQTT stack returning a struct that can be used to send
    /// and receive messages and a future that must be run in order for the
    /// stack to operate.
    pub fn build(self) -> (Mcutie, McutieTask<'t, T, L, S>) {
        let mut dtype = String::<32>::new();
        dtype.push_str(self.device_type).unwrap();
        DEVICE_TYPE.set(dtype).unwrap();

        let mut did = String::<32>::new();
        if let Some(device_id) = self.device_id {
            did.push_str(device_id).unwrap();
        } else if let HardwareAddress::Ethernet(address) = self.network.hardware_address() {
            let mut buffer = [0_u8; 12];
            hex::encode_to_slice(address.as_bytes(), &mut buffer).unwrap();
            did.push_str(str::from_utf8(&buffer).unwrap()).unwrap();
        }

        DEVICE_ID.set(did).unwrap();

        (
            Mcutie {},
            McutieTask {
                network: self.network,
                broker: self.broker,
                last_will: self.last_will,
                username: self.username,
                password: self.password,
                subscriptions: self.subscriptions,
            },
        )
    }
}
