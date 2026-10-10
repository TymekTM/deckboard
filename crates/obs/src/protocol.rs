//! obs-websocket v5 protocol framing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RPC_VERSION: u32 = 1;
pub const ALL_SUBSCRIPTIONS: u64 = 2047;

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Hello(Hello),
    Identify(Identify),
    Identified(Identified),
    Event(Event),
    Request(Request),
    RequestResponse(RequestResponse),
}

#[derive(Serialize, Deserialize)]
struct RawEnvelope {
    op: u32,
    d: Value,
}

impl Serialize for Message {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (op, d) = match self {
            Message::Hello(h) => (
                0,
                serde_json::to_value(h).map_err(serde::ser::Error::custom)?,
            ),
            Message::Identify(i) => (
                1,
                serde_json::to_value(i).map_err(serde::ser::Error::custom)?,
            ),
            Message::Identified(i) => (
                2,
                serde_json::to_value(i).map_err(serde::ser::Error::custom)?,
            ),
            Message::Event(e) => (
                5,
                serde_json::to_value(e).map_err(serde::ser::Error::custom)?,
            ),
            Message::Request(r) => (
                6,
                serde_json::to_value(r).map_err(serde::ser::Error::custom)?,
            ),
            Message::RequestResponse(r) => (
                7,
                serde_json::to_value(r).map_err(serde::ser::Error::custom)?,
            ),
        };
        RawEnvelope { op, d }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawEnvelope::deserialize(deserializer)?;
        match raw.op {
            0 => serde_json::from_value(raw.d)
                .map(Message::Hello)
                .map_err(serde::de::Error::custom),
            1 => serde_json::from_value(raw.d)
                .map(Message::Identify)
                .map_err(serde::de::Error::custom),
            2 => serde_json::from_value(raw.d)
                .map(Message::Identified)
                .map_err(serde::de::Error::custom),
            5 => serde_json::from_value(raw.d)
                .map(Message::Event)
                .map_err(serde::de::Error::custom),
            6 => serde_json::from_value(raw.d)
                .map(Message::Request)
                .map_err(serde::de::Error::custom),
            7 => serde_json::from_value(raw.d)
                .map(Message::RequestResponse)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "unsupported op code: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub obs_web_socket_version: String,
    pub rpc_version: u32,
    pub authentication: Option<HelloAuth>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelloAuth {
    pub challenge: String,
    pub salt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identify {
    pub rpc_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_subscriptions: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identified {
    pub negotiated_rpc_version: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub event_type: String,
    pub event_intent: Option<u64>,
    #[serde(default)]
    pub event_data: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub request_type: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResponse {
    pub request_type: String,
    pub request_id: String,
    pub request_status: RequestStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestStatus {
    pub result: bool,
    pub code: u32,
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framing_serde() {
        let hello_json = r#"{"op":0,"d":{"obsWebSocketVersion":"5.5.0","rpcVersion":1,"authentication":{"challenge":"ch1","salt":"s1"}}}"#;
        let msg: Message = serde_json::from_str(hello_json).unwrap();
        match msg {
            Message::Hello(h) => {
                assert_eq!(h.obs_web_socket_version, "5.5.0");
                assert_eq!(h.rpc_version, 1);
                let auth = h.authentication.unwrap();
                assert_eq!(auth.challenge, "ch1");
                assert_eq!(auth.salt, "s1");
            }
            _ => panic!("wrong message variant"),
        }

        let identify = Message::Identify(Identify {
            rpc_version: 1,
            authentication: Some("auth123".into()),
            event_subscriptions: Some(ALL_SUBSCRIPTIONS),
        });
        let s = serde_json::to_string(&identify).unwrap();
        assert!(s.contains(r#""op":1"#));
        assert!(s.contains(r#""authentication":"auth123""#));

        let event_json = r#"{"op":5,"d":{"eventType":"CurrentProgramSceneChanged","eventIntent":4,"eventData":{"sceneName":"Gameplay"}}}"#;
        let msg: Message = serde_json::from_str(event_json).unwrap();
        match msg {
            Message::Event(e) => {
                assert_eq!(e.event_type, "CurrentProgramSceneChanged");
                assert_eq!(e.event_data["sceneName"], "Gameplay");
            }
            _ => panic!("wrong message variant"),
        }
    }
}
