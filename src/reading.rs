//! One device's state as a provider saw it at one moment. Serialized exactly as the C#
//! app writes it to state.json: PascalCase names, the kind as a number, the time as ISO 8601 with its offset.

use chrono::{DateTime, Local};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Headset = 0,
    Mouse = 1,
    Keyboard = 2,
    Gamepad = 3,
    #[default]
    Other = 4,
}

impl DeviceKind {
    fn from_number(n: u64) -> Self {
        match n {
            0 => Self::Headset,
            1 => Self::Mouse,
            2 => Self::Keyboard,
            3 => Self::Gamepad,
            _ => Self::Other,
        }
    }
}

impl Serialize for DeviceKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for DeviceKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // A number, as the C# app writes it; a name is taken too
        Ok(match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => Self::from_number(n.as_u64().unwrap_or(4)),
            serde_json::Value::String(s) => match s.as_str() {
                "Headset" => Self::Headset,
                "Mouse" => Self::Mouse,
                "Keyboard" => Self::Keyboard,
                "Gamepad" => Self::Gamepad,
                _ => Self::Other,
            },
            _ => Self::Other,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DeviceReading {
    /// Stable key for settings and history, e.g. "razer:barracuda-x" or "bt:A1B2C3D4E5F6".
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// How the device is reached, as shown: "2.4 GHz", "Bluetooth", "cable"...
    pub via: String,
    pub online: bool,
    /// 0-100, or None when the device doesn't report a level right now. Never a stale level passed off as
    /// current: a device that can't be read while charging reports None and says why in `detail`.
    pub level: Option<i32>,
    /// `level` is our estimate (e.g. from the voltage), not the device's own number.
    pub estimate: bool,
    pub charging: bool,
    /// The charger is still connected and the device itself reports the battery full.
    pub full_on_charger: bool,
    /// Extra text for the UI: the voltage, or why there is no reading.
    pub detail: Option<String>,
    #[serde(serialize_with = "write_time", deserialize_with = "read_time")]
    pub time: DateTime<Local>,
}

impl Default for DeviceReading {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: DeviceKind::Other,
            via: String::new(),
            online: false,
            level: None,
            estimate: false,
            charging: false,
            full_on_charger: false,
            detail: None,
            time: Local::now(),
        }
    }
}

impl DeviceReading {
    /// An offline reading with no level, stamped now: the starting point every provider fills in.
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: DeviceKind, via: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), kind, via: via.into(), ..Default::default() }
    }
}

fn write_time<S: Serializer>(t: &DateTime<Local>, s: S) -> Result<S::Ok, S::Error> {
    // Microseconds: .NET reads up to 7 fraction digits
    s.serialize_str(&t.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string())
}

fn read_time<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Local>, D::Error> {
    let text = String::deserialize(d)?;
    if let Ok(t) = DateTime::parse_from_rfc3339(&text) {
        return Ok(t.with_timezone(&Local));
    }
    // .NET writes a DateTime without an offset when its kind is unspecified: local time
    chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .and_then(|n| n.and_local_timezone(Local).earliest())
        .ok_or_else(|| serde::de::Error::custom(format!("not a time: {text}")))
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

/// The C# record's ToString, so a dump of both apps can be compared line by line.
impl fmt::Display for DeviceReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DeviceReading {{ Id = {}, Name = {}, Kind = {:?}, Via = {}, Online = {}, Level = {}, Estimate = {}, \
                   Charging = {}, FullOnCharger = {}, Detail = {}, Time = {} }}",
            self.id,
            self.name,
            self.kind,
            self.via,
            yes_no(self.online),
            self.level.map(|l| l.to_string()).unwrap_or_default(),
            yes_no(self.estimate),
            yes_no(self.charging),
            yes_no(self.full_on_charger),
            self.detail.as_deref().unwrap_or(""),
            self.time.format("%d.%m.%Y %H:%M:%S")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_csharp_app_wrote() {
        let json = r#"{"Id":"razer:barracuda-x","Name":"Razer Barracuda X","Kind":0,"Via":"2.4 ГГц","Online":true,
            "Level":85,"Estimate":false,"Charging":false,"FullOnCharger":false,"Detail":null,
            "Time":"2026-11-18T13:08:10.0661234+03:00"}"#;
        let r: DeviceReading = serde_json::from_str(json).unwrap();
        assert_eq!(r.kind, DeviceKind::Headset);
        assert_eq!(r.level, Some(85));
        assert_eq!(r.via, "2.4 ГГц");
        let back = serde_json::to_string(&r).unwrap();
        assert!(back.contains(r#""Kind":0"#) && back.contains(r#""FullOnCharger":false"#), "{back}");
    }
}
