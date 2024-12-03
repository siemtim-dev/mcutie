//! Home Assistant auto-discovery and related messages.
//!
//! Normally you would declare your entities statically in your binary. It is
//! then trivial to send out discovery messages or state changes.
//!
//! ```
//! # use mcutie::{Publishable, Topic};
//! # use mcutie::homeassistant::{Entity, Device, Origin, AvailabilityState, AvailabilityTopics};
//! # use mcutie::homeassistant::binary_sensor::{BinarySensor, BinarySensorClass, BinarySensorState};
//! const DEVICE_AVAILABILITY_TOPIC: Topic<&'static str> = Topic::Device("status");
//! const MOTION_STATE_TOPIC: Topic<&'static str> = Topic::Device("motion/status");
//!
//! const DEVICE: Device<'static> = Device::new();
//! const ORIGIN: Origin<'static> = Origin::new();
//!
//! const MOTION_SENSOR: Entity<'static, 1, BinarySensor> = Entity {
//!     device: DEVICE,
//!     origin: ORIGIN,
//!     object_id: "motion",
//!     unique_id: Some("motion"),
//!     name: "Motion",
//!     availability: AvailabilityTopics::All([DEVICE_AVAILABILITY_TOPIC]),
//!     state_topic: MOTION_STATE_TOPIC,
//!     component: BinarySensor {
//!         device_class: Some(BinarySensorClass::Motion),
//!     },
//! };
//!
//! async fn send_discovery_messages() {
//!     MOTION_SENSOR.publish_discovery().await.unwrap();
//!     DEVICE_AVAILABILITY_TOPIC.with_bytes(AvailabilityState::Online).publish().await.unwrap();
//! }
//!
//! async fn send_state(state: BinarySensorState) {
//!     MOTION_SENSOR.publish_state(state).await.unwrap();
//! }
//! ```
use core::{future::Future, ops::Deref};

use mqttrs::QoS;
use serde::{
    ser::{Error as _, SerializeStruct},
    Serialize, Serializer,
};

use crate::{
    device_id, device_type, homeassistant::ser::DiscoverySerializer, io::publish, Error,
    McutieTask, MqttMessage, Payload, Publishable, Topic, TopicString, DATA_CHANNEL,
};

pub mod binary_sensor;
// pub mod light;
pub mod sensor;
mod ser;

const HA_STATUS_TOPIC: Topic<&'static str> = Topic::General("homeassistant/status");
const STATE_ONLINE: &str = "online";
const STATE_OFFLINE: &str = "offline";

/// A trait representing a specific type of entity in Home Assistant
pub trait Component: Serialize {
    /// The state to publish.
    type State;

    fn platform() -> &'static str;

    fn publish_state<T: Deref<Target = str>>(
        &self,
        topic: &Topic<T>,
        state: Self::State,
    ) -> impl Future<Output = Result<(), Error>>;
}

impl<'t, T, L, const S: usize> McutieTask<'t, T, L, S>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    pub(super) async fn ha_after_connected(&self) {
        let _ = HA_STATUS_TOPIC.subscribe(false).await;
    }

    pub(super) async fn ha_handle_update(
        &self,
        topic: &Topic<TopicString>,
        payload: &Payload,
    ) -> bool {
        if topic == &HA_STATUS_TOPIC {
            if payload.as_ref() == STATE_ONLINE.as_bytes() {
                DATA_CHANNEL.send(MqttMessage::HomeAssistantOnline).await;
            }

            true
        } else {
            false
        }
    }
}

impl<T: Deref<Target = str>> Serialize for Topic<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut topic = TopicString::new();
        self.to_string(&mut topic)
            .map_err(|_| S::Error::custom("topic was too large to serialize"))?;
        serializer.serialize_str(&topic)
    }
}

fn name_or_device<S>(name: &Option<&str>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(name.unwrap_or_else(|| device_type()))
}

/// Represents the device in Home Assistant.
///
/// Can just be the default in which case useful properties such as the ID are
/// automatically included.
#[derive(Clone, Copy, Default)]
pub struct Device<'a> {
    pub name: Option<&'a str>,
    pub configuration_url: Option<&'a str>,
}

impl<'a> Device<'a> {
    pub const fn new() -> Self {
        Self {
            name: None,
            configuration_url: None,
        }
    }
}

impl<'a> Serialize for Device<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut len = 2;
        if self.configuration_url.is_some() {
            len += 1;
        }

        let mut serializer = serializer.serialize_struct("Device", len)?;

        serializer.serialize_field("name", self.name.unwrap_or_else(|| device_type()))?;
        serializer.serialize_field("ids", device_id())?;

        if let Some(cu) = self.configuration_url {
            serializer.serialize_field("cu", cu)?;
        } else {
            serializer.skip_field("cu")?;
        }

        serializer.end()
    }
}

/// Represents the device's origin in Home Assistant.
///
/// Can just be the default in which case useful properties are automatically
/// included.
#[derive(Clone, Copy, Default, Serialize)]
pub struct Origin<'a> {
    #[serde(serialize_with = "name_or_device")]
    pub name: Option<&'a str>,
}

impl<'a> Origin<'a> {
    pub const fn new() -> Self {
        Self { name: None }
    }
}

/// A single entity for Home Assistant.
///
/// Calling [`Entity::publish_discovery`] will publish the discovery message to
/// allow Home Assistant to detect this entity. Read the
/// [Home Assistant MQTT docs](https://www.home-assistant.io/integrations/mqtt/)
/// for information on what some of these properties mean.
pub struct Entity<'a, const A: usize, C: Component> {
    pub device: Device<'a>,
    pub origin: Origin<'a>,
    pub object_id: &'a str,
    pub unique_id: Option<&'a str>,
    pub name: &'a str,
    /// Specifies the availability topics that Home Assistant will listen to to
    /// determine this entity's availability.
    pub availability: AvailabilityTopics<'a, A>,
    /// The state topic that this entity's state is published to.
    pub state_topic: Topic<&'a str>,
    /// The specific entity.
    pub component: C,
}

impl<'a, const A: usize, C: Component> Entity<'a, A, C> {
    /// Publishes the discovery message for this entity to the broker.
    pub async fn publish_discovery(&self) -> Result<(), Error> {
        let mut topic = TopicString::new();
        topic
            .push_str(option_env!("HA_DISCOVERY_PREFIX").unwrap_or("homeassistant"))
            .map_err(|_| Error::TooLarge)?;
        topic.push('/').map_err(|_| Error::TooLarge)?;
        topic.push_str(C::platform()).map_err(|_| Error::TooLarge)?;
        topic.push('/').map_err(|_| Error::TooLarge)?;
        topic
            .push_str(self.object_id)
            .map_err(|_| Error::TooLarge)?;
        topic.push_str("/config").map_err(|_| Error::TooLarge)?;

        let mut payload = Payload::new();
        payload.serialize_json(self).map_err(|_| Error::TooLarge)?;

        publish(&topic, &payload, QoS::AtMostOnce, false).await
    }

    /// Publishes this entity's state to the broker.
    pub async fn publish_state(&self, state: C::State) -> Result<(), Error> {
        self.component.publish_state(&self.state_topic, state).await
    }
}

/// A payload representing a device or entity's availability.
pub enum AvailabilityState {
    Online,
    Offline,
}

impl AsRef<[u8]> for AvailabilityState {
    fn as_ref(&self) -> &'static [u8] {
        match self {
            Self::Online => STATE_ONLINE.as_bytes(),
            Self::Offline => STATE_OFFLINE.as_bytes(),
        }
    }
}

/// The availiabity topics that home assistant will use to determine an entity's
/// availability.
pub enum AvailabilityTopics<'a, const A: usize> {
    /// The entity is always available.
    None,
    /// The entity is available if all of the topics are publishes as online.
    All([Topic<&'a str>; A]),
    /// The entity is available if any of the topics are publishes as online.
    Any([Topic<&'a str>; A]),
    /// The entity is available based on the most recent of the topics to
    /// publish state.
    Latest([Topic<&'a str>; A]),
}

impl<'a, const A: usize, C: Component> Serialize for Entity<'a, A, C> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let outer = DiscoverySerializer {
            discovery: self,
            inner: serializer,
        };

        self.component.serialize(outer)
    }
}
