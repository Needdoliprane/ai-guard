//! Fixed authorization-query route sharing the retained HTTPS client and slots.
use super::{Transport, read_response_bytes_limited, transport_error};
use crate::runtime::kubernetes::{
    KubernetesError, KubernetesResult, prerequisites::KubernetesPermissionActionV1,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const MAX_SSAR_REQUEST_BYTES: usize = 4096;
const MAX_SSAR_RESPONSE_BYTES: usize = 32 * 1024;

impl Transport {
    pub(in crate::runtime::kubernetes) async fn get_unique<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> KubernetesResult<T> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let response = self
            .client
            .get(format!("{}{path}", self.endpoint))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(KubernetesError::ApiRejected(response.status().as_u16()));
        }
        let bytes = read_response_bytes_limited(response, super::MAX_RESPONSE_BYTES).await?;
        let value =
            crate::json::decode_unique(&bytes, 64).map_err(|_| KubernetesError::InvalidResponse)?;
        serde_json::from_value(value).map_err(|_| KubernetesError::InvalidResponse)
    }

    /// Authorization query only, with no caller-provided path, identity or body.
    pub(in crate::runtime::kubernetes) async fn self_subject_access_review(
        &self,
        namespace: &str,
        action: KubernetesPermissionActionV1,
    ) -> KubernetesResult<Value> {
        if !super::super::valid_dns_label(namespace) {
            return Err(KubernetesError::InvalidAuthority);
        }
        let body = serde_json::to_vec(&json!({
            "apiVersion":"authorization.k8s.io/v1", "kind":"SelfSubjectAccessReview",
            "spec":{"resourceAttributes":action.attributes(namespace)}
        }))
        .map_err(|_| KubernetesError::InvalidResponse)?;
        if body.len() > MAX_SSAR_REQUEST_BYTES {
            return Err(KubernetesError::InvalidResponse);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let response = self
            .client
            .post(format!(
                "{}/apis/authorization.k8s.io/v1/selfsubjectaccessreviews",
                self.endpoint
            ))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() != reqwest::StatusCode::CREATED {
            return Err(KubernetesError::ApiRejected(response.status().as_u16()));
        }
        let bytes = read_response_bytes_limited(response, MAX_SSAR_RESPONSE_BYTES).await?;
        crate::json::decode_unique(&bytes, 16).map_err(|_| KubernetesError::InvalidResponse)
    }
}
