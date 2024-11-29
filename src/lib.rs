#![no_std]
#![doc = include_str!("../README.md")]

use core::sync::atomic::{AtomicU16, Ordering};

use defmt::{debug, error, info, trace, warn, Debug2Format};
use embassy_futures::select::{select, select3, Either};
use embassy_net::{
    dns::DnsQueryType,
    tcp::{TcpReader, TcpSocket, TcpWriter},
    Stack,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
    mutex::Mutex,
    pubsub::{PubSubChannel, Subscriber, WaitResult},
    signal::Signal,
};
use embassy_time::Timer;
use embedded_io_async::Write;
use heapless::{String, Vec};
pub use mqttrs::QoS;
use mqttrs::{
    decode_slice,
    Connect,
    ConnectReturnCode,
    LastWill,
    Packet,
    Pid,
    Protocol,
    Publish,
    QosPid,
    Subscribe,
    SubscribeReturnCodes,
    SubscribeTopic,
    Unsubscribe,
};

pub use crate::buffer::Buffer;

mod buffer;
#[cfg(feature = "homeassistant")]
mod homeassistant;

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

static WRITE_BUFFER: Mutex<CriticalSectionRawMutex, Buffer<4096>> = Mutex::new(Buffer::new());
static WRITE_PENDING: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static WRITE_COMPLETE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static DATA_CHANNEL: Channel<CriticalSectionRawMutex, MqttMessage, 10> = Channel::new();
static CONTROL_CHANNEL: PubSubChannel<CriticalSectionRawMutex, ControlMessage, 2, 5, 1> =
    PubSubChannel::new();

static PID: AtomicU16 = AtomicU16::new(0);

async fn subscribe() -> Subscriber<'static, CriticalSectionRawMutex, ControlMessage, 2, 5, 1> {
    loop {
        if let Ok(sub) = CONTROL_CHANNEL.subscriber() {
            return sub;
        }

        Timer::after_millis(50).await;
    }
}

/// Various errors
pub enum Error {
    IOError,
    TimedOut,
    TooLarge,
    PacketError,
}

/// An MQTT topic that is optionally prefixed with the device type and unique ID. This allows the
/// topic to be easily defined as a const before knowing what the device ID is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Topic<T: AsRef<str>> {
    /// A topic that is prefixed with the device type.
    DeviceType(T),
    /// A topic that is prefixed with the device type and unique ID.
    Device(T),
    /// Any topic.
    General(T),
}

impl Topic<TopicString> {
    fn from_str<'a>(device_type: &'a str, device_id: &'a str, mut st: &'a str) -> Result<Self, ()> {
        let is_prefix =
            |pr: &str| -> bool { st.starts_with(pr) && &st[pr.len()..pr.len() + 1] == "/" };

        if is_prefix(device_type) {
            st = &st[device_type.len() + 1..];

            if is_prefix(device_id) {
                st = &st[device_id.len() + 1..];

                let mut topic = TopicString::new();
                topic.push_str(st)?;
                Ok(Topic::Device(topic))
            } else {
                let mut topic = TopicString::new();
                topic.push_str(st)?;
                Ok(Topic::DeviceType(topic))
            }
        } else {
            let mut topic = TopicString::new();
            topic.push_str(st)?;
            Ok(Topic::General(topic))
        }
    }
}

impl<T: AsRef<str>> Topic<T> {
    fn to_string<const N: usize>(
        &self,
        device_type: &str,
        device_id: &str,
        result: &mut String<N>,
    ) -> Result<(), ()> {
        match self {
            Topic::Device(st) => {
                result.push_str(device_type)?;
                result.push_str("/")?;
                result.push_str(device_id)?;
                result.push_str("/")?;
                result.push_str(st.as_ref())?;
            }
            Topic::DeviceType(st) => {
                result.push_str(device_type)?;
                result.push_str("/")?;
                result.push_str(st.as_ref())?;
            }
            Topic::General(st) => {
                result.push_str(st.as_ref())?;
            }
        }

        Ok(())
    }
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
}

#[derive(Clone)]
enum ControlMessage {
    Published(Pid),
    Subscribed(Pid, SubscribeReturnCodes),
    Unsubscribed(Pid),
}

fn packet_size(buffer: &[u8]) -> Option<usize> {
    let mut pos = 1;
    let mut multiplier = 1;
    let mut value = 0;

    while pos < buffer.len() {
        value += (buffer[pos] & 127) as usize * multiplier;
        multiplier *= 128;

        if (buffer[pos] & 128) == 0 {
            return Some(value + pos + 1);
        }

        pos += 1;
        if pos == 5 {
            return Some(0);
        }
    }

    None
}

async fn send_packet(packet: Packet<'_>) -> Result<(), Error> {
    loop {
        trace!("Waiting for data to be written");
        WRITE_COMPLETE.wait().await;

        {
            let mut buffer = WRITE_BUFFER.lock().await;
            trace!("Encoding packet");

            match buffer.encode_packet(&packet) {
                Ok(()) => {
                    trace!("Signaling data ready");
                    WRITE_PENDING.signal(());
                    return Ok(());
                }
                Err(mqttrs::Error::WriteZero) => {}
                Err(_) => {
                    error!("Failed to send packet");
                    return Err(Error::PacketError);
                }
            }
        }
    }
}

/// The MQTT task that must be run in order for the stack to operate.
pub struct McutieTask<'t, T>
where
    T: AsRef<str> + 't,
{
    network: Stack<'t>,
    device_type: &'t str,
    device_id: &'t str,
    broker: &'t str,
    last_will: Option<(Topic<T>, Payload)>,
    username: Option<&'t str>,
    password: Option<&'t str>,
}

impl<'t, T> McutieTask<'t, T>
where
    T: AsRef<str> + 't,
{
    async fn recv_loop(&self, mut reader: TcpReader<'_>) -> Result<(), Error> {
        let mut buffer = [0_u8; 4096];
        let mut cursor: usize = 0;

        let controller = match CONTROL_CHANNEL.publisher() {
            Ok(c) => c,
            Err(_) => {
                panic!("Failed to acquire control channel");
            }
        };

        loop {
            match reader.read(&mut buffer[cursor..]).await {
                Ok(0) => {
                    error!("Receive socket closed");
                    return Ok(());
                }
                Ok(len) => {
                    cursor += len;
                }
                Err(_) => {
                    error!("I/O failure reading packet");
                    return Err(Error::IOError);
                }
            }

            let packet_length = match packet_size(&buffer[0..cursor]) {
                Some(0) => {
                    error!("Invalid MQTT packet");
                    return Err(Error::PacketError);
                }
                Some(len) => len,
                None => {
                    // None is returned when there is not yet enough data to decode a packet.
                    continue;
                }
            };

            let packet = match decode_slice(&buffer[0..packet_length]) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    error!("Packet length calculation failed.");
                    return Err(Error::PacketError);
                }
                Err(_) => {
                    error!("Invalid MQTT packet");
                    return Err(Error::PacketError);
                }
            };

            trace!(
                "Received packet from broker: {}",
                Debug2Format(&packet.get_type())
            );

            match packet {
                Packet::Connack(connack) => match connack.code {
                    ConnectReturnCode::Accepted => {
                        DATA_CHANNEL.send(MqttMessage::Connected).await;
                    }
                    _ => {
                        error!("Connection request to broker was not accepted");
                        return Err(Error::IOError);
                    }
                },
                Packet::Pingresp => {}

                Packet::Publish(publish) => {
                    match (
                        Topic::from_str(self.device_type, self.device_id, publish.topic_name),
                        Payload::from(publish.payload),
                    ) {
                        (Ok(topic), Ok(payload)) => {
                            DATA_CHANNEL
                                .send(MqttMessage::Publish(topic, payload))
                                .await;
                        }
                        _ => {
                            error!("Unable to process publish data as it was too large");
                        }
                    }

                    match publish.qospid {
                        mqttrs::QosPid::AtMostOnce => todo!(),
                        mqttrs::QosPid::AtLeastOnce(pid) => {
                            send_packet(Packet::Puback(pid)).await?;
                        }
                        mqttrs::QosPid::ExactlyOnce(pid) => {
                            send_packet(Packet::Pubrec(pid)).await?;
                        }
                    }
                }
                Packet::Puback(pid) => {
                    controller.publish(ControlMessage::Published(pid)).await;
                }
                Packet::Pubrec(pid) => {
                    controller.publish(ControlMessage::Published(pid)).await;
                    send_packet(Packet::Pubrel(pid)).await?;
                }
                Packet::Pubrel(pid) => send_packet(Packet::Pubrel(pid)).await?,
                Packet::Pubcomp(_) => {}

                Packet::Suback(suback) => {
                    if let Some(return_code) = suback.return_codes.first() {
                        controller
                            .publish(ControlMessage::Subscribed(suback.pid, *return_code))
                            .await;
                    } else {
                        warn!("Unexpected suback with no return codes");
                    }
                }
                Packet::Unsuback(pid) => {
                    controller.publish(ControlMessage::Unsubscribed(pid)).await;
                }

                Packet::Connect(_)
                | Packet::Subscribe(_)
                | Packet::Pingreq
                | Packet::Unsubscribe(_)
                | Packet::Disconnect => {
                    debug!(
                        "Unexpected packet from broker: {}",
                        Debug2Format(&packet.get_type())
                    );
                }
            }

            // Adjust the buffer to reclaim any unused data
            if packet_length == cursor {
                cursor = 0;
            } else {
                buffer.copy_within(packet_length..cursor, 0);
                cursor -= packet_length;
            }
        }
    }

    async fn write_loop(&self, mut writer: TcpWriter<'_>) {
        // Clear out any old data.
        {
            let mut buffer = WRITE_BUFFER.lock().await;
            buffer.reset();
            WRITE_PENDING.reset();

            let mut last_will_topic = TopicString::new();
            let mut last_will_payload = Payload::new();
            let last_will = self.last_will.as_ref().and_then(|(t, p)| {
                if t.to_string(self.device_type, self.device_id, &mut last_will_topic)
                    .is_ok()
                    && embedded_io::Write::write_all(&mut last_will_payload, p).is_ok()
                {
                    Some(LastWill {
                        topic: &last_will_topic,
                        message: &last_will_payload,
                        qos: QoS::AtMostOnce,
                        retain: false,
                    })
                } else {
                    None
                }
            });

            // Send our connection request.
            if buffer
                .encode_packet(&Packet::Connect(Connect {
                    protocol: Protocol::MQTT311,
                    keep_alive: 60,
                    client_id: self.device_id,
                    clean_session: true,
                    last_will,
                    username: self.username,
                    password: self.password.map(|s| s.as_bytes()),
                }))
                .is_err()
            {
                error!("Failed to encode connection packet");
                return;
            }

            if let Err(e) = writer.write_all(&buffer).await {
                error!("Failed to send connection packet: {}", e);
                return;
            }

            buffer.reset();

            WRITE_COMPLETE.signal(());
        }

        loop {
            trace!("Writer waiting for data");
            WRITE_PENDING.wait().await;

            {
                let mut buffer = WRITE_BUFFER.lock().await;
                WRITE_PENDING.reset();
                trace!("Writer locked data");

                if let Err(e) = writer.write_all(&buffer).await {
                    error!("Failed to send data: {}", e);
                    return;
                }

                buffer.reset();
            }

            trace!("Writer signaling completion");
            WRITE_COMPLETE.signal(());
        }
    }

    /// Runs the MQTT stack. The future returns from this must be awaited for everything to work.
    pub async fn run(self) {
        let mut timeout: Option<u64> = None;

        let mut rx_buffer = [0; 4096];
        let mut tx_buffer = [0; 4096];

        loop {
            if let Some(millis) = timeout.replace(DEFAULT_BACKOFF) {
                Timer::after_millis(millis).await;
            }

            if !self.network.is_config_up() {
                trace!("Waiting for network to configure.");
                self.network.wait_config_up().await;
                trace!("Network configured.");
            }

            let ip_addrs = match self.network.dns_query(self.broker, DnsQueryType::A).await {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to lookup '{}' for broker: {}", self.broker, e);
                    continue;
                }
            };

            let ip = match ip_addrs.first() {
                Some(i) => *i,
                None => {
                    error!("No IP address found for broker '{}'", self.broker);
                    continue;
                }
            };

            trace!("Connecting to {}:1883", ip);

            let mut socket = TcpSocket::new(self.network, &mut rx_buffer, &mut tx_buffer);
            if let Err(e) = socket.connect((ip, 1883)).await {
                error!("Failed to connect to {}:1883: {}", ip, e);
                continue;
            }

            info!("Connected to {}", self.broker);
            timeout = Some(RESET_BACKOFF);

            let (reader, writer) = socket.split();

            let recv_loop = self.recv_loop(reader);
            let send_loop = self.write_loop(writer);

            let ping_loop = async {
                loop {
                    Timer::after_secs(45).await;

                    let _ = send_packet(Packet::Pingreq).await;
                }
            };

            select3(send_loop, ping_loop, recv_loop).await;

            socket.close();

            DATA_CHANNEL.send(MqttMessage::Disconnected).await;
        }
    }
}

pub struct Receiver;

impl Receiver {
    pub async fn receive(&self) -> MqttMessage {
        DATA_CHANNEL.receive().await
    }
}

#[derive(Clone)]
pub struct Mcutie<'d> {
    device_type: &'d str,
    device_id: &'d str,
}

impl<'d> Mcutie<'d> {
    fn assign_pid(&self) -> Pid {
        Pid::new() + PID.fetch_add(1, Ordering::SeqCst)
    }

    /// A receiver for messages about state and publications from the broker. While you can obtain
    /// multiple receivers only one will receive a message.
    pub fn receiver(&self) -> Receiver {
        Receiver
    }

    /// Publishes a message with the given QoS level.
    ///
    /// If the level is anything other than `QoS::AtMostOnce` then this will wait until the broker
    /// has confirmed that it has received the message.
    pub async fn publish<T: AsRef<str>, P: AsRef<[u8]> + ?Sized>(
        &self,
        topic: &Topic<T>,
        payload: &P,
        qos: QoS,
    ) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let mut topic_str = TopicString::new();
        if topic
            .to_string(self.device_type, self.device_id, &mut topic_str)
            .is_err()
        {
            return Err(Error::TooLarge);
        }

        let (qospid, pid) = match qos {
            QoS::AtMostOnce => (QosPid::AtMostOnce, None),
            QoS::AtLeastOnce => {
                let pid = self.assign_pid();
                (QosPid::AtLeastOnce(pid), Some(pid))
            }
            QoS::ExactlyOnce => {
                let pid = self.assign_pid();
                (QosPid::ExactlyOnce(pid), Some(pid))
            }
        };

        let packet = Packet::Publish(Publish {
            dup: false,
            qospid,
            retain: false,
            topic_name: &topic_str,
            payload: payload.as_ref(),
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

    /// Subscribes to a topic. If `wait_for_ack` is true then this will wait until confirmation is
    /// received from the broker before returning.
    pub async fn subscribe<T: AsRef<str>>(
        &self,
        topic: &Topic<T>,
        wait_for_ack: bool,
    ) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let mut topic_path = TopicString::new();
        if topic
            .to_string(self.device_type, self.device_id, &mut topic_path)
            .is_err()
        {
            return Err(Error::TooLarge);
        }

        let pid = self.assign_pid();

        let subscribe_topic = SubscribeTopic {
            topic_path,
            qos: QoS::AtLeastOnce,
        };

        let topics = match Vec::<SubscribeTopic, 5>::from_slice(&[subscribe_topic]) {
            Ok(t) => t,
            Err(_) => return Err(Error::TooLarge),
        };

        let packet = Packet::Subscribe(Subscribe { pid, topics });

        send_packet(packet).await?;

        if wait_for_ack {
            match select(
                async {
                    loop {
                        match subscriber.next_message().await {
                            WaitResult::Lagged(_) => {
                                // Maybe we missed the message?
                            }
                            WaitResult::Message(ControlMessage::Subscribed(
                                subscribed_pid,
                                return_code,
                            )) => {
                                if subscribed_pid == pid {
                                    if matches!(return_code, SubscribeReturnCodes::Success(_)) {
                                        return Ok(());
                                    } else {
                                        return Err(Error::IOError);
                                    }
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

    /// Unsubscribes from a topic. If `wait_for_ack` is true then this will wait until confirmation is
    /// received from the broker before returning.
    pub async fn unsubscribe<T: AsRef<str>>(
        &self,
        topic: &Topic<T>,
        wait_for_ack: bool,
    ) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let mut topic_path = TopicString::new();
        if topic
            .to_string(self.device_type, self.device_id, &mut topic_path)
            .is_err()
        {
            return Err(Error::TooLarge);
        }

        let pid = self.assign_pid();

        let topics = match Vec::<TopicString, 5>::from_slice(&[topic_path]) {
            Ok(t) => t,
            Err(_) => return Err(Error::TooLarge),
        };

        let packet = Packet::Unsubscribe(Unsubscribe { pid, topics });

        send_packet(packet).await?;

        if wait_for_ack {
            match select(
                async {
                    loop {
                        match subscriber.next_message().await {
                            WaitResult::Lagged(_) => {
                                // Maybe we missed the message?
                            }
                            WaitResult::Message(ControlMessage::Unsubscribed(subscribed_pid)) => {
                                if subscribed_pid == pid {
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
}

/// Initialises the MQTT stack returning a struct that can be used to send and receive messages and
/// a future that must be run in order for the stack to operate.
///
/// `broker` may be an IP address or a DNS name for the broker to connect to. The task will loop
/// forever attempting to stay connected.
///
/// `device_type` is expected to be the same for all devices of the same type while `device_id` is
/// expected to be unique. Together they are used to form device specific topics and when used
/// as part of the Home Assistant discovery data.
///
/// `last_will` allows you to publish some data to subscribers if the connection to the broker is
/// lost.
pub fn init<'t, T>(
    network: Stack<'t>,
    device_type: &'t str,
    device_id: &'t str,
    broker: &'t str,
    last_will: Option<(Topic<T>, Payload)>,
    username: Option<&'t str>,
    password: Option<&'t str>,
) -> (Mcutie<'t>, McutieTask<'t, T>)
where
    T: AsRef<str> + 't,
{
    (
        Mcutie {
            device_type,
            device_id,
        },
        McutieTask {
            network,
            device_type,
            device_id,
            broker,
            last_will,
            username,
            password,
        },
    )
}
