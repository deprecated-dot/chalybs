use std::time::Duration;

use tracing::{info, warn};

use rumqttc::{Client, Connection, Event, Incoming, MqttOptions, QoS};

use crate::config::TasmotaConfig;
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

use super::PeripheralHook;

/// Short-lived MQTT hook for a Tasmota relay (POWER ON/OFF).
///
/// Semantics:
///   * On VM up:   publish "ON"  → cmnd/<device_id>/POWER
///   * On VM down: publish "OFF" → cmnd/<device_id>/POWER
///
/// Errors are **soft-fail** (per operator decision): warnings only,
/// and VM bring-up/shutdown MUST proceed deterministically.
pub struct TasmotaHook {
    cfg: TasmotaConfig,
}

impl TasmotaHook {
    pub fn new(cfg: TasmotaConfig) -> Self {
        Self { cfg }
    }

    /// Parse mqtt_host into ("hostname", port).
    ///
    /// Required format:
    ///     "host:port"
    ///
    /// Soft-fail: invalid format produces an error, but caller will
    /// treat failure as non-fatal and continue VM bring-up.
    fn parse_mqtt_host(&self) -> Result<(String, u16)> {
        let raw = self.cfg.mqtt_host.trim();
        if raw.is_empty() {
            return Err(ChalybsError::Peripheral(
                "tasmota: mqtt_host is empty".into(),
            ));
        }

        let mut parts = raw.split(':');
        let host = parts
            .next()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ChalybsError::Peripheral(format!("tasmota: invalid mqtt_host `{raw}`"))
            })?;

        let port_str = parts.next().ok_or_else(|| {
            ChalybsError::Peripheral(format!("tasmota: mqtt_host missing port `{raw}`"))
        })?;

        let port: u16 = port_str.parse().map_err(|_| {
            ChalybsError::Peripheral(format!("tasmota: mqtt_host has invalid port `{raw}`"))
        })?;

        Ok((host, port))
    }

    /// Publish "ON" or "OFF" to cmnd/<device_id>/POWER.
    ///
    /// ALL errors are soft-fail: warnings only, return Ok(()).
    fn publish_power(&self, on: bool) -> Result<()> {
        let (host, port) = match self.parse_mqtt_host() {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "Tasmota MQTT: invalid mqtt_host `{}`: {e}",
                    self.cfg.mqtt_host
                );
                return Ok(()); // SOFT FAIL
            }
        };

        let topic = format!("cmnd/{}/POWER", self.cfg.device_id);
        let payload = if on { "ON" } else { "OFF" };

        info!(
            broker = format!("{host}:{port}").as_str(),
            topic = topic.as_str(),
            payload = payload,
            "Tasmota MQTT: publishing POWER command"
        );

        let client_id = format!("chalybs-{}", self.cfg.device_id);
        let mut opts = MqttOptions::new(client_id, host, port);
        opts.set_keep_alive(Duration::from_secs(5));

        if let Some(ref user) = self.cfg.username {
            let pass = self.cfg.password.clone().unwrap_or_default();
            opts.set_credentials(user.clone(), pass);
        }

        let (client, mut connection): (Client, Connection) = Client::new(opts, 10);

        //
        // **DETERMINISTIC FIX:**
        // Pump event loop UNTIL we see ConnAck BEFORE publishing.
        //
        // Without this, publish() can occur before CONNECT completes,
        // causing the packet to be dropped silently.
        //
        let mut iter = connection.iter();
        let mut connected = false;

        for _ in 0..20 {
            if let Some(ev) = iter.next() {
                match ev {
                    Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                        connected = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!("Tasmota MQTT: connection error before publish: {e}");
                        return Ok(()); // SOFT FAIL
                    }
                }
            }
        }

        if !connected {
            warn!("Tasmota MQTT: never received ConnAck before publish");
            return Ok(()); // SOFT FAIL
        }

        // Publish (soft-fail)
        if let Err(e) = client.publish(topic.as_str(), QoS::AtLeastOnce, false, payload.as_bytes())
        {
            warn!("Tasmota MQTT: publish error on `{topic}`: {e}");
            return Ok(()); // SOFT FAIL
        }

        // One more poll to flush PUBLISH
        if let Some(res) = iter.next() {
            if let Err(e) = res {
                warn!("Tasmota MQTT: eventloop error after publish: {e}");
            }
        }

        Ok(())
    }
}

impl PeripheralHook for TasmotaHook {
    fn vm_up(&self, _rt: &VmRuntime) -> Result<()> {
        info!("Tasmota: VM up → POWER ON");
        self.publish_power(true)
    }

    fn vm_down(&self, _rt: &VmRuntime) -> Result<()> {
        info!("Tasmota: VM down → POWER OFF");
        self.publish_power(false)
    }
}
