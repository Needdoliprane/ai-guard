//! Bounded diagnostic metadata. A stage does not establish whether an effect
//! occurred, a container is terminal, or an earlier start has been fenced.
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimePrestartStage {
    ObserveOnly,
    AuthorityBeforeSpawn,
    InvocationBeforeSpawn,
    StopBeforeSpawn,
    GuardBeforeSpawn,
    Spawn,
    StartedObservation,
}

#[derive(Clone, Serialize)]
pub(crate) struct RuntimePrestartStageFailure {
    stage: RuntimePrestartStage,
    container_id: Option<String>,
}

impl RuntimePrestartStageFailure {
    pub(crate) fn observe_only() -> Self {
        Self {
            stage: RuntimePrestartStage::ObserveOnly,
            container_id: None,
        }
    }

    pub(crate) fn new(stage: RuntimePrestartStage, container_id: &str) -> Self {
        let valid = container_id.len() == 64
            && container_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        Self {
            stage,
            container_id: valid.then(|| container_id.to_owned()),
        }
    }
    #[cfg(test)]
    pub(crate) fn stage(&self) -> RuntimePrestartStage {
        self.stage
    }
    #[cfg(test)]
    pub(crate) fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }
}

impl fmt::Display for RuntimePrestartStageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("runtime stage failed; execution requires reconciliation")
    }
}
impl fmt::Debug for RuntimePrestartStageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RuntimePrestartStageFailure(<private>)")
    }
}
impl std::error::Error for RuntimePrestartStageFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct NativeRefusal;
    impl fmt::Display for NativeRefusal {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("opaque native refusal")
        }
    }
    impl std::error::Error for NativeRefusal {}

    #[test]
    fn stage_context_preserves_native_downcast_and_bounds_metadata() {
        let cid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let failure = RuntimePrestartStageFailure::new(RuntimePrestartStage::GuardBeforeSpawn, cid);
        let error = anyhow::Error::new(NativeRefusal)
            .context(failure.clone())
            .context("owned runtime result requires reconciliation");
        assert!(error.downcast_ref::<NativeRefusal>().is_some());
        let retained = error.downcast_ref::<RuntimePrestartStageFailure>().unwrap();
        assert_eq!(retained.stage(), RuntimePrestartStage::GuardBeforeSpawn);
        assert_eq!(retained.container_id(), Some(cid));
        assert_eq!(
            serde_json::to_value(retained).unwrap(),
            serde_json::json!({"stage":"guard_before_spawn","container_id":cid})
        );
        assert!(!format!("{failure} {failure:?}").contains(cid));
        let untrusted = "untrusted-argv-\u{1f600}".repeat(128);
        let bounded = RuntimePrestartStageFailure::new(RuntimePrestartStage::Spawn, &untrusted);
        assert_eq!(bounded.container_id(), None);
        assert_eq!(
            serde_json::to_value(bounded).unwrap()["container_id"],
            serde_json::Value::Null
        );
    }
}
