//! The app's one HTTP client (ADR 0010): blocking `ureq` on the system's
//! certificates, run on background threads.

use std::time::Duration;

/// An agent for the service at `base_url`, plain HTTP only when that URL is
/// (a fake server in tests). Statuses come back as answers, not errors, so
/// each caller reads them its own way.
pub fn http_agent(base_url: &str, user_agent: &str, body_timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(base_url.starts_with("https://"))
        // The system's certificates, so antivirus or company TLS inspection
        // the system trusts does not break the app's requests.
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .http_status_as_error(false)
        .user_agent(user_agent)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body_timeout))
        .build()
        .into()
}
