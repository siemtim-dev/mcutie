use core::{fmt::Display, ops::Deref};

use embassy_futures::select::{select, Either};
use embassy_sync::pubsub::WaitResult;
use embassy_time::Timer;
use heapless::{String, Vec};
use mqttrs::{Packet, QoS, Subscribe, SubscribeReturnCodes, SubscribeTopic, Unsubscribe};

use crate::{
    device_id, device_type,
    io::{assign_pid, send_packet, subscribe},
    publish::{PublishBytes, PublishDisplay, PublishJson},
    ControlMessage, Error, TopicString, CONFIRMATION_TIMEOUT,
};

/// An MQTT topic that is optionally prefixed with the device type and unique ID. This allows the
/// topic to be easily defined as a const before knowing what the device ID is.
#[derive(Clone, Copy, Eq)]
pub enum Topic<T: Deref<Target = str>> {
    /// A topic that is prefixed with the device type.
    DeviceType(T),
    /// A topic that is prefixed with the device type and unique ID.
    Device(T),
    /// Any topic.
    General(T),
}

impl<A: Deref<Target = str>, B: Deref<Target = str>> PartialEq<Topic<A>> for Topic<B> {
    fn eq(&self, other: &Topic<A>) -> bool {
        match (self, other) {
            (Topic::DeviceType(l0), Topic::DeviceType(r0)) => l0.deref() == r0.deref(),
            (Topic::Device(l0), Topic::Device(r0)) => l0.deref() == r0.deref(),
            (Topic::General(l0), Topic::General(r0)) => l0.deref() == r0.deref(),
            _ => false,
        }
    }
}

impl Topic<TopicString> {
    pub(crate) fn from_str(mut st: &str) -> Result<Self, ()> {
        let mut strip_prefix = |pr: &str| -> bool {
            if st.starts_with(pr) && &st[pr.len()..pr.len() + 1] == "/" {
                st = &st[pr.len() + 1..];
                true
            } else {
                false
            }
        };

        if strip_prefix(device_type()) {
            if strip_prefix(device_id()) {
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

impl<T: Deref<Target = str>> Topic<T> {
    pub(crate) fn to_string<const N: usize>(&self, result: &mut String<N>) -> Result<(), Error> {
        match self {
            Topic::Device(st) => {
                result
                    .push_str(device_type())
                    .map_err(|_| Error::TooLarge)?;
                result.push_str("/").map_err(|_| Error::TooLarge)?;
                result.push_str(device_id()).map_err(|_| Error::TooLarge)?;
                result.push_str("/").map_err(|_| Error::TooLarge)?;
                result.push_str(st.as_ref()).map_err(|_| Error::TooLarge)?;
            }
            Topic::DeviceType(st) => {
                result
                    .push_str(device_type())
                    .map_err(|_| Error::TooLarge)?;
                result.push_str("/").map_err(|_| Error::TooLarge)?;
                result.push_str(st.as_ref()).map_err(|_| Error::TooLarge)?;
            }
            Topic::General(st) => {
                result.push_str(st.as_ref()).map_err(|_| Error::TooLarge)?;
            }
        }

        Ok(())
    }

    /// Converts to a topic containing an [`str`]. Particularly useful for converting from an owned
    /// string for match patterns
    pub fn as_ref(&self) -> Topic<&str> {
        match self {
            Topic::DeviceType(st) => Topic::DeviceType(st.as_ref()),
            Topic::Device(st) => Topic::Device(st.as_ref()),
            Topic::General(st) => Topic::General(st.as_ref()),
        }
    }

    /// Creates a publishable message with something that can return a reference
    /// to the payload.
    pub fn with_bytes<'a, B: AsRef<[u8]>>(&'a self, data: &'a B) -> PublishBytes<'a, T, B> {
        PublishBytes {
            topic: self,
            data,
            qos: QoS::AtMostOnce,
            retain: false,
        }
    }

    /// Creates a publishable message with something that implements [`Display`].
    pub fn with_display<'a, D: Display>(&'a self, data: &'a D) -> PublishDisplay<'a, T, D> {
        PublishDisplay {
            topic: self,
            data,
            qos: QoS::AtMostOnce,
            retain: false,
        }
    }

    #[cfg(feature = "serde")]
    /// Creates a publishable message with something that can be serialized to
    /// JSON.
    pub fn with_json<'a, D: serde::Serialize>(&'a self, data: &'a D) -> PublishJson<'a, T, D> {
        PublishJson {
            topic: self,
            data,
            qos: QoS::AtMostOnce,
            retain: false,
        }
    }

    /// Subscribes to this topic. If `wait_for_ack` is true then this will wait until confirmation
    /// is received from the broker before returning.
    pub async fn subscribe(&self, wait_for_ack: bool) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let mut topic_path = TopicString::new();
        if self.to_string(&mut topic_path).is_err() {
            return Err(Error::TooLarge);
        }

        let pid = assign_pid();

        let subscribe_topic = SubscribeTopic {
            topic_path,
            qos: QoS::AtLeastOnce,
        };

        // The size of this vec must match that used by mqttrs.
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
    pub async fn unsubscribe(&self, wait_for_ack: bool) -> Result<(), Error> {
        let mut subscriber = subscribe().await;

        let mut topic_path = TopicString::new();
        if self.to_string(&mut topic_path).is_err() {
            return Err(Error::TooLarge);
        }

        let pid = assign_pid();

        // The size of this vec must match that used by mqttrs.
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
