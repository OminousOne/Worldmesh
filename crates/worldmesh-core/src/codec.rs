//! Encoding the game's data. Everything the game defines (views, inputs, commands, actions,
//! events, entity data) travels as MessagePack with field names: compact, and readable by the
//! browser client without a schema.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tracing::warn;

/// Encode a game value.
pub fn to_bytes<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    rmp_serde::to_vec_named(value)
        .map_err(|err| warn!(%err, "encoding a game value failed"))
        .ok()
}

/// Decode a game value (None, with a warning, if it doesn't match the type).
pub fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    rmp_serde::from_slice(bytes)
        .map_err(|err| warn!(%err, "decoding a game value failed"))
        .ok()
}

/// A game value from a browser (JSON) as bytes, without knowing its type.
pub fn json_to_bytes(value: &Value) -> Option<Vec<u8>> {
    to_bytes(value)
}

/// A game value as JSON for a browser, without knowing its type.
pub fn bytes_to_json(bytes: &[u8]) -> Value {
    rmp_serde::from_slice::<Value>(bytes).unwrap_or(Value::Null)
}

/// A well-mixed 64-bit hash (splitmix64): handy for deterministic per-tick randomness.
pub fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A trace id for the `n`th envelope an entity sends.
pub fn trace_id(entity: u32, n: u64) -> u64 {
    splitmix64((u64::from(entity) << 32) ^ n).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    #[serde(tag = "e", rename_all = "snake_case")]
    enum Event {
        Hit { target: u32, damage: u16 },
        Miss,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Input {
        dx: f32,
        dy: f32,
    }

    #[test]
    fn game_values_round_trip_and_read_as_json() {
        let ev = Event::Hit {
            target: 7,
            damage: 12,
        };
        let bytes = to_bytes(&ev).unwrap();
        assert_eq!(from_bytes::<Event>(&bytes), Some(ev));
        assert_eq!(
            bytes_to_json(&bytes),
            serde_json::json!({"e": "hit", "target": 7, "damage": 12})
        );
        assert_eq!(
            bytes_to_json(&to_bytes(&Event::Miss).unwrap()),
            serde_json::json!({"e": "miss"})
        );
    }

    #[test]
    fn json_from_a_browser_decodes_as_the_game_type() {
        let json = serde_json::json!({"dx": 0.5, "dy": -1});
        let bytes = json_to_bytes(&json).unwrap();
        assert_eq!(
            from_bytes::<Input>(&bytes),
            Some(Input { dx: 0.5, dy: -1.0 })
        );
        assert_eq!(
            from_bytes::<Event>(&bytes),
            None,
            "the wrong type is refused, not misread"
        );
    }
}
