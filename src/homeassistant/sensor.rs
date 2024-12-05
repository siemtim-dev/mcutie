//! Tools for publishing a [Home Assistant sensor](https://www.home-assistant.io/integrations/sensor.mqtt/).
use core::ops::Deref;

use serde::Serialize;

use crate::{homeassistant::Component, Error, Publishable, Topic};

/// The type of sensor.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorClass {
    ApparentPower,
    Aqi,
    AtmosphericPressure,
    Battery,
    CarbonDioxide,
    CarbonMonoxide,
    Current,
    DataRate,
    DataSize,
    Date,
    Distance,
    Duration,
    Energy,
    EnergyStorage,
    Enum,
    Frequency,
    Gas,
    Humidity,
    Illuminance,
    Irradiance,
    Moisture,
    Monetary,
    NitrogenDioxide,
    NitrogenMonoxide,
    NitrousOxide,
    Ozone,
    Ph,
    Pm1,
    Pm25,
    Pm10,
    PowerFactor,
    Power,
    Precipitation,
    PrecipitationIntensity,
    Pressure,
    ReactivePower,
    SignalStrength,
    SoundPressure,
    Speed,
    SulphurDioxide,
    Temperature,
    Timestamp,
    VolatileOrganicCompounds,
    VolatileOrganicCompoundsParts,
    Voltage,
    Volume,
    VolumeFlowRate,
    VolumeStorage,
    Water,
    Weight,
    WindSpeed,
}

/// The type of measurement that this entity publishes.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorStateClass {
    Measurement,
    Total,
    TotalIncreasing,
}

/// A binary sensor that can publish a [`f32`] value.
#[derive(Serialize)]
pub struct Sensor<'u> {
    pub device_class: Option<SensorClass>,
    pub state_class: Option<SensorStateClass>,
    pub unit_of_measurement: Option<&'u str>,
}

impl Component for Sensor<'_> {
    type State = f32;

    fn platform() -> &'static str {
        "sensor"
    }

    async fn publish_state<T: Deref<Target = str>>(
        &self,
        topic: &Topic<T>,
        state: Self::State,
    ) -> Result<(), Error> {
        topic.with_display(state).publish().await
    }
}
