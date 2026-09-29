use super::*;
use std::{
    fs,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum TestMessage {
    Ready { version: String },
    Commit,
}

#[test]
fn authenticated_channels_exchange_only_valid_bounded_messages() {
    let listener = Listener::bind(&random_id().unwrap(), Role::Core).unwrap();
    let endpoint = listener.endpoint();
    let client = thread::spawn(move || {
        let mut channel = Channel::connect(&endpoint).unwrap();
        channel
            .send(&TestMessage::Ready {
                version: "0.2.0".into(),
            })
            .unwrap();
        assert_eq!(
            channel.receive::<TestMessage>().unwrap(),
            TestMessage::Commit
        );
    });
    let mut channel = listener
        .accept(
            std::process::id(),
            &std::env::current_exe().unwrap(),
            Duration::from_secs(5),
            || false,
        )
        .unwrap();
    assert_eq!(
        channel.receive::<TestMessage>().unwrap(),
        TestMessage::Ready {
            version: "0.2.0".into()
        }
    );
    channel.send(&TestMessage::Commit).unwrap();
    client.join().unwrap();
}

#[test]
fn wrong_token_role_transaction_or_process_is_rejected_before_command_dispatch() {
    for case in ["token", "role", "transaction", "process"] {
        let listener = Listener::bind(&random_id().unwrap(), Role::Core).unwrap();
        let mut endpoint = listener.endpoint();
        let mut pid = std::process::id();
        match case {
            "token" => endpoint.token = random_id().unwrap(),
            "role" => endpoint.role = Role::Candidate,
            "transaction" => endpoint.id = random_id().unwrap(),
            _ => pid += 1,
        }
        let client = thread::spawn(move || assert!(Channel::connect(&endpoint).is_err()));
        assert!(listener
            .accept(
                pid,
                &std::env::current_exe().unwrap(),
                Duration::from_millis(350),
                || false
            )
            .is_err());
        client.join().unwrap();
    }
}

#[test]
fn oversized_truncated_and_unknown_frames_are_rejected() {
    for case in ["oversized", "truncated", "unknown"] {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            match case {
                "oversized" => stream
                    .write_all(&((MAX_FRAME + 1) as u32).to_le_bytes())
                    .unwrap(),
                "truncated" => {
                    stream.write_all(&64u32.to_le_bytes()).unwrap();
                    stream.write_all(b"short").unwrap();
                }
                _ => {
                    let bytes = br#"{"state":"run_arbitrary_command","command":"unexpected"}"#;
                    stream
                        .write_all(&(bytes.len() as u32).to_le_bytes())
                        .unwrap();
                    stream.write_all(bytes).unwrap();
                }
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut channel = Channel::new(stream, Duration::from_secs(1)).unwrap();
        assert!(channel.receive::<TestMessage>().is_err());
        client.join().unwrap();
    }
}

#[test]
#[ignore = "spawned only by the authenticated child-process test"]
fn fixture_child() {
    let path = std::env::var_os("RELA_TEST_IPC_REQUEST").unwrap();
    let endpoint: Endpoint = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let mut channel = Channel::connect(&endpoint).unwrap();
    channel
        .send(&TestMessage::Ready {
            version: "0.2.0".into(),
        })
        .unwrap();
    assert_eq!(
        channel.receive::<TestMessage>().unwrap(),
        TestMessage::Commit
    );
}

#[test]
fn actual_child_process_authenticates_and_completes_the_handshake() {
    let id = random_id().unwrap();
    let listener = Listener::bind(&id, Role::Candidate).unwrap();
    let path = std::env::temp_dir().join(format!("rela-ipc-test-{id}.json"));
    fs::write(&path, serde_json::to_vec(&listener.endpoint()).unwrap()).unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut child = Command::new(&executable)
        .args(["--exact", "updates::ipc::tests::fixture_child", "--ignored"])
        .env("RELA_TEST_IPC_REQUEST", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let mut channel = listener
        .accept(child.id(), &executable, Duration::from_secs(5), || {
            child.try_wait().unwrap().is_some()
        })
        .unwrap();
    assert_eq!(
        channel.receive::<TestMessage>().unwrap(),
        TestMessage::Ready {
            version: "0.2.0".into()
        }
    );
    channel.send(&TestMessage::Commit).unwrap();
    assert!(child.wait().unwrap().success());
    fs::remove_file(path).unwrap();
}
