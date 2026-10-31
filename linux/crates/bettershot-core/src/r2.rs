//! Port of the portable parts of `Sources/Sharing/R2Uploader.swift` and
//! `R2CredentialStore.swift`: AWS SigV4 signing for Cloudflare R2, endpoint
//! selection, request construction, and response interpretation. Sending the
//! requests and storing secrets (Secret Service on Linux) live in the platform
//! layer, so this module never touches the network or a keyring.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
pub const REGION: &str = "auto";
pub const SERVICE: &str = "s3";
pub const CONNECTION_PROBE_KEY: &str = "bettershot-connection-probe";

/// Non-secret preference keys, as on macOS. Secrets go to the keyring.
pub mod keys {
    pub const ACCOUNT_ID: &str = "bs_r2_accountID";
    pub const BUCKET: &str = "bs_r2_bucket";
    pub const PUBLIC_BASE_URL: &str = "bs_r2_publicBaseURL";
    pub const USE_DIRECT_LINKS: &str = "bs_r2_useDirectLinks";
    pub const ENABLED: &str = "bs_r2_enabled";
    pub const ENABLED_MIGRATED: &str = "bs_r2_enabledMigrated";
    pub const JURISDICTION: &str = "bs_r2_jurisdiction";
    pub const ACCESS_KEY_ID: &str = "accessKeyID";
    pub const SECRET_ACCESS_KEY: &str = "secretAccessKey";
}

/// Where an R2 bucket's data is held. A jurisdictional bucket is served only
/// from that jurisdiction's endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Jurisdiction {
    #[default]
    Standard,
    Eu,
    Us,
    Fedramp,
}

impl Jurisdiction {
    pub const ALL: [Self; 4] = [Self::Standard, Self::Eu, Self::Us, Self::Fedramp];

    pub fn raw_value(self) -> &'static str {
        match self {
            Self::Standard => "",
            Self::Eu => "eu",
            Self::Us => "us",
            Self::Fedramp => "fedramp",
        }
    }

    /// Unknown stored values fall back to the standard endpoint.
    pub fn from_raw_value(raw: &str) -> Self {
        Self::ALL.into_iter().find(|j| j.raw_value() == raw).unwrap_or_default()
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Standard => "None",
            Self::Eu => "European Union",
            Self::Us => "United States",
            Self::Fedramp => "FedRAMP",
        }
    }
}

/// Immutable snapshot of R2 settings and secrets.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Credentials {
    pub account_id: String,
    pub bucket: String,
    pub public_base_url: String,
    pub use_direct_links: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub enabled: bool,
    pub jurisdiction: Jurisdiction,
}

impl std::fmt::Debug for Credentials {
    /// Never prints secrets, even in logs or panic messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("account_id", &self.account_id)
            .field("bucket", &self.bucket)
            .field("public_base_url", &self.public_base_url)
            .field("use_direct_links", &self.use_direct_links)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("enabled", &self.enabled)
            .field("jurisdiction", &self.jurisdiction)
            .finish()
    }
}

impl Credentials {
    pub fn host(&self) -> String {
        match self.jurisdiction {
            Jurisdiction::Standard => format!("{}.r2.cloudflarestorage.com", self.account_id),
            other => format!("{}.{}.r2.cloudflarestorage.com", self.account_id, other.raw_value()),
        }
    }

    pub fn is_configured(&self) -> bool {
        [&self.account_id, &self.bucket, &self.public_base_url, &self.access_key_id, &self.secret_access_key]
            .iter()
            .all(|value| !value.is_empty())
    }

    /// Why Share must not upload; None when it may. "Upload when I share" is
    /// the consent to publish.
    pub fn share_blocker(&self) -> Option<&'static str> {
        if !self.is_configured() {
            Some("R2 sharing is not configured. Add your credentials in Settings > Sharing.")
        } else if !self.enabled {
            Some("Uploads are off. Turn on Upload when I share in Settings > Sharing.")
        } else {
            None
        }
    }
}

/// Installs that saved keys before the toggle existed turn uploads on once;
/// after that migration any missing value means off.
pub fn resolved_enabled(stored: Option<bool>, has_keys: bool, has_migrated: bool) -> bool {
    stored.unwrap_or(!has_migrated && has_keys)
}

// MARK: - Signing

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub amz_date: String,
    pub content_sha256: String,
    pub authorization_header: String,
}

pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub fn hmac(key: &[u8], message: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn derive_signing_key(secret_access_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret_access_key}").as_bytes(), date_stamp);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, service);
    hmac(&k_service, "aws4_request")
}

/// SigV4 URI encoding: unreserved characters and `/` stay, everything else is
/// percent-encoded as UTF-8 with uppercase hex.
pub fn uri_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Signs a request with an unsigned payload, as every BetterShot R2 request is.
pub fn sign(
    method: &str,
    host: &str,
    canonical_uri: &str,
    access_key_id: &str,
    secret_access_key: &str,
    content_type: &str,
    date: DateTime<Utc>,
) -> Signature {
    let amz_date = date.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = &amz_date[..8];
    let canonical_headers = format!(
        "content-type:{content_type}\nhost:{host}\nx-amz-content-sha256:{UNSIGNED_PAYLOAD}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        [method, canonical_uri, "", &canonical_headers, signed_headers, UNSIGNED_PAYLOAD].join("\n");
    let credential_scope = format!("{date_stamp}/{REGION}/{SERVICE}/aws4_request");
    let string_to_sign = [ALGORITHM, &amz_date, &credential_scope, &sha256_hex(&canonical_request)].join("\n");
    let signing_key = derive_signing_key(secret_access_key, date_stamp, REGION, SERVICE);
    let signature = hex::encode(hmac(&signing_key, &string_to_sign));
    Signature {
        authorization_header: format!(
            "{ALGORITHM} Credential={access_key_id}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
        ),
        amz_date,
        content_sha256: UNSIGNED_PAYLOAD.to_owned(),
    }
}

// MARK: - Requests

/// A signed request ready for any HTTP client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// Builds a signed request for `key` in the configured bucket.
pub fn object_request(
    method: &str,
    key: &str,
    content_type: &str,
    credentials: &Credentials,
    date: DateTime<Utc>,
) -> SignedRequest {
    let host = credentials.host();
    let canonical_uri = format!("/{}/{}", uri_encode_path(&credentials.bucket), uri_encode_path(key));
    let signature = sign(
        method,
        &host,
        &canonical_uri,
        &credentials.access_key_id,
        &credentials.secret_access_key,
        content_type,
        date,
    );
    SignedRequest {
        method: method.to_owned(),
        url: format!("https://{host}{canonical_uri}"),
        headers: vec![
            ("Content-Type".into(), content_type.into()),
            ("x-amz-date".into(), signature.amz_date),
            ("x-amz-content-sha256".into(), signature.content_sha256),
            ("Authorization".into(), signature.authorization_header),
        ],
    }
}

/// The probe Test Connection sends. A missing probe object still proves the
/// bucket and keys work.
pub fn connection_test_request(credentials: &Credentials, date: DateTime<Utc>) -> Result<SignedRequest, String> {
    if !credentials.is_configured() {
        return Err("Fill in all fields before testing.".into());
    }
    Ok(object_request("GET", CONNECTION_PROBE_KEY, "application/octet-stream", credentials, date))
}

/// Interprets the response to [`connection_test_request`].
pub fn connection_test_result(status: u16, body: &str, credentials: &Credentials) -> Result<(), String> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if status == 404 && extract_xml_value("Code", body).as_deref() != Some("NoSuchBucket") {
        return Ok(());
    }
    Err(error_message(body, status, credentials))
}

/// A readable message from an R2 XML error. `AccessDenied` usually means the
/// bucket lives in a jurisdiction, so it names the setting and the host used.
pub fn error_message(body: &str, status: u16, credentials: &Credentials) -> String {
    let Some(code) = extract_xml_value("Code", body) else {
        return format!("R2 request failed with status {status}.");
    };
    let mut message = code.clone();
    if let Some(detail) = extract_xml_value("Message", body).filter(|detail| !detail.is_empty()) {
        message = format!("{code}: {detail}");
    }
    if code != "AccessDenied" {
        return message;
    }
    format!(
        "{message}. If the keys are correct, set Jurisdiction in Settings > Sharing to match the bucket (requests went to {}).",
        credentials.host()
    )
}

pub fn extract_xml_value(tag: &str, xml: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    (start <= end).then(|| xml[start..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Mirrors `Tests/R2SigV4Check.swift`: AWS's published GET Object vector.
    #[test]
    fn aws_official_get_object_vector() {
        let canonical_request = [
            "GET",
            "/test.txt",
            "",
            "host:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n",
            "host;range;x-amz-content-sha256;x-amz-date",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ]
        .join("\n");
        let hash = sha256_hex(&canonical_request);
        assert_eq!(hash, "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972");
        let string_to_sign =
            ["AWS4-HMAC-SHA256", "20130524T000000Z", "20130524/us-east-1/s3/aws4_request", &hash].join("\n");
        let key = derive_signing_key("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLE", "20130524", "us-east-1", "s3");
        assert_eq!(hex::encode(&key), "db833e0f5e435b208142db4786ec9153e01cc2cde3b2f7ec5083d8810df17b14");
        assert_eq!(
            hex::encode(hmac(&key, &string_to_sign)),
            "35788a3fc1643e1b1ea7f1e67b4fde26dbfef66fd5d75519c81e5914c5ce2003"
        );
    }

    /// Mirrors the R2 upload-style vector, run through the production `sign`.
    #[test]
    fn r2_upload_style_vector() {
        let canonical_uri = uri_encode_path("/mybucket/recordings/abc-123/My File (final).mp4");
        assert_eq!(canonical_uri, "/mybucket/recordings/abc-123/My%20File%20%28final%29.mp4");
        let date = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        let signature = sign(
            "PUT",
            "test-account.r2.cloudflarestorage.com",
            &canonical_uri,
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLE",
            "video/quicktime",
            date,
        );
        assert_eq!(signature.amz_date, "20240115T103000Z");
        assert_eq!(signature.content_sha256, "UNSIGNED-PAYLOAD");
        assert_eq!(
            hex::encode(derive_signing_key("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLE", "20240115", "auto", "s3")),
            "5a9eda12430c696b70e82fb9774d9d09a92532fb81e1bf096c22eec47cc6b2fc"
        );
        assert_eq!(
            signature.authorization_header,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240115/auto/s3/aws4_request, SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date, Signature=91c4717ab69ef61db3fe119f890325d5c7ce8c19f0c8df7b4e85521e70214491"
        );
        assert_eq!(uri_encode_path("é"), "%C3%A9");
    }

    fn credentials(account: &str, jurisdiction: Jurisdiction) -> Credentials {
        Credentials {
            account_id: account.into(),
            bucket: "bucket".into(),
            public_base_url: "https://share.example.com".into(),
            use_direct_links: false,
            access_key_id: "test-only-key".into(),
            secret_access_key: "test-only-secret".into(),
            enabled: true,
            jurisdiction,
        }
    }

    /// Mirrors `Tests/R2EndpointCheck.swift`, without a network stub: the
    /// request URL and response interpretation are pure here.
    #[test]
    fn requests_reach_the_jurisdiction_endpoint() {
        let date = Utc::now();
        for (jurisdiction, host) in [
            (Jurisdiction::Standard, "acct.r2.cloudflarestorage.com"),
            (Jurisdiction::Eu, "acct.eu.r2.cloudflarestorage.com"),
            (Jurisdiction::Us, "acct.us.r2.cloudflarestorage.com"),
            (Jurisdiction::Fedramp, "acct.fedramp.r2.cloudflarestorage.com"),
        ] {
            let creds = credentials("acct", jurisdiction);
            let request = connection_test_request(&creds, date).unwrap();
            assert_eq!(request.url, format!("https://{host}/bucket/{CONNECTION_PROBE_KEY}"));
            let denied = "<Error><Code>AccessDenied</Code><Message>AccessDenied</Message></Error>";
            let message = connection_test_result(403, denied, &creds).unwrap_err();
            assert!(message.starts_with("AccessDenied: AccessDenied."), "{message}");
            assert!(message.contains("set Jurisdiction in Settings > Sharing") && message.contains(host), "{message}");
        }
        let workaround = credentials("acct.eu", Jurisdiction::Standard);
        assert_eq!(
            workaround.host(),
            "acct.eu.r2.cloudflarestorage.com",
            "the Account ID workaround keeps reaching the EU endpoint"
        );

        let eu = credentials("acct", Jurisdiction::Eu);
        assert_eq!(
            connection_test_result(404, "<Error><Code>NoSuchKey</Code></Error>", &eu),
            Ok(()),
            "a missing probe is a working connection"
        );
        assert!(connection_test_result(404, "<Error><Code>NoSuchBucket</Code></Error>", &eu).is_err());
        let other = connection_test_result(403, "<Error><Code>SignatureDoesNotMatch</Code></Error>", &eu).unwrap_err();
        assert!(!other.contains("Jurisdiction"), "only AccessDenied suggests the jurisdiction: {other}");
        assert_eq!(connection_test_result(500, "", &eu).unwrap_err(), "R2 request failed with status 500.");
        assert!(connection_test_request(&Credentials::default(), date).is_err());
        assert_eq!(Jurisdiction::from_raw_value("future"), Jurisdiction::Standard);
    }

    /// Mirrors the pure parts of `Tests/R2UploadToggleCheck.swift`.
    #[test]
    fn upload_toggle() {
        let mut creds = credentials("acct", Jurisdiction::Standard);
        creds.enabled = false;
        assert!(creds.share_blocker().unwrap().contains("Uploads are off"));
        creds.enabled = true;
        assert_eq!(creds.share_blocker(), None);
        assert!(Credentials::default().share_blocker().unwrap().contains("not configured"));

        assert!(resolved_enabled(None, true, false), "an upgrader's first launch turns sharing on once");
        assert!(!resolved_enabled(None, false, false), "a fresh install has no keys, so it resolves to off");
        assert!(!resolved_enabled(None, true, true), "once migrated, a missing value never re-infers on");
        assert!(!resolved_enabled(Some(false), true, true));
        assert!(!resolved_enabled(Some(false), true, false), "an explicit off is never overwritten");
        assert!(resolved_enabled(Some(true), false, true), "an explicit on is never overwritten");
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let text = format!("{:?}", credentials("acct", Jurisdiction::Eu));
        assert!(!text.contains("test-only-secret") && !text.contains("test-only-key"), "{text}");
    }
}
