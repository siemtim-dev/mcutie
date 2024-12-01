use core::{fmt::Display, ops::Deref};

use serde::{
    ser::{SerializeSeq, SerializeStruct},
    Serialize, Serializer,
};

use crate::{
    device_id, device_type, Mcutie, McutieTask, MqttMessage, Payload, Publishable, Topic,
    TopicString, DATA_CHANNEL,
};

const HA_STATUS_TOPIC: Topic<&'static str> = Topic::General("homeassistant/status");
const STATE_ONLINE: &str = "online";

pub trait Component: Serialize {
    fn platform() -> &'static str;
}

impl<'t, T, L, const S: usize> McutieTask<'t, T, L, S>
where
    T: Deref<Target = str> + 't,
    L: Publishable + 't,
{
    pub(super) async fn ha_after_connected(&self) {
        let mcutie = Mcutie {};
        let _ = mcutie.subscribe(&HA_STATUS_TOPIC, false).await;
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
        self.to_string(&mut topic).unwrap();
        serializer.serialize_str(&topic)
    }
}

fn name_or_device<S>(name: &Option<&str>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(name.unwrap_or_else(|| device_type()))
}

#[derive(Default)]
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

#[derive(Default, Serialize)]
pub struct Origin<'a> {
    #[serde(serialize_with = "name_or_device")]
    pub name: Option<&'a str>,
}

impl<'a> Origin<'a> {
    pub const fn new() -> Self {
        Self { name: None }
    }
}

pub struct Discovery<'a, const A: usize, C: Component> {
    pub device: &'a Device<'a>,
    pub origin: &'a Origin<'a>,
    pub object_id: &'a str,
    pub unique_id: Option<&'a str>,
    pub name: &'a str,
    pub availability: Option<AvailabilityTopics<'a, A>>,
    pub state_topic: Topic<&'a str>,
    pub component: C,
}

impl<'a, const A: usize, C: Component> Publishable for Discovery<'a, A, C> {
    type TopicError = ();
    type PayloadError = serde_json_core::ser::Error;

    fn write_topic(&self, topic: &mut TopicString) -> Result<(), Self::TopicError> {
        topic.push_str(option_env!("HA_DISCOVERY_PREFIX").unwrap_or("homeassistant"))?;
        topic.push('/')?;
        topic.push_str(C::platform())?;
        topic.push('/')?;
        topic.push_str(self.object_id)?;
        topic.push_str("/config")
    }

    fn write_payload(&self, payload: &mut crate::Payload) -> Result<(), Self::PayloadError> {
        payload.serialize_json(self)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    Rgb,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityState {
    Online,
    Offline,
}

impl Display for AvailabilityState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AvailabilityMode {
    All,
    Any,
    Latest,
}

pub struct AvailabilityTopics<'a, const A: usize> {
    pub mode: AvailabilityMode,
    pub topics: [Topic<&'a str>; A],
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Battery,
    BatteryCharging,
    CarbonMonoxide,
    Cold,
    Connectivity,
    Door,
    GarageDoor,
    Gas,
    Heat,
    Light,
    Lock,
    Moisture,
    Motion,
    Moving,
    Occupancy,
    Opening,
    Plug,
    Power,
    Presence,
    Problem,
    Running,
    Safety,
    Smoke,
    Sound,
    Tamper,
    Update,
    Vibration,
    Window,
}

#[derive(Serialize)]
pub struct BinarySensor {
    pub device_class: Option<DeviceClass>,
}

impl Component for BinarySensor {
    fn platform() -> &'static str {
        "binary_sensor"
    }
}

pub struct Light<'a, const C: usize, const E: usize> {
    pub command_topic: Topic<&'a str>,
    pub supported_color_modes: [ColorMode; C],
    pub effects: [&'a str; E],
}

impl<'a, const C: usize, const E: usize> Serialize for Light<'a, C, E> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut len = 3;

        if C > 0 {
            len += 1;
        }

        if E > 0 {
            len += 2;
        }

        let mut serializer = serializer.serialize_struct("Light", len)?;

        serializer.serialize_field("cmd_t", &self.command_topic)?;
        serializer.serialize_field("schema", "json")?;

        if C > 0 {
            serializer.serialize_field("sup_clrm", &List::new(&self.supported_color_modes))?;
        }

        if E > 0 {
            serializer.serialize_field("effect", &true)?;
            serializer.serialize_field("fx_list", &List::new(&self.effects))?;
        }

        serializer.end()
    }
}

impl<'a, const A: usize, C: Component> Serialize for Discovery<'a, A, C> {
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

struct List<'a, T: Serialize, const N: usize> {
    list: &'a [T; N],
}

impl<'a, T: Serialize, const N: usize> List<'a, T, N> {
    fn new(list: &'a [T; N]) -> Self {
        Self { list }
    }
}

impl<'a, T: Serialize, const N: usize> Serialize for List<'a, T, N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut serializer = serializer.serialize_seq(Some(N))?;

        for item in self.list {
            serializer.serialize_element(item)?;
        }

        serializer.end()
    }
}

struct DiscoverySerializer<'a, const A: usize, C: Component, S: Serializer> {
    discovery: &'a Discovery<'a, A, C>,
    inner: S,
}

impl<'a, const A: usize, C: Component, S: Serializer> Serializer
    for DiscoverySerializer<'a, A, C, S>
{
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = S::SerializeStruct;
    type SerializeStructVariant = S::SerializeStructVariant;

    fn serialize_struct(
        self,
        name: &'static str,
        mut len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        len += 6;
        if self.discovery.unique_id.is_some() {
            len += 1;
        }
        if self.discovery.availability.is_some() {
            len += 2;
        }

        let mut serializer = self.inner.serialize_struct(name, len)?;

        serializer.serialize_field("dev", &self.discovery.device)?;
        serializer.serialize_field("o", &self.discovery.origin)?;
        serializer.serialize_field("p", C::platform())?;
        serializer.serialize_field("obj_id", self.discovery.object_id)?;

        serializer.serialize_field("name", self.discovery.name)?;
        serializer.serialize_field("stat_t", &self.discovery.state_topic)?;

        if let Some(availability) = &self.discovery.availability {
            serializer.serialize_field("avty", &List::new(&availability.topics))?;
            serializer.serialize_field("avty_mode", &availability.mode)?;
        }

        if let Some(v) = self.discovery.unique_id {
            serializer.serialize_field("uniq_id", v)?;
        } else {
            serializer.skip_field("uniq_id")?;
        }

        Ok(serializer)
    }

    fn serialize_bool(self, _: bool) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_i8(self, _: i8) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_i16(self, _: i16) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_i32(self, _: i32) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_i64(self, _: i64) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_u8(self, _: u8) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_u16(self, _: u16) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_u32(self, _: u32) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_u64(self, _: u64) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_f32(self, _: f32) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_f64(self, _: f64) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_char(self, _: char) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_str(self, _: &str) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_some<T>(self, _: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        unimplemented!()
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_newtype_struct<T>(self, _: &'static str, _: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        unimplemented!()
    }

    fn serialize_newtype_variant<T>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        unimplemented!()
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        unimplemented!()
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
        unimplemented!()
    }

    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        unimplemented!()
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        unimplemented!()
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        unimplemented!()
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        unimplemented!()
    }

    fn serialize_i128(self, _: i128) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn serialize_u128(self, _: u128) -> Result<Self::Ok, Self::Error> {
        unimplemented!()
    }

    fn collect_seq<I>(self, _: I) -> Result<Self::Ok, Self::Error>
    where
        I: IntoIterator,
        <I as IntoIterator>::Item: Serialize,
    {
        unimplemented!()
    }

    fn collect_map<K, V, I>(self, _: I) -> Result<Self::Ok, Self::Error>
    where
        K: Serialize,
        V: Serialize,
        I: IntoIterator<Item = (K, V)>,
    {
        unimplemented!()
    }

    fn collect_str<T>(self, _: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + core::fmt::Display,
    {
        unimplemented!()
    }

    fn is_human_readable(&self) -> bool {
        unimplemented!()
    }
}
