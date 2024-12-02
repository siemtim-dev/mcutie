use core::{fmt::Display, future::Future, ops::Deref};

use embedded_io::Write;
use mqttrs::QoS;
use serde::Serialize;

use crate::{io::publish, Error, Payload, Topic, TopicString};

/// A message that can be published to an MQTT broker.
pub trait Publishable {
    fn write_topic(&self, buffer: &mut TopicString) -> Result<(), Error>;
    fn write_payload(&self, buffer: &mut Payload) -> Result<(), Error>;

    fn qos(&self) -> QoS {
        QoS::AtMostOnce
    }

    fn retain(&self) -> bool {
        false
    }

    fn publish(&self) -> impl Future<Output = Result<(), Error>> {
        async {
            let mut topic = TopicString::new();
            self.write_topic(&mut topic)?;

            let mut payload = Payload::new();
            self.write_payload(&mut payload)?;

            publish(&topic, &payload, self.qos(), self.retain()).await
        }
    }
}

/// A [`Publishable`] with a raw byte payload.
pub struct PublishBytes<'a, T: Deref<Target = str> + 'a, B: AsRef<[u8]>> {
    pub(crate) topic: &'a Topic<T>,
    pub(crate) data: &'a B,
    pub(crate) qos: QoS,
    pub(crate) retain: bool,
}

impl<'a, T: Deref<Target = str> + 'a, B: AsRef<[u8]>> PublishBytes<'a, T, B> {
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    pub fn retain(mut self, retain: bool) -> Self {
        self.retain = retain;
        self
    }
}

impl<'a, T: Deref<Target = str> + 'a, B: AsRef<[u8]>> Publishable for PublishBytes<'a, T, B> {
    fn write_topic(&self, buffer: &mut TopicString) -> Result<(), Error> {
        self.topic.to_string(buffer)
    }

    fn write_payload(&self, buffer: &mut Payload) -> Result<(), Error> {
        buffer
            .write_all(self.data.as_ref())
            .map_err(|_| Error::TooLarge)
    }

    fn qos(&self) -> QoS {
        self.qos
    }

    fn retain(&self) -> bool {
        self.retain
    }

    async fn publish(&self) -> Result<(), Error> {
        let mut topic = TopicString::new();
        self.write_topic(&mut topic)?;

        publish(&topic, self.data.as_ref(), self.qos(), self.retain()).await
    }
}

/// A [`Publishable`] with a payload that implements [`Display`].
pub struct PublishDisplay<'a, T: Deref<Target = str> + 'a, D: Display> {
    pub(crate) topic: &'a Topic<T>,
    pub(crate) data: &'a D,
    pub(crate) qos: QoS,
    pub(crate) retain: bool,
}

impl<'a, T: Deref<Target = str> + 'a, D: Display> PublishDisplay<'a, T, D> {
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    pub fn retain(mut self, retain: bool) -> Self {
        self.retain = retain;
        self
    }
}

impl<'a, T: Deref<Target = str> + 'a, D: Display> Publishable for PublishDisplay<'a, T, D> {
    fn write_topic(&self, buffer: &mut TopicString) -> Result<(), Error> {
        self.topic.to_string(buffer)
    }

    fn write_payload(&self, buffer: &mut Payload) -> Result<(), Error> {
        write!(buffer, "{}", self.data).map_err(|_| Error::TooLarge)
    }

    fn qos(&self) -> QoS {
        self.qos
    }

    fn retain(&self) -> bool {
        self.retain
    }
}

#[cfg(feature = "serde")]
/// A [`Publishable`] with that serializes a JSON payload.
pub struct PublishJson<'a, T: Deref<Target = str> + 'a, D: Serialize> {
    pub(crate) topic: &'a Topic<T>,
    pub(crate) data: &'a D,
    pub(crate) qos: QoS,
    pub(crate) retain: bool,
}

#[cfg(feature = "serde")]
impl<'a, T: Deref<Target = str> + 'a, D: Serialize> PublishJson<'a, T, D> {
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    pub fn retain(mut self, retain: bool) -> Self {
        self.retain = retain;
        self
    }
}

#[cfg(feature = "serde")]
impl<'a, T: Deref<Target = str> + 'a, D: Serialize> Publishable for PublishJson<'a, T, D> {
    fn write_topic(&self, buffer: &mut TopicString) -> Result<(), Error> {
        self.topic.to_string(buffer)
    }

    fn write_payload(&self, buffer: &mut Payload) -> Result<(), Error> {
        buffer
            .serialize_json(self.data)
            .map_err(|_| Error::TooLarge)
    }

    fn qos(&self) -> QoS {
        self.qos
    }

    fn retain(&self) -> bool {
        self.retain
    }
}
