//! What every Offer Post destination does with a request and a photo: tell
//! a request that never left from one whose fate is unknown, and read the
//! photo from the Product's folder.

use std::io::ErrorKind;
use std::path::Path;

use mascate_kernel::PlatformError;
use mascate_marketing::PublishFailure;

/// A failure before the post's request left, getting the login or reading
/// what the destination needs first: nothing went out.
pub(crate) fn before_sending(error: PlatformError) -> PublishFailure {
    match error {
        PlatformError::NotConnected => PublishFailure::NotConnected,
        PlatformError::Expired => PublishFailure::Expired,
        PlatformError::RateLimited => PublishFailure::RateLimited,
        PlatformError::Refused(why) | PlatformError::Failed(why) => PublishFailure::NotSent(why),
        PlatformError::NotFound => PublishFailure::NotSent(error.to_string()),
    }
}

/// Whether ureq failed before any of the request left: the address did
/// not resolve, the connection or the TLS handshake failed.
pub(crate) fn never_sent(error: &ureq::Error) -> bool {
    if let ureq::Error::Io(error) = error {
        return matches!(
            error.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::HostUnreachable
                | ErrorKind::NetworkUnreachable
                | ErrorKind::AddrNotAvailable
                | ErrorKind::PermissionDenied
        );
    }
    matches!(
        error,
        ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect)
            | ureq::Error::BadUri(_)
            | ureq::Error::RequireHttpsOnly(_)
            | ureq::Error::InvalidProxyUrl
            | ureq::Error::ConnectProxyFailed(_)
            | ureq::Error::Tls(_)
            | ureq::Error::TlsRequired
    )
}

/// A photo read for upload.
pub(crate) struct Photo {
    /// The file's name, without quotes or line breaks.
    pub name: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The photo at `path`, if `platform` takes it: JPG or PNG, up to
/// `max_bytes`. Nothing has been sent when it fails.
pub(crate) fn read_photo(
    path: &Path,
    platform: &str,
    max_bytes: u64,
) -> Result<Photo, PublishFailure> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let content_type = match extension.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        _ => {
            return Err(PublishFailure::NotSent(format!(
                "o {platform} só recebe foto em JPG ou PNG"
            )));
        }
    };
    let unreadable = |error: std::io::Error| {
        PublishFailure::NotSent(format!("não consegui ler a foto: {error}"))
    };
    if std::fs::metadata(path).map_err(unreadable)?.len() > max_bytes {
        return Err(PublishFailure::NotSent(format!(
            "a foto passa de {} MB, o maior arquivo que o {platform} recebe",
            max_bytes / (1024 * 1024)
        )));
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    // Quotes and line breaks would end a part's header early.
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "foto".into())
        .replace(['"', '\\', '\r', '\n'], "_");
    Ok(Photo {
        name,
        content_type,
        bytes,
    })
}
