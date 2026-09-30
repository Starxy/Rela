//! Bounded anonymous HTTPS reads. Remote response bodies/URLs never enter errors.
use rela_protocol::AppError;
use reqwest::{header, redirect::Policy, Client, StatusCode};
use std::time::Duration;
use url::Url;

pub struct Fetcher {
    pub(super) client: Client,
    pub(super) allow_loopback: bool,
}
pub enum Response {
    NotModified,
    Content {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

impl Fetcher {
    pub fn new() -> Result<Self, AppError> {
        Self::build(false)
    }

    fn build(allow_loopback: bool) -> Result<Self, AppError> {
        Self::with_timeout(allow_loopback, Duration::from_secs(20))
    }

    fn with_timeout(allow_loopback: bool, timeout: Duration) -> Result<Self, AppError> {
        let client = Client::builder()
            .user_agent(concat!("Rela/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .https_only(!allow_loopback)
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= 4 {
                    return attempt.error("redirect limit");
                }
                if allowed_url(attempt.url(), allow_loopback) {
                    attempt.follow()
                } else {
                    attempt.error("untrusted redirect")
                }
            }))
            .build()
            .map_err(|_| fetch_error("无法准备线上请求。"))?;
        Ok(Self {
            client,
            allow_loopback,
        })
    }

    #[cfg(test)]
    pub(super) fn local_for_test() -> Self {
        Self::build(true).expect("test HTTP client")
    }

    pub async fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        limit: usize,
    ) -> Result<Response, AppError> {
        self.fetch_inner(url, etag, limit, self.allow_loopback)
            .await
    }

    async fn fetch_inner(
        &self,
        url: &str,
        etag: Option<&str>,
        limit: usize,
        allow_loopback: bool,
    ) -> Result<Response, AppError> {
        let url = Url::parse(url).map_err(|_| fetch_error("线上入口无效。"))?;
        if !allowed_url(&url, allow_loopback) {
            return Err(fetch_error("线上入口不受支持。"));
        }
        for attempt in 0..2 {
            let result = self.fetch_once(url.clone(), etag, limit).await;
            match result {
                Err(error) if error.code == "download_retryable" && attempt == 0 => {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                }
                other => return other,
            }
        }
        Err(fetch_error("暂时无法获取线上配置，请稍后重试。"))
    }

    async fn fetch_once(
        &self,
        url: Url,
        etag: Option<&str>,
        limit: usize,
    ) -> Result<Response, AppError> {
        let mut request = self
            .client
            .get(url)
            .header(header::ACCEPT, "application/json");
        if let Some(etag) = etag.filter(|value| value.len() <= 256) {
            if let Ok(value) = header::HeaderValue::from_str(etag) {
                request = request.header(header::IF_NONE_MATCH, value);
            }
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| AppError::new("download_retryable", "网络请求失败，请检查网络后重试。"))?;
        if response.status() == StatusCode::NOT_MODIFIED {
            if etag.is_none() {
                return Err(fetch_error("服务器返回了无缓存可用的响应。"));
            }
            return Ok(Response::NotModified);
        }
        if response.status().is_server_error() || response.status() == StatusCode::TOO_MANY_REQUESTS
        {
            return Err(AppError::new(
                "download_retryable",
                "线上服务暂时不可用，请稍后重试。",
            ));
        }
        if response.status() != StatusCode::OK {
            return Err(fetch_error(if response.status() == StatusCode::NOT_FOUND {
                "线上清单尚未发布。"
            } else {
                "线上请求未成功。"
            }));
        }
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(fetch_error("线上内容超过大小限制。"));
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 256)
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fetch_error("下载中断，请重试。"))?
        {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(fetch_error("线上内容超过大小限制。"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Response::Content { bytes, etag })
    }
}

pub(super) fn allowed_url(url: &Url, allow_loopback: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    if allow_loopback && url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
        return true;
    }
    url.scheme() == "https"
        && url.port().is_none()
        && matches!(url.host_str(), Some("raw.githubusercontent.com"))
}
fn fetch_error(message: &str) -> AppError {
    AppError::new("download_failed", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve(response: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/manifest", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut input = [0; 4096];
            let count = stream.read(&mut input).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&input[..count]).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn conditional_reads_and_size_limits_use_real_http() {
        tauri::async_runtime::block_on(async {
            let fetcher = Fetcher::build(true).unwrap();
            let (url, server) = serve("HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n");
            assert!(matches!(
                fetcher
                    .fetch_inner(&url, Some("\"v1\""), 16, true)
                    .await
                    .unwrap(),
                Response::NotModified
            ));
            let request = server.join().unwrap().to_lowercase();
            assert!(request.contains("if-none-match: \"v1\""));
            assert!(!request.contains("authorization:"));
            let (url, server) =
                serve("HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n");
            assert!(fetcher.fetch_inner(&url, None, 16, true).await.is_err());
            server.join().unwrap();
            let (url, server) = serve("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n14\r\n01234567890123456789\r\n0\r\n\r\n");
            assert!(fetcher.fetch_inner(&url, None, 16, true).await.is_err());
            server.join().unwrap();
            let (url, server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n{}");
            let Response::Content { bytes, etag } =
                fetcher.fetch_inner(&url, None, 16, true).await.unwrap()
            else {
                panic!("expected content")
            };
            assert_eq!(bytes, b"{}");
            assert_eq!(etag.as_deref(), Some("\"v1\""));
            server.join().unwrap();
        });
    }

    #[test]
    fn manifest_transport_disallows_cleartext_and_foreign_hosts() {
        for url in [
            "http://github.com/Starxy/Rela",
            "https://github.com.evil.test/a",
            "https://user:password@github.com/a",
            "file:///C:/test",
        ] {
            assert!(!allowed_url(&Url::parse(url).unwrap(), false));
        }
        assert!(allowed_url(
            &Url::parse(
                "https://raw.githubusercontent.com/Starxy/Rela/refs/heads/main/resources.json"
            )
            .unwrap(),
            false
        ));
    }

    #[test]
    fn retry_timeout_and_untrusted_redirect_use_real_http() {
        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/retry", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                for status in ["503 Service Unavailable", "200 OK"] {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut input = [0; 4096];
                    assert!(stream.read(&mut input).unwrap() > 0);
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                    )
                    .unwrap();
                }
            });
            let fetcher = Fetcher::build(true).unwrap();
            assert!(matches!(
                fetcher.fetch_inner(&url, None, 16, true).await.unwrap(),
                Response::Content { .. }
            ));
            server.join().unwrap();

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/slow", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                for _ in 0..2 {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut input = [0; 4096];
                    assert!(stream.read(&mut input).unwrap() > 0);
                    thread::sleep(Duration::from_millis(160));
                }
            });
            let fast = Fetcher::with_timeout(true, Duration::from_millis(60)).unwrap();
            let started = std::time::Instant::now();
            assert!(fast.fetch_inner(&url, None, 16, true).await.is_err());
            assert!(started.elapsed() < Duration::from_secs(3));
            server.join().unwrap();

            let (url, server) = serve("HTTP/1.1 302 Found\r\nLocation: http://untrusted.invalid/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let error = fetcher
                .fetch_once(Url::parse(&url).unwrap(), None, 16)
                .await
                .err()
                .unwrap();
            assert!(!error.message.contains("secret"));
            server.join().unwrap();
        });
    }
}
