//! Exercise the real public mapper with its actual typed error values.
use super::*;
use crate::human_authority::HumanAuthorityFailure;

#[test]
fn expired_authority_is_a_transient_identity_read_refusal() {
    let direct = anyhow::Error::from(HumanAuthorityFailure::EpochExpired);
    let wrapped = anyhow::Error::from(HumanAuthorityFailure::EpochExpired)
        .context("private-epoch-context-canary");
    for cause in [direct, wrapped] {
        let (status, Json(body)) = store_failure(cause);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "identity_unavailable");
        assert_eq!(body["error"]["retry_action"], "retry");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.to_ascii_lowercase().contains("expired"));
        assert!(!message.to_ascii_lowercase().contains("audit"));
        assert!(message.len() <= 256);
        assert!(!body.to_string().contains("private-epoch-context-canary"));
        assert_eq!(body.as_object().unwrap().len(), 1);
        assert_eq!(body["error"].as_object().unwrap().len(), 3);
    }
}

#[test]
fn credential_expiration_keeps_its_authentication_refusal() {
    let cause = anyhow::Error::from(IdentityFailureV1::CredentialExpired)
        .context("private-credential-context-canary");
    let (status, Json(body)) = store_failure(cause);
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body,
        json!({"error": {
            "code": "credential_expired",
            "message": "The local credential has expired.",
            "retry_action": "authenticate"
        }})
    );
}

#[test]
fn storage_and_other_authority_failures_keep_opaque_operator_refusals() {
    use HumanAuthorityFailure::*;
    let unchanged = [
        InvalidManifest,
        ConfigurationMismatch,
        StorageBindingMismatch,
        StorageUnavailable,
        OwnerContended,
        OwnerInvalid,
        Closing,
        GateBusy,
        SourceMismatch,
        TaskFailed,
        OpeningTimedOut,
    ];
    let storage_error = std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "private-storage-detail-canary",
    );
    let causes = unchanged
        .into_iter()
        .map(|failure| anyhow::Error::from(failure).context("private-authority-context-canary"))
        .chain(std::iter::once(anyhow::Error::from(storage_error)));
    for cause in causes {
        let (status, Json(body)) = store_failure(cause);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            json!({"error": {
                "code": "audit_unavailable",
                "message": "The required identity or audit store is unavailable.",
                "retry_action": "contact_operator"
            }})
        );
    }
}
