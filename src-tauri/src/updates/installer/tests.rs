use super::*;

#[test]
fn transaction_arguments_keep_spaces_target_last_and_reject_missing_or_overlong_paths() {
    let root = std::env::temp_dir().join(format!(
        "rela-installer-args-{}",
        crate::updates::ipc::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let target = root.join("target with spaces");
    std::fs::create_dir(&target).unwrap();
    let request = root.join("request with spaces.dat");
    std::fs::write(&request, b"fixture").unwrap();
    let args = transaction_args(&target, &request).unwrap();
    assert_eq!(args.len(), 2);
    assert!(args[0].starts_with("/RELAUPDATE=\"") && args[0].ends_with('"'));
    assert!(args[1].starts_with("/D=") && !args[1].contains('"'));
    assert!(args[1].ends_with("target with spaces"));
    assert!(transaction_args(&target, &root.join("missing")).is_err());
    let mut long = root.clone();
    for _ in 0..6 {
        long = long.join("x".repeat(90));
        std::fs::create_dir(&long).unwrap();
    }
    let long_request = long.join("request.dat");
    std::fs::write(&long_request, b"fixture").unwrap();
    assert!(transaction_args(&target, &long_request).is_err());
    let resolved = root.canonicalize().unwrap();
    assert_eq!(
        resolved.parent(),
        Some(std::env::temp_dir().canonicalize().unwrap().as_path())
    );
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("rela-installer-args-"));
    std::fs::remove_dir_all(resolved).unwrap();
}

#[test]
fn local_bridge_rejects_unexpected_path_and_oversized_request() {
    tauri::async_runtime::block_on(async {
        for request in [
            b"GET /wrong HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec(),
            vec![b'x'; MAX_REQUEST + 1],
        ] {
            let bridge = Bridge::bind(b"trusted-response".to_vec()).await.unwrap();
            let addr = bridge.listener.local_addr().unwrap();
            let server = tauri::async_runtime::spawn(async move { bridge.serve().await });
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream.write_all(&request).await.unwrap();
            assert!(server.await.unwrap().is_err());
            let mut output = Vec::new();
            let _ = stream.read_to_end(&mut output).await;
            assert!(!output.windows(16).any(|chunk| chunk == b"trusted-response"));
        }
    });
}

#[test]
fn local_bridge_times_out_without_a_complete_request() {
    tauri::async_runtime::block_on(async {
        let bridge = Bridge::bind(b"trusted-response".to_vec()).await.unwrap();
        let addr = bridge.listener.local_addr().unwrap();
        let server = tauri::async_runtime::spawn(async move { bridge.serve().await });
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream.write_all(b"GET /").await.unwrap();
        assert!(server.await.unwrap().is_err());
        let mut output = Vec::new();
        let _ = stream.read_to_end(&mut output).await;
        assert!(output.is_empty());
    });
}
