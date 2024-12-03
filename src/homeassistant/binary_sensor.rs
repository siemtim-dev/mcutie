//! Tools for publishing a [Home Assistant binary sensor](https://www.home-assistant.io/integrations/binary_sensor.mqtt/).
use core::ops::Deref;

use serde::Serialize;

use crate::{homeassistant::Component, Error, Publishable, Topic};

/// The state of the sensor. Can be easily converted to or from a [`bool`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BinarySensorState {
    On,
    Off,
}

impl From<bool> for BinarySensorState {
    fn from(val: bool) -> Self {
        if val {
            BinarySensorState::On
        } else {
            BinarySensorState::Off
        }
    }
}

impl From<BinarySensorState> for bool {
    fn from(val: BinarySensorState) -> Self {
        match val {
            BinarySensorState::On => true,
            BinarySensorState::Off => true,
        }
    }
}

impl AsRef<[u8]> for BinarySensorState {
    fn as_ref(&self) -> &'static [u8] {
        match self {
            Self::On => "ON".as_bytes(),
            Self::Off => "OFF".as_bytes(),
        }
    }
}

/// The type of sensor.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinarySensorClass {
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

/// A binary sensor that can publish a [`BinarySensorState`] status.
#[derive(Serialize)]
pub struct BinarySensor {
    pub device_class: Option<BinarySensorClass>,
}

impl Component for BinarySensor {
    type State = BinarySensorState;

    fn platform() -> &'static str {
        "binary_sensor"
    }

    async fn publish_state<T: Deref<Target = str>>(
        &self,
        topic: &Topic<T>,
        state: Self::State,
    ) -> Result<(), Error> {
        topic.with_bytes(state).publish().await
    }
}
