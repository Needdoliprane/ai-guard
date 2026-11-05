//! Existing retained result data shared by its producer and cold consistency checks.
//! These records grant no authentication, journal or runtime authority.
use crate::runtime::ContainerObservation;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureFactsV1 {
    pub(crate) total_bytes: u64,
    pub(crate) truncated: bool,
    pub(crate) text_clipped: Option<bool>,
    pub(crate) text: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransportFactsV1 {
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_ms_decimal: String,
    pub(crate) stdout: CaptureFactsV1,
    pub(crate) stderr: CaptureFactsV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchResultFactsV1 {
    pub(crate) version: u32,
    pub(crate) worker_id: String,
    pub(crate) attempt_id: String,
    pub(crate) observation: ContainerObservation,
    pub(crate) transport: Option<TransportFactsV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CoverageFactsV1 {
    pub(crate) version: u32,
    pub(crate) normalized_events: Option<usize>,
    pub(crate) maximum_events: usize,
    pub(crate) event_limit_reached: Option<bool>,
    pub(crate) stdout_truncated: Option<bool>,
    pub(crate) unterminated_final_line_omitted: Option<bool>,
    pub(crate) nested_authorization: bool,
    pub(crate) raw_output_capture: bool,
}
