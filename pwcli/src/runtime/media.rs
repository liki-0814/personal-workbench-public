use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures::StreamExt;

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

pub struct FetchedImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

pub struct ImageFetchError {
    pub status: reqwest::StatusCode,
    pub message: String,
}

pub async fn fetch_remote_image(request_url: &str) -> Result<FetchedImage, ImageFetchError> {
    let mut url = reqwest::Url::parse(request_url.trim())
        .map_err(|_| fetch_error(reqwest::StatusCode::BAD_REQUEST, "Invalid image URL"))?;

    for redirect_count in 0..=3 {
        let (client, checked_url) = restricted_client(&url, true)
            .await
            .map_err(|message| fetch_error(reqwest::StatusCode::BAD_REQUEST, &message))?;
        let upstream = client
            .get(checked_url.clone())
            .send()
            .await
            .map_err(|cause| fetch_error(reqwest::StatusCode::BAD_GATEWAY, &cause.to_string()))?;
        if upstream.status().is_redirection() {
            if redirect_count == 3 {
                return Err(fetch_error(
                    reqwest::StatusCode::BAD_GATEWAY,
                    "Too many image redirects",
                ));
            }
            let location = upstream
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    fetch_error(
                        reqwest::StatusCode::BAD_GATEWAY,
                        "Redirect has no valid location",
                    )
                })?;
            url = checked_url.join(location).map_err(|_| {
                fetch_error(reqwest::StatusCode::BAD_GATEWAY, "Invalid redirect URL")
            })?;
            continue;
        }
        if !upstream.status().is_success() {
            return Err(fetch_error(
                reqwest::StatusCode::BAD_GATEWAY,
                &format!("Image server returned {}", upstream.status()),
            ));
        }
        if upstream
            .content_length()
            .is_some_and(|size| size > MAX_IMAGE_BYTES as u64)
        {
            return Err(fetch_error(
                reqwest::StatusCode::PAYLOAD_TOO_LARGE,
                "Image exceeds 20MB limit",
            ));
        }
        let declared_mime = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !declared_mime.starts_with("image/") {
            return Err(fetch_error(
                reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Remote content is not an image",
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = upstream.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|cause| {
                fetch_error(reqwest::StatusCode::BAD_GATEWAY, &cause.to_string())
            })?;
            if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
                return Err(fetch_error(
                    reqwest::StatusCode::PAYLOAD_TOO_LARGE,
                    "Image exceeds 20MB limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let detected_mime = detect_raster_mime(&bytes).ok_or_else(|| {
            fetch_error(
                reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Unsupported or invalid image data",
            )
        })?;
        if !mime_matches(&declared_mime, detected_mime) {
            return Err(fetch_error(
                reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Image MIME does not match its content",
            ));
        }
        return Ok(FetchedImage {
            bytes,
            mime: detected_mime,
        });
    }
    Err(fetch_error(
        reqwest::StatusCode::BAD_GATEWAY,
        "Unable to fetch image",
    ))
}

pub(crate) async fn restricted_client(
    url: &reqwest::Url,
    https_only: bool,
) -> Result<(reqwest::Client, reqwest::Url), String> {
    let scheme_allowed = if https_only {
        url.scheme() == "https"
    } else {
        matches!(url.scheme(), "http" | "https")
    };
    if !scheme_allowed || !url.username().is_empty() || url.password().is_some() {
        return Err(if https_only {
            "Only credential-free HTTPS URLs are allowed".into()
        } else {
            "Only credential-free HTTP(S) URLs are allowed".into()
        });
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Image URL has no host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|cause| format!("Unable to resolve image host: {cause}"))?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err("Image host resolves to a private or local address".into());
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30));
    for address in addresses {
        builder = builder.resolve(host, address);
    }
    let client = builder
        .build()
        .map_err(|cause| format!("Unable to create image client: {cause}"))?;
    Ok((client, url.clone()))
}

pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            !(first == 0
                || ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || (first == 100 && (64..=127).contains(&second))
                || (first == 192 && second == 0 && third == 0)
                || (first == 198 && matches!(second, 18 | 19))
                || first >= 240)
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(ipv4));
            }
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] & 0xffc0) == 0xfec0)
        }
    }
}

pub fn detect_raster_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn mime_matches(declared: &str, detected: &str) -> bool {
    declared == detected || (declared == "image/jpg" && detected == "image/jpeg")
}

fn fetch_error(status: reqwest::StatusCode, message: &str) -> ImageFetchError {
    ImageFetchError {
        status,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_raster_magic() {
        assert_eq!(
            detect_raster_mime(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_raster_mime(b"<html>nope</html>"), None);
    }

    #[test]
    fn rejects_local_network_addresses() {
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("169.254.169.254".parse().unwrap()));
        assert!(!is_public_ip("100.64.0.1".parse().unwrap()));
        assert!(!is_public_ip("198.18.0.1".parse().unwrap()));
        assert!(!is_public_ip("fc00::1".parse().unwrap()));
        assert!(!is_public_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("2001:db8::1".parse().unwrap()));
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn restricted_client_rejects_local_dns_targets() {
        let url = reqwest::Url::parse("http://localhost:3456/private").unwrap();
        assert!(restricted_client(&url, false).await.is_err());
    }
}
