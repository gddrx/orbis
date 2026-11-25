//! Fetching the stream to a file, in ranges: YouTube serves a whole file in
//! one request slowly, a range of 10 MiB at full speed (yt-dlp does the
//! same).

use std::path::Path;
use std::time::Duration;

use stream::Stream;
use tokio::io::AsyncWriteExt;

use crate::DownloadError;

/// The size of one range request.
pub(crate) const CHUNK: u64 = 10 * 1024 * 1024;
/// Tries of one range before the download fails.
const TRIES: u32 = 3;

/// The total size from a `Content-Range: bytes 0-99/1234` header.
fn total_of(content_range: &str) -> Option<u64> {
    content_range.rsplit('/').next()?.trim().parse().ok()
}

/// Fetches `stream` into `dest` in ranges of `chunk` bytes; `progress` gets
/// the bytes so far and the total, once known.
pub(crate) async fn fetch(
    http: &reqwest::Client,
    stream: &Stream,
    dest: &Path,
    chunk: u64,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<(), DownloadError> {
    let mut file = tokio::fs::File::create(dest).await?;
    let mut done: u64 = 0;
    let mut total: Option<u64> = None;
    loop {
        let end = done + chunk - 1;
        let mut tries = 0;
        let response = loop {
            tries += 1;
            let mut request = http
                .get(&stream.url)
                .header(reqwest::header::RANGE, format!("bytes={done}-{end}"));
            for (name, value) in &stream.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.send().await.and_then(|r| r.error_for_status()) {
                Ok(response) => break response,
                // The URL carries signatures: errors never print it.
                Err(e) if tries < TRIES => {
                    let _ = e;
                    tokio::time::sleep(Duration::from_secs(u64::from(tries))).await;
                }
                Err(e) => {
                    let status = e.status().map(|s| s.as_u16().to_string());
                    return Err(DownloadError::Http(status.unwrap_or_else(|| {
                        if e.is_timeout() {
                            "timed out".to_owned()
                        } else {
                            "connection failed".to_owned()
                        }
                    })));
                }
            }
        };
        let whole = response.status() == reqwest::StatusCode::OK;
        if total.is_none() {
            total = if whole {
                response.content_length()
            } else {
                response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(total_of)
            };
        }
        let mut response = response;
        let before = done;
        while let Some(bytes) = response
            .chunk()
            .await
            .map_err(|_| DownloadError::Http("the stream broke off".to_owned()))?
        {
            file.write_all(&bytes).await?;
            done += bytes.len() as u64;
            progress(done, total);
        }
        // A server that ignores ranges sends everything at once; without a
        // total, a short range is the last.
        let got = done - before;
        if whole || got == 0 || total.map_or(got < chunk, |t| done >= t) {
            break;
        }
    }
    file.flush().await?;
    if let Some(total) = total
        && done < total
    {
        return Err(DownloadError::Http(format!(
            "the stream ended at {done} of {total} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A server of `body` that answers ranges with 206, as googlevideo
    /// does; counts the requests.
    pub(crate) async fn range_server(body: Vec<u8>) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        let body = Arc::new(body);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = Arc::clone(&body);
                let counter = Arc::clone(&counter);
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
                    counter.fetch_add(1, Ordering::SeqCst);
                    let range = head
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim().split_once('-'))
                        .and_then(|(a, b)| {
                            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?))
                        });
                    let response = match range {
                        Some((start, end)) => {
                            let end = end.min(body.len() - 1);
                            let part = &body[start..=end];
                            let mut out = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len(),
                                part.len()
                            )
                            .into_bytes();
                            out.extend_from_slice(part);
                            out
                        }
                        None => {
                            let mut out = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            out.extend_from_slice(&body);
                            out
                        }
                    };
                    let _ = socket.write_all(&response).await;
                });
            }
        });
        (address, requests)
    }

    pub(crate) fn stream_at(url: String, codec: &str) -> Stream {
        Stream {
            video_id: "v".into(),
            url,
            format_id: "251".into(),
            codec: codec.into(),
            container: "webm".into(),
            bitrate_kbps: None,
            sample_rate: None,
            duration: None,
            expires_at: None,
            headers: vec![("User-Agent".into(), "orbis-test".into())],
        }
    }

    #[test]
    fn totals_come_from_content_range() {
        assert_eq!(total_of("bytes 0-99/1234"), Some(1234));
        assert_eq!(total_of("bytes 0-99/*"), None);
    }

    #[tokio::test]
    async fn the_stream_comes_in_ranges_and_whole() {
        let body: Vec<u8> = (0..25_000u32).map(|i| (i % 251) as u8).collect();
        let (address, requests) = range_server(body.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("part");
        let mut seen = Vec::new();
        fetch(
            &reqwest::Client::new(),
            &stream_at(format!("http://{address}/x"), "opus"),
            &dest,
            10_000,
            &mut |done, total| seen.push((done, total)),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(requests.load(Ordering::SeqCst), 3, "three ranges of 10 000");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(seen.last(), Some(&(25_000, Some(25_000))));
    }

    #[tokio::test]
    async fn a_dead_server_fails_without_the_url() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing listens on the port of a dropped listener.
        let address = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let error = fetch(
            &reqwest::Client::new(),
            &stream_at(format!("http://{address}/secret-signature"), "opus"),
            &dir.path().join("part"),
            CHUNK,
            &mut |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("secret-signature"), "{error}");
    }
}
