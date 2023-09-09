use std::{net::IpAddr, ops::RangeInclusive};

use chrono::NaiveTime;
use easyfix_messages::fields::FixString;
use serde::{Deserialize, Deserializer};
use tokio::time::Duration;

use crate::session_id::SessionId;

fn duration_from_seconds<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Duration::from_secs(u64::deserialize(deserializer)?))
}

/// FIX Trading Port session configuration.
#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    /// IP address
    pub host: IpAddr,
    /// Port number
    pub port: u16,
    /// FIX SenderCompID<49> field value for outgoing messages.
    pub sender_comp_id: FixString,
    /// FIX SenderSubID<50> field value for outgoing messages.
    pub sender_sub_id: Option<FixString>,
    /// Timeout \[s\] for inbound/outbound messages. When reached, `TestRequest<1>`
    /// is sent when inbound message is missing or `Heartbeat<0>` is sent when
    /// outbound message is missing.
    #[serde(deserialize_with = "duration_from_seconds")]
    pub heartbeat_interval: Duration,
    /// Timeout \[s\] for `Logon<A>` message, when reached, connection is dropped.
    #[serde(deserialize_with = "duration_from_seconds")]
    pub auto_disconnect_after_no_logon_received: Duration,
    /// How many times `TestRequest<1> `is sent when inbound timeout is reached,
    /// before connection is dropped.
    pub auto_disconnect_after_no_heartbeat: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionSettings {
    pub session_id: SessionId,
    // TODO: Optional
    pub session_time: RangeInclusive<NaiveTime>,
    pub logon_time: RangeInclusive<NaiveTime>,

    pub send_redundant_resend_requests: bool,
    pub check_comp_id: bool,
    pub check_latency: bool,
    pub max_latency: Duration,

    pub reset_on_logon: bool,
    pub reset_on_logout: bool,
    pub reset_on_disconnect: bool,

    pub refresh_on_logon: bool,

    pub sender_default_appl_ver_id: FixString,
    pub target_default_appl_ver_id: FixString,

    /// Enable the next expected message sequence number (optional tag 789
    /// on Logon) on sent Logon message and use value of tag 789 on received
    /// Logon message to synchronize session.
    pub enable_next_expected_msg_seq_num: bool,

    // Enable messages persistence.
    pub persist: bool,

    // Enable Logout<5> verification.
    pub verify_logout: bool,
}
