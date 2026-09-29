use super::*;
use std::{io::Cursor, net::TcpListener, thread};

pub(crate) fn signed(bytes: &[u8]) -> (Package, String) {
    let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let signature = minisign::sign(
        Some(&keys.pk),
        &keys.sk,
        Cursor::new(bytes),
        Some("version:0.2.0"),
        None,
    )
    .unwrap();
    (
        Package {
            url: "https://github.com/Starxy/Rela/releases/download/v0.2.0/Rela.zip".into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            signature: STANDARD.encode(signature.to_string()),
        },
        STANDARD.encode(keys.pk.to_box().unwrap().to_string()),
    )
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rela-package-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for item in fs::read_dir(&self.0).unwrap() {
            fs::remove_file(item.unwrap().path()).unwrap();
        }
        fs::remove_dir(&self.0).unwrap();
    }
}
fn serve(bytes: Vec<u8>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/artifact", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut input = [0; 4096];
        let n = stream.read(&mut input).unwrap();
        let _ = stream.write_all(&bytes);
        String::from_utf8(input[..n].to_vec()).unwrap()
    });
    (url, handle)
}
fn response(bytes: &[u8]) -> Vec<u8> {
    let mut result = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    result.extend_from_slice(bytes);
    result
}

#[test]
fn real_signed_download_reports_progress_and_preserves_verified_handle() {
    tauri::async_runtime::block_on(async {
        let bytes = vec![42u8; 160_000];
        let (mut package, key) = signed(&bytes);
        let (url, server) = serve(response(&bytes));
        package.url = url;
        let fixture = Fixture::new();
        let mut progress = Vec::new();
        let mut verified = Fetcher::local_for_test()
            .download_package(&package, &key, &fixture.0, |n, total| {
                assert_eq!(total, bytes.len() as u64);
                progress.push(n);
            })
            .await
            .unwrap();
        let mut actual = Vec::new();
        verified.reader().unwrap().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(progress.first(), Some(&0));
        assert_eq!(progress.last(), Some(&package.size));
        assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
        let path = verified.path().to_owned();
        #[cfg(windows)]
        {
            assert!(OpenOptions::new().write(true).open(&path).is_err());
            assert!(fs::remove_file(&path).is_err());
        }
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(!request.contains("authorization:"));
        drop(verified);
        assert!(!path.exists());
    });
}

#[test]
fn corruption_wrong_key_size_and_signature_metadata_are_rejected_on_reopen() {
    let bytes = b"signed application bytes";
    let (package, key) = signed(bytes);
    let fixture = Fixture::new();
    let path = fixture.0.join("package.zip");
    fs::write(&path, bytes).unwrap();
    drop(VerifiedPackage::open(&path, &package, &key).unwrap());
    for case in 0..4 {
        let mut bad = package.clone();
        let mut wrong_key = key.clone();
        match case {
            0 => bad
                .sha256
                .replace_range(..1, if &bad.sha256[..1] == "a" { "b" } else { "a" }),
            1 => bad.size += 1,
            2 => wrong_key = signed(b"other").1,
            _ => {
                let text = String::from_utf8(STANDARD.decode(&bad.signature).unwrap()).unwrap();
                bad.signature = STANDARD.encode(text.replace("version:0.2.0", "version:9.9.9"));
            }
        }
        assert!(VerifiedPackage::open(&path, &bad, &wrong_key).is_err());
    }
    fs::write(&path, b"tampered application bytes").unwrap();
    assert!(VerifiedPackage::open(&path, &package, &key).is_err());
}

#[test]
fn partial_oversized_wrong_status_and_tampered_downloads_leave_no_candidate() {
    tauri::async_runtime::block_on(async {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\na".to_vec(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n".to_vec(),
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd".to_vec(),
            response(b"fake"),
            b"HTTP/1.1 302 Found\r\nLocation: http://untrusted.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ] {
            let (mut package, key) = signed(b"abcd"); let (url, server) = serve(response); package.url = url;
            let fixture = Fixture::new();
            assert!(Fetcher::local_for_test().download_package(&package, &key, &fixture.0, |_, _| {}).await.is_err());
            server.join().unwrap();
            assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
        }
    });
}
