//! State engine (docs/protocol-v2.md §5): one authoritative map of live
//! channels. Producers push; clients get `state.sync` once and
//! `state.patch` batches (latest wins per channel) from the flusher task.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

use deckboard_proto::{ChannelInfo, ChannelValue, StateShape, StateSync};

/// The one place that knows how pushed keys map to channels: everything
/// extension/native producers push lands under `ext.<key>` (the key being
/// the legacy watch key).
pub fn ext_channel(key: &str) -> String {
    format!("ext.{key}")
}

/// The numeric point a series push carries: a bare number, or the original
/// app's custom-value object (`{"value": "12.3", "suffix": "%", ..}`) with
/// the number extracted. `None` when the push carries no usable number.
fn series_point(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value.get("value").and_then(|inner| {
            inner
                .as_f64()
                .or_else(|| inner.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        })
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ChannelMeta {
    pub shape: StateShape,
    pub cap: u32,
}

#[derive(Default)]
struct Inner {
    channels: BTreeMap<String, ChannelMeta>,
    /// Current value for scalar/toggle/list channels.
    values: BTreeMap<String, serde_json::Value>,
    /// Ring buffers for series channels, oldest -> newest.
    series: BTreeMap<String, VecDeque<f64>>,
    /// Channels changed since the last flush.
    dirty: BTreeSet<String>,
}

pub struct StateEngine {
    inner: Mutex<Inner>,
    default_cap: u32,
}

impl StateEngine {
    pub fn new(default_cap: u32) -> StateEngine {
        StateEngine {
            inner: Mutex::new(Inner::default()),
            default_cap,
        }
    }

    /// Tiles declare their channels while boards are built; the last
    /// declared shape wins. If the shape changed since earlier pushes
    /// (producers may start before any tile references the channel), the
    /// stored value migrates: numeric scalars seed the series buffer, a
    /// series' newest point becomes the scalar value.
    pub fn register(&self, channel: &str, shape: StateShape, cap: Option<u32>) {
        let mut inner = self.inner.lock().expect("state engine poisoned");
        let meta = ChannelMeta {
            shape,
            cap: cap.unwrap_or(self.default_cap),
        };
        match inner.channels.get(channel).copied() {
            Some(old) if old.shape == shape => {
                inner.channels.insert(channel.to_string(), meta);
            }
            Some(old) => {
                inner.channels.insert(channel.to_string(), meta);
                tracing::debug!(channel, from = ?old.shape, to = ?shape, "channel shape changed, migrating value");
                if shape == StateShape::Series {
                    if let Some(point) = inner.values.remove(channel).and_then(|v| v.as_f64()) {
                        inner
                            .series
                            .insert(channel.to_string(), VecDeque::from(vec![point]));
                    }
                } else if let Some(buffer) = inner.series.remove(channel) {
                    if let Some(newest) = buffer.back().copied() {
                        inner
                            .values
                            .insert(channel.to_string(), serde_json::Value::from(newest));
                    }
                }
            }
            None => {
                inner.channels.insert(channel.to_string(), meta);
            }
        }
    }

    /// Producer push. Unregistered channels auto-register as scalar, so
    /// pushes are visible before any tile references them. Series channels
    /// accumulate numeric points in their ring buffer; producers may push a
    /// bare number or the original app's custom-value object (`{"value":
    /// "12.3", "suffix": "%", ..}`) - the number is extracted either way,
    /// matching the client's own `numericValue()` tolerance. Anything else
    /// non-numeric is dropped.
    pub fn set(&self, channel: &str, value: serde_json::Value) {
        let mut inner = self.inner.lock().expect("state engine poisoned");
        let meta = *inner
            .channels
            .entry(channel.to_string())
            .or_insert(ChannelMeta {
                shape: StateShape::Scalar,
                cap: self.default_cap,
            });
        match meta.shape {
            StateShape::Series => {
                let Some(point) = series_point(&value) else {
                    tracing::warn!(channel, "non-numeric value for series channel dropped");
                    return;
                };
                let buffer = inner.series.entry(channel.to_string()).or_default();
                buffer.push_back(point);
                while buffer.len() > meta.cap as usize {
                    buffer.pop_front();
                }
            }
            _ => {
                // Producers push only on change; identical values are
                // dropped here so the flusher stays silent too.
                if inner.values.get(channel) == Some(&value) {
                    return;
                }
                inner.values.insert(channel.to_string(), value);
            }
        }
        inner.dirty.insert(channel.to_string());
    }

    /// The `welcome` channel catalog: every known channel with shape and
    /// (for series) ring-buffer size.
    pub fn catalog(&self) -> BTreeMap<String, ChannelInfo> {
        self.inner
            .lock()
            .expect("state engine poisoned")
            .channels
            .iter()
            .map(|(name, meta)| {
                (
                    name.clone(),
                    ChannelInfo {
                        shape: meta.shape,
                        cap: (meta.shape == StateShape::Series).then_some(meta.cap),
                    },
                )
            })
            .collect()
    }

    /// Full current state for the one-shot `state.sync` after `welcome`.
    pub fn snapshot(&self) -> StateSync {
        let inner = self.inner.lock().expect("state engine poisoned");
        StateSync {
            values: inner.values.clone(),
            series: inner
                .series
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().copied().collect()))
                .collect(),
        }
    }

    /// Latest value per changed channel; called by the flusher task.
    pub fn drain_dirty(&self) -> Vec<ChannelValue> {
        let mut inner = self.inner.lock().expect("state engine poisoned");
        let dirty = std::mem::take(&mut inner.dirty);
        dirty
            .into_iter()
            .filter_map(|channel| {
                let meta = *inner.channels.get(&channel)?;
                if meta.shape == StateShape::Series {
                    let newest = inner.series.get(&channel)?.back().copied()?;
                    Some(ChannelValue {
                        channel,
                        value: serde_json::Value::from(newest),
                    })
                } else {
                    inner
                        .values
                        .get(&channel)
                        .cloned()
                        .map(|value| ChannelValue { channel, value })
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_channels_keep_latest_value() {
        let engine = StateEngine::new(120);
        engine.set("ext.speaker-muted", serde_json::json!("OFF"));
        engine.set("ext.speaker-muted", serde_json::json!("ON"));
        engine.set("ext.count", serde_json::json!(3));
        let sync = engine.snapshot();
        assert_eq!(sync.values["ext.speaker-muted"], "ON");
        assert_eq!(sync.values["ext.count"], 3);
        let changes = engine.drain_dirty();
        assert_eq!(changes.len(), 2); // latest wins per channel
    }

    #[test]
    fn scalar_pushes_only_on_change() {
        let engine = StateEngine::new(120);
        engine.set("ext.x", serde_json::json!("a"));
        assert_eq!(engine.drain_dirty().len(), 1);
        // Identical value: dropped before it can dirty the channel.
        engine.set("ext.x", serde_json::json!("a"));
        assert!(engine.drain_dirty().is_empty());
        engine.set("ext.x", serde_json::json!("b"));
        assert_eq!(engine.drain_dirty().len(), 1);
    }

    #[test]
    fn series_channels_ring_buffer() {
        let engine = StateEngine::new(3);
        engine.register("ext.cpu", StateShape::Series, None);
        for point in [1.0, 2.0, 3.0, 4.0] {
            engine.set("ext.cpu", serde_json::json!(point));
        }
        let sync = engine.snapshot();
        assert_eq!(sync.series["ext.cpu"], vec![2.0, 3.0, 4.0]); // capped at 3
        let changes = engine.drain_dirty();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, serde_json::json!(4.0)); // newest point only
        assert!(engine.drain_dirty().is_empty());
    }

    #[test]
    fn series_accepts_the_original_custom_value_object() {
        let engine = StateEngine::new(120);
        engine.register("ext.si-load-cpu", StateShape::Series, None);
        engine.register("ext.si-load-gb-ram", StateShape::Series, None);
        // The native system-info push (JS-extension parity): object form
        // with the number inside, as the stock client consumes it too.
        engine.set(
            "ext.si-load-cpu",
            serde_json::json!({"title": "CPU Load", "description": "Ryzen", "value": "5.3", "suffix": "%"}),
        );
        engine.set(
            "ext.si-load-gb-ram",
            serde_json::json!({"title": "RAM Usage", "value": 18.7, "suffix": "GB"}),
        );
        let sync = engine.snapshot();
        assert_eq!(sync.series["ext.si-load-cpu"], vec![5.3]);
        assert_eq!(sync.series["ext.si-load-gb-ram"], vec![18.7]);
    }

    #[test]
    fn shape_change_migrates_the_value() {
        let engine = StateEngine::new(120);
        // Producer pushed before any tile declared the channel: scalar.
        engine.set("ext.cpu", serde_json::json!(0.5));
        // A graph tile then declares it series: the point seeds the buffer.
        engine.register("ext.cpu", StateShape::Series, None);
        let sync = engine.snapshot();
        assert_eq!(sync.series["ext.cpu"], vec![0.5]);
        assert!(!sync.values.contains_key("ext.cpu"));
        // And back: newest point becomes the scalar value.
        engine.register("ext.cpu", StateShape::Scalar, None);
        let sync = engine.snapshot();
        assert_eq!(sync.values["ext.cpu"], 0.5);
        assert!(!sync.series.contains_key("ext.cpu"));
    }

    #[test]
    fn unregistered_pushes_auto_register_as_scalar() {
        let engine = StateEngine::new(120);
        engine.set("ext.late", serde_json::json!("x"));
        let catalog = engine.catalog();
        assert_eq!(catalog["ext.late"].shape, StateShape::Scalar);
        assert_eq!(catalog["ext.late"].cap, None);
    }

    #[test]
    fn series_rejects_non_numeric() {
        let engine = StateEngine::new(120);
        engine.register("ext.cpu", StateShape::Series, None);
        engine.set("ext.cpu", serde_json::json!("oops"));
        // No point ever landed: the channel stays out of the snapshot and
        // nothing is dirty.
        assert!(engine
            .snapshot()
            .series
            .get("ext.cpu")
            .is_none_or(|v| v.is_empty()));
        assert!(engine.drain_dirty().is_empty());
    }
}
