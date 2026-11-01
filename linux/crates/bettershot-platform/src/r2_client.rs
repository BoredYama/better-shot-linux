//! Sends the R2 requests `bettershot_core::r2` signs: the network half of
//! `Sources/Sharing/R2Uploader.swift`. Calls block, so run them off the main
//! thread. Credentials are never logged, and errors never include them.
//!
//! With `BETTERSHOT_TESTING=1`, `BETTERSHOT_R2_ENDPOINT` (for example
//! `http://127.0.0.1:8123`) replaces `https://<account>.r2.cloudflarestorage.com`
//! so tests can talk to a local fake. It is ignored otherwise.

use bettershot_core::r2::{self, Credentials, SignedRequest};
use bettershot_core::share;
use chrono::Utc;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "The upload was cancelled."),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl From<String> for UploadError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

pub struct Client {
    agent: ureq::Agent,
    endpoint: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

/// What to share.
pub struct ShareRequest<'a> {
    pub item_id: uuid::Uuid,
    pub file: &'a Path,
    pub title: Option<String>,
    pub app_version: &'a str,
}

/// What an upload produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uploaded {
    pub link: String,
    pub filename: String,
}

impl Client {
    pub fn new() -> Self {
        let testing = std::env::var("BETTERSHOT_TESTING").as_deref() == Ok("1");
        let endpoint = testing.then(|| std::env::var("BETTERSHOT_R2_ENDPOINT").ok()).flatten();
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        let mut config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(600)))
            .tls_config(tls)
            .user_agent(concat!("BetterShot/", env!("CARGO_PKG_VERSION"), " (Linux)"));
        if endpoint.is_some() {
            // The local fake is reached directly, never through a proxy.
            config = config.proxy(None);
        }
        Self { agent: config.build().into(), endpoint }
    }

    fn url(&self, request: &SignedRequest, credentials: &Credentials) -> String {
        match &self.endpoint {
            Some(endpoint) => request.url.replacen(&format!("https://{}", credentials.host()), endpoint, 1),
            None => request.url.clone(),
        }
    }

    /// Sends a request; returns the status and body text.
    fn send(
        &self,
        request: &SignedRequest,
        credentials: &Credentials,
        body: Option<&[u8]>,
    ) -> Result<(u16, String), String> {
        let url = self.url(request, credentials);
        let builder = ureq::http::Request::builder().method(request.method.as_str()).uri(&url);
        let builder = request.headers.iter().fold(builder, |b, (name, value)| b.header(name, value));
        let result = match body {
            Some(bytes) => self.agent.run(builder.body(bytes).map_err(|e| e.to_string())?),
            None => self.agent.run(builder.body(()).map_err(|e| e.to_string())?),
        };
        let mut response = result
            .map_err(|error| format!("Couldn’t reach R2 ({error}). Check your internet connection and try again."))?;
        let status = response.status().as_u16();
        let mut text = String::new();
        let _ = response.body_mut().as_reader().take(1 << 20).read_to_string(&mut text);
        Ok((status, text))
    }

    /// Test Connection: a GET of a probe key proves the keys and bucket work.
    pub fn test_connection(&self, credentials: &Credentials) -> Result<(), String> {
        let request = r2::connection_test_request(credentials, Utc::now())?;
        let (status, body) = self.send(&request, credentials, None)?;
        r2::connection_test_result(status, &body, credentials)
    }

    fn put(&self, credentials: &Credentials, key: &str, content_type: &str, bytes: &[u8]) -> Result<(), String> {
        let request = r2::object_request("PUT", key, content_type, credentials, Utc::now());
        let (status, body) = self.send(&request, credentials, Some(bytes))?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(r2::error_message(&body, status, credentials))
        }
    }

    /// Uploads the request's file with its poster and manifest under a new
    /// share ID and returns the link: the viewer page, or the file itself with
    /// direct links. `progress` receives 0...1 after each object.
    pub fn upload_share(
        &self,
        credentials: &Credentials,
        request: ShareRequest<'_>,
        progress: &dyn Fn(f64),
        cancel: &AtomicBool,
    ) -> Result<Uploaded, UploadError> {
        let ShareRequest { item_id, file, title, app_version } = request;
        if let Some(blocker) = credentials.share_blocker() {
            return Err(UploadError::Failed(blocker.into()));
        }
        let id = share::slug(&item_id);
        let contents = share::make_image_bundle(&id, file, title, app_version, Utc::now());
        let link = share::share_link(
            &id,
            &contents.media_filename,
            &credentials.public_base_url,
            credentials.use_direct_links,
        )?;
        let prefix = share::object_prefix(&id);
        let media = std::fs::read(file).map_err(|e| format!("The capture couldn’t be read for upload: {e}"))?;
        let manifest = serde_json::to_vec(&contents.manifest).map_err(|e| e.to_string())?;
        let poster_len = contents.poster.as_ref().map_or(0, Vec::len);
        let total = (media.len() + poster_len + manifest.len()).max(1) as f64;

        let check = || if cancel.load(Ordering::Relaxed) { Err(UploadError::Cancelled) } else { Ok(()) };
        check()?;
        progress(0.0);
        self.put(credentials, &format!("{prefix}{}", contents.media_filename), &contents.media_mime_type, &media)?;
        progress(media.len() as f64 / total);
        check()?;
        if let Some(poster) = &contents.poster {
            self.put(credentials, &format!("{prefix}{}", share::POSTER_FILENAME), "image/jpeg", poster)?;
        }
        check()?;
        self.put(credentials, &format!("{prefix}{}", share::MANIFEST_FILENAME), "application/json", &manifest)?;
        progress(1.0);
        Ok(Uploaded { link, filename: contents.media_filename })
    }

    /// Removes every object behind a share: media, poster, and manifest.
    pub fn delete_share(&self, credentials: &Credentials, slug: &str) -> Result<(), String> {
        if !credentials.is_configured() {
            return Err("R2 sharing is not configured. Add your credentials in Settings > Sharing.".into());
        }
        let prefix = share::object_prefix(slug);
        let mut keys =
            vec![format!("{prefix}{}", share::POSTER_FILENAME), format!("{prefix}{}", share::MANIFEST_FILENAME)];
        let manifest = r2::object_request(
            "GET",
            &format!("{prefix}{}", share::MANIFEST_FILENAME),
            "application/octet-stream",
            credentials,
            Utc::now(),
        );
        let (status, body) = self.send(&manifest, credentials, None)?;
        if status != 404 {
            if !(200..300).contains(&status) {
                return Err(r2::error_message(&body, status, credentials));
            }
            if let Ok(manifest) = serde_json::from_str::<share::ShareManifest>(&body) {
                let media = manifest.media;
                if media.is_empty() || media == "." || media == ".." || share::sanitized_filename(&media) != media {
                    return Err(
                        "The cloud manifest has an invalid media filename. Check this share in your R2 bucket.".into(),
                    );
                }
                keys.insert(0, format!("{prefix}{media}"));
            }
        }
        for key in keys {
            let request = r2::object_request("DELETE", &key, "application/octet-stream", credentials, Utc::now());
            let (status, body) = self.send(&request, credentials, None)?;
            if !(200..300).contains(&status) && status != 404 {
                return Err(r2::error_message(&body, status, credentials));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A one-thread fake bucket: PUT stores, GET returns, DELETE removes.
    type Objects = Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>;

    fn fake_bucket() -> (String, Objects) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let objects = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let store = objects.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let mut length = 0;
                let mut authorized = false;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let lower = header.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    authorized |= lower.starts_with("authorization: aws4-hmac-sha256 credential=akidfixture/");
                    if header == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut objects = store.lock().unwrap();
                let (status, reply) = if !authorized {
                    ("403 Forbidden", b"<Error><Code>AccessDenied</Code><Message>no</Message></Error>".to_vec())
                } else {
                    match method.as_str() {
                        "PUT" => {
                            objects.insert(path, body);
                            ("200 OK", Vec::new())
                        }
                        "GET" => match objects.get(&path) {
                            Some(data) => ("200 OK", data.clone()),
                            None => ("404 Not Found", b"<Error><Code>NoSuchKey</Code></Error>".to_vec()),
                        },
                        "DELETE" => {
                            objects.remove(&path);
                            ("204 No Content", Vec::new())
                        }
                        _ => ("405 Method Not Allowed", Vec::new()),
                    }
                };
                let mut stream = stream;
                let _ =
                    write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
                let _ = stream.write_all(&reply);
            }
        });
        (address, objects)
    }

    fn request(item_id: uuid::Uuid, file: &Path) -> ShareRequest<'_> {
        ShareRequest { item_id, file, title: None, app_version: "0.5.9" }
    }

    fn credentials() -> Credentials {
        Credentials {
            account_id: "acct".into(),
            bucket: "shots".into(),
            public_base_url: "https://share.example.com".into(),
            access_key_id: "AKIDFIXTURE".into(),
            secret_access_key: "fixture-secret".into(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn uploads_test_and_deletes_against_a_fake_bucket() {
        let (address, objects) = fake_bucket();
        let client = {
            let mut client = Client::new();
            client.agent = ureq::Agent::config_builder().http_status_as_error(false).proxy(None).build().into();
            client.endpoint = Some(address);
            client
        };
        let credentials = credentials();
        client.test_connection(&credentials).unwrap();
        let bad = Credentials { access_key_id: "WRONG".into(), ..credentials.clone() };
        let error = client.test_connection(&bad).unwrap_err();
        assert!(error.contains("AccessDenied") && !error.contains("fixture-secret"), "{error}");

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Shot.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([1, 2, 3, 255])).save(&file).unwrap();
        let id = uuid::Uuid::new_v4();
        let seen = Mutex::new(Vec::new());
        let uploaded = client
            .upload_share(&credentials, request(id, &file), &|p| seen.lock().unwrap().push(p), &AtomicBool::new(false))
            .unwrap();
        assert!(uploaded.link.starts_with("https://bettershot.site/s/"));
        let slug = share::slug(&id);
        {
            let objects = objects.lock().unwrap();
            let manifest: share::ShareManifest =
                serde_json::from_slice(&objects[&format!("/shots/s/{slug}/meta.json")]).unwrap();
            assert_eq!((manifest.width, manifest.media.as_str()), (Some(40), "Shot.png"));
            assert_eq!(objects[&format!("/shots/s/{slug}/Shot.png")], std::fs::read(&file).unwrap());
            assert!(objects.contains_key(&format!("/shots/s/{slug}/poster.jpg")));
        }
        assert_eq!(seen.lock().unwrap().last(), Some(&1.0));

        client.delete_share(&credentials, &slug).unwrap();
        assert!(objects.lock().unwrap().is_empty());

        let cancelled = client.upload_share(&credentials, request(id, &file), &|_| {}, &AtomicBool::new(true));
        assert_eq!(cancelled, Err(UploadError::Cancelled));
        let off = Credentials { enabled: false, ..credentials };
        assert!(
            matches!(client.upload_share(&off, request(id, &file), &|_| {}, &AtomicBool::new(false)), Err(UploadError::Failed(m)) if m.contains("Uploads are off"))
        );
    }
}
