//! Map sampling failures onto fallback decisions.

use xai_grok_sampler::{SamplingErrorInfo, SamplingErrorKind};
use xai_grok_sampling_types::{SamplingError, is_quota_exhausted_message};

/// Why the router should leave the current hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverReason {
    RateLimited,
    QuotaExhausted,
    Unauthorized,
    ServerError,
    MissingCredential,
    CapabilityMismatch,
    ContextTooLarge,
}

impl FailoverReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Unauthorized => "unauthorized",
            Self::ServerError => "server_error",
            Self::MissingCredential => "missing_credential",
            Self::CapabilityMismatch => "capability_mismatch",
            Self::ContextTooLarge => "context_too_large",
        }
    }
}

/// Classify a rich sampler error for account failover.
pub fn classify_sampling_error(
    err: &SamplingError,
    failover_on_server_error: bool,
) -> Option<FailoverReason> {
    if err.is_context_length_error() {
        return None;
    }
    if err.is_quota_exhausted() {
        return Some(FailoverReason::QuotaExhausted);
    }
    if err.is_rate_limited() {
        return Some(FailoverReason::RateLimited);
    }
    if matches!(
        err,
        SamplingError::Api {
            status: reqwest::StatusCode::UNAUTHORIZED,
            ..
        }
    ) {
        return Some(FailoverReason::Unauthorized);
    }
    if failover_on_server_error {
        if let SamplingError::Api { status, .. } = err {
            let code = status.as_u16();
            if (500..600).contains(&code) {
                return Some(FailoverReason::ServerError);
            }
        }
    }
    if err.is_account_failover_candidate() {
        return Some(FailoverReason::QuotaExhausted);
    }
    None
}

/// Classify the shell-facing error info (after transport retries).
pub fn classify_error_info(
    info: &SamplingErrorInfo,
    failover_on_server_error: bool,
) -> Option<FailoverReason> {
    if xai_grok_sampling_types::is_context_length_error(&info.message) {
        return None;
    }
    if is_quota_exhausted_message(&info.message) || info.status_code == Some(402) {
        return Some(FailoverReason::QuotaExhausted);
    }
    if matches!(info.kind, SamplingErrorKind::RateLimited) || info.status_code == Some(429) {
        // Prefer quota language over bare rate limit when both apply.
        if is_quota_exhausted_message(&info.message) {
            return Some(FailoverReason::QuotaExhausted);
        }
        return Some(FailoverReason::RateLimited);
    }
    if matches!(info.kind, SamplingErrorKind::Auth) || info.status_code == Some(401) {
        return Some(FailoverReason::Unauthorized);
    }
    if failover_on_server_error {
        if let Some(code) = info.status_code {
            if (500..600).contains(&code) {
                return Some(FailoverReason::ServerError);
            }
        }
    }
    None
}

/// Runtime account routing is intentionally narrower than diagnostics: only
/// confirmed throttling or exhausted quota may advance to another provider.
pub fn runtime_failover_reason(info: &SamplingErrorInfo) -> Option<FailoverReason> {
    classify_error_info(info, false).filter(|reason| {
        matches!(
            reason,
            FailoverReason::RateLimited | FailoverReason::QuotaExhausted
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn api(status: StatusCode, message: &str) -> SamplingError {
        SamplingError::Api {
            status,
            message: message.into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
        }
    }

    #[test]
    fn rate_limit_classifies() {
        let err = api(StatusCode::TOO_MANY_REQUESTS, "slow down");
        assert_eq!(
            classify_sampling_error(&err, false),
            Some(FailoverReason::RateLimited)
        );
    }

    #[test]
    fn quota_classifies() {
        let err = api(
            StatusCode::TOO_MANY_REQUESTS,
            "insufficient_quota: exceeded",
        );
        assert_eq!(
            classify_sampling_error(&err, false),
            Some(FailoverReason::QuotaExhausted)
        );
    }

    #[test]
    fn context_length_does_not_failover() {
        let err = api(StatusCode::BAD_REQUEST, "maximum context length exceeded");
        assert_eq!(classify_sampling_error(&err, false), None);
    }

    #[test]
    fn server_error_optional() {
        let err = api(StatusCode::BAD_GATEWAY, "bad gateway");
        assert_eq!(classify_sampling_error(&err, false), None);
        assert_eq!(
            classify_sampling_error(&err, true),
            Some(FailoverReason::ServerError)
        );
    }

    fn info(kind: SamplingErrorKind, status_code: Option<u16>, message: &str) -> SamplingErrorInfo {
        SamplingErrorInfo {
            kind,
            status_code,
            message: message.to_owned(),
            is_retryable: false,
            retry_after_secs: None,
            model_metadata: None,
            empty_response_context: None,
            doom_loop_triggers: None,
            doom_loop_aborted_at_chunk: None,
        }
    }

    #[test]
    fn runtime_advances_only_for_rate_limit_or_quota_by_default() {
        assert_eq!(
            runtime_failover_reason(&info(
                SamplingErrorKind::RateLimited,
                Some(429),
                "slow down",
            )),
            Some(FailoverReason::RateLimited)
        );
        assert_eq!(
            runtime_failover_reason(&info(
                SamplingErrorKind::Api,
                Some(429),
                "insufficient_quota",
            )),
            Some(FailoverReason::QuotaExhausted)
        );
        assert_eq!(
            runtime_failover_reason(&info(SamplingErrorKind::Auth, Some(401), "unauthorized",)),
            None
        );
        assert_eq!(
            runtime_failover_reason(&info(SamplingErrorKind::Api, Some(500), "server error",)),
            None
        );
        assert_eq!(
            runtime_failover_reason(&info(SamplingErrorKind::Http, None, "network error")),
            None
        );
        assert_eq!(
            runtime_failover_reason(&info(
                SamplingErrorKind::Api,
                Some(400),
                "maximum context length exceeded",
            )),
            None
        );
    }
}
