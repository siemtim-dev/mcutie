//! Tools for publishing a [Home Assistant light](https://www.home-assistant.io/integrations/light.mqtt/).
use serde::{ser::SerializeStruct, Serialize, Serializer};

use crate::{homeassistant::ser::List, Topic};

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum ColorMode {
    Rgb,
}

struct Light<'a, const C: usize, const E: usize> {
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
