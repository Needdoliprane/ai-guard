#![cfg(unix)]

use agentsandbox::{
    broker::local_ipc::{EndpointRecordV1, IpcError, PrivateListener, read_frame, write_frame},
    control_fs::Directory,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

fn private_root() -> (tempfile::TempDir, Directory) {
    #[cfg(target_os = "macos")]
    let temporary = tempfile::tempdir_in("/private/tmp").unwrap();
    #[cfg(not(target_os = "macos"))]
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temporary.path().canonicalize().unwrap();
    let root = Directory::open_or_create(&path).unwrap();
    (temporary, root)
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Message {
    value: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct LenientMessage {
    value: String,
}

#[tokio::test]
async fn checked_listener_and_connector_exchange_bounded_strict_frames() {
    let (_temporary, root) = private_root();
    let listener = PrivateListener::bind_new(&root).unwrap();
    let client = listener.endpoint().connect_checked(&root).await.unwrap();
    let server = listener.accept_checked().await.unwrap();
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut server_read, mut server_write) = tokio::io::split(server);
    let timeout = Duration::from_secs(1);
    let sender = tokio::spawn(async move {
        write_frame(
            &mut client_write,
            &Message {
                value: "hello".into(),
            },
            1024,
            timeout,
        )
        .await
    });
    let received: Message = read_frame(&mut server_read, 1024, timeout).await.unwrap();
    sender.await.unwrap().unwrap();
    assert_eq!(received.value, "hello");
    write_frame(&mut server_write, &received, 1024, timeout)
        .await
        .unwrap();
    let echoed: Message = read_frame(&mut client_read, 1024, timeout).await.unwrap();
    assert_eq!(echoed, received);
    let debug = format!("{:?}", listener.endpoint());
    assert!(!debug.contains(root.path().to_string_lossy().as_ref()));
}

#[tokio::test]
async fn frame_reader_rejects_unknown_fields_oversize_and_total_timeout() {
    use tokio::net::UnixStream;

    let (_temporary, root) = private_root();
    let (mut peer, mut reader) = UnixStream::pair().unwrap();
    let body = br#"{"value":"x","extra":null}"#;
    let parsed = serde_json::from_slice::<LenientMessage>(body).unwrap();
    assert_eq!(parsed.value, "x");
    peer.write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    peer.write_all(body).await.unwrap();
    assert_eq!(
        read_frame::<LenientMessage>(&mut reader, 1024, Duration::from_secs(1)).await,
        Err(IpcError::InvalidInput)
    );

    let (mut peer, mut reader) = UnixStream::pair().unwrap();
    peer.write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    peer.write_all(body).await.unwrap();
    let error = read_frame::<Message>(&mut reader, 1024, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains(root.path().to_string_lossy().as_ref()));

    // The announced size is rejected from the prefix, without waiting for a body.
    let (mut peer, mut reader) = UnixStream::pair().unwrap();
    peer.write_all(&1025_u32.to_be_bytes()).await.unwrap();
    let started = Instant::now();
    assert_eq!(
        read_frame::<Message>(&mut reader, 1024, Duration::from_secs(1)).await,
        Err(IpcError::TooLarge)
    );
    assert!(started.elapsed() < Duration::from_millis(500));

    // Prefix and body share one total deadline; two sub-deadline delays must time out.
    let (mut peer, mut reader) = UnixStream::pair().unwrap();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(85)).await;
        peer.write_all(&4_u32.to_be_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(85)).await;
        let _ = peer.write_all(b"null").await;
    });
    let started = Instant::now();
    let error = read_frame::<serde_json::Value>(&mut reader, 1024, Duration::from_millis(140))
        .await
        .unwrap_err();
    assert_eq!(error, IpcError::Timeout);
    assert!(started.elapsed() < Duration::from_millis(500));
    writer.await.unwrap();

    let (mut peer, mut reader) = UnixStream::pair().unwrap();
    peer.write_all(&8_u32.to_be_bytes()).await.unwrap();
    peer.write_all(b"short").await.unwrap();
    peer.shutdown().await.unwrap();
    assert!(
        read_frame::<Message>(&mut reader, 1024, Duration::from_secs(1))
            .await
            .is_err()
    );

    // A zero cap is invalid before any allocation or socket read.
    let (mut _peer, mut reader) = UnixStream::pair().unwrap();
    assert_eq!(
        read_frame::<Message>(&mut reader, 0, Duration::from_secs(1)).await,
        Err(IpcError::InvalidInput)
    );
    assert_eq!(
        read_frame::<Message>(&mut reader, 512 * 1024 + 1, Duration::from_secs(1)).await,
        Err(IpcError::InvalidInput)
    );

    let (mut writer, peer) = UnixStream::pair().unwrap();
    let too_large = Message {
        value: "x".repeat(2048),
    };
    assert_eq!(
        write_frame(&mut writer, &too_large, 1024, Duration::from_secs(1)).await,
        Err(IpcError::TooLarge)
    );
    let mut first_byte = [0_u8; 1];
    assert!(peer.try_read(&mut first_byte).is_err());

    let err = IpcError::Timeout;
    assert_eq!(err, err);
    assert_eq!(format!("{err:?}"), "Timeout");
}

#[tokio::test]
async fn listener_generations_never_unlink_prior_socket_and_tampered_record_is_refused() {
    let (_temporary, root) = private_root();
    let first = PrivateListener::bind_new(&root).unwrap();
    let first_dirname = first.endpoint().directory_name().to_owned();
    let first_socket = root.path().join(&first_dirname).join("s");
    let second = PrivateListener::bind_new(&root).unwrap();
    assert_ne!(
        first.endpoint().directory_name(),
        second.endpoint().directory_name()
    );
    assert!(
        fs::symlink_metadata(&first_socket)
            .unwrap()
            .file_type()
            .is_socket()
    );

    // Altering the serialized public record cannot redirect a connection.
    let mut altered = serde_json::to_value(first.endpoint()).unwrap();
    altered["directory_inode"] =
        serde_json::Value::from(altered["directory_inode"].as_u64().unwrap() + 1);
    let altered: EndpointRecordV1 = serde_json::from_value(altered).unwrap();
    assert!(altered.connect_checked(&root).await.is_err());

    // A pathname symlink is rejected, then the original socket entry is restored.
    let saved_socket = root.path().join(&first_dirname).join("s.saved");
    fs::rename(&first_socket, &saved_socket).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&saved_socket, &first_socket).unwrap();
    assert!(first.endpoint().connect_checked(&root).await.is_err());
    fs::remove_file(&first_socket).unwrap();
    fs::rename(&saved_socket, &first_socket).unwrap();

    // Wrong socket and parent modes are refused without silently repairing them.
    fs::set_permissions(&first_socket, fs::Permissions::from_mode(0o620)).unwrap();
    assert!(first.endpoint().connect_checked(&root).await.is_err());
    assert_eq!(
        fs::symlink_metadata(&first_socket)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o620
    );
    fs::set_permissions(&first_socket, fs::Permissions::from_mode(0o600)).unwrap();

    let first_directory = root.path().join(&first_dirname);
    fs::set_permissions(&first_directory, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(first.endpoint().connect_checked(&root).await.is_err());
    assert_eq!(
        fs::metadata(&first_directory).unwrap().permissions().mode() & 0o777,
        0o750
    );
    fs::set_permissions(&first_directory, fs::Permissions::from_mode(0o700)).unwrap();

    let directory_backup = root.path().join(format!("{first_dirname}.saved"));
    fs::rename(&first_directory, &directory_backup).unwrap();
    std::os::unix::fs::symlink(&directory_backup, &first_directory).unwrap();
    assert!(first.endpoint().connect_checked(&root).await.is_err());
    fs::remove_file(&first_directory).unwrap();
    fs::rename(&directory_backup, &first_directory).unwrap();

    // The original generation remains usable after the rejected substitutions.
    let _client = first.endpoint().connect_checked(&root).await.unwrap();
    let _accepted = first.accept_checked().await.unwrap();

    // Strict deserialization rejects even nullable unknown fields and endpoint data stays opaque.
    let mut record = serde_json::to_value(first.endpoint()).unwrap();
    record["unexpected"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<EndpointRecordV1>(record).is_err());
    let debug = format!("{:?}", first.endpoint());
    assert!(!debug.contains(root.path().to_string_lossy().as_ref()));

    // Filesystem identity metadata is part of the record, not a path chosen by the caller.
    let serialized = serde_json::to_value(first.endpoint()).unwrap();
    assert_eq!(serialized["version"], 1);
    assert_eq!(
        serialized["owner_uid"].as_u64(),
        Some(unsafe { libc::geteuid() as u64 })
    );
    assert!(serialized["directory_device"].as_u64().is_some());
    assert!(serialized["directory_inode"].as_u64().is_some());
    assert!(serialized["socket_device"].as_u64().is_some());
    assert!(serialized["socket_inode"].as_u64().is_some());
    let socket_meta = fs::symlink_metadata(&first_socket).unwrap();
    assert_eq!(serialized["socket_inode"].as_u64(), Some(socket_meta.ino()));
}
