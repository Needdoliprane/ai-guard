use super::{
    CancelGeneration,
    batch_io::OwnedBatchIo,
    bootstrap_stdin::{
        BootstrapStdinSubmitError, MAX_BOOTSTRAP_STDIN_BYTES, PrearmedBootstrapStdin,
    },
};
use std::{process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    time::{Instant, timeout},
};

fn inert_child() -> Child {
    Command::new("/bin/cat")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn exclusive_bootstrap_stdin_releases_or_closes_the_exact_pipe_once() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Case {
        Submit,
        Stop,
        Veto,
        Oversize,
        Abandon,
    }

    let frame = b"local inert fixture\n";
    for case in [
        Case::Submit,
        Case::Stop,
        Case::Veto,
        Case::Oversize,
        Case::Abandon,
    ] {
        let mut child = inert_child();
        let stdin = PrearmedBootstrapStdin::take(&mut child).unwrap();
        assert!(PrearmedBootstrapStdin::take(&mut child).is_err());
        let mut io = OwnedBatchIo::own(child, None, 256);
        stdin
            .wait_writable(Instant::now() + Duration::from_secs(5))
            .await
            .unwrap();
        let generation = CancelGeneration::new();
        let expected = generation.current();
        if case == Case::Stop {
            generation.invalidate();
        }
        if case == Case::Abandon {
            drop(stdin);
        } else {
            let oversize = [0; MAX_BOOTSTRAP_STDIN_BYTES + 1];
            let bytes = if case == Case::Oversize {
                oversize.as_slice()
            } else {
                frame.as_slice()
            };
            let result = stdin.submit_once(bytes, &generation, expected, || {
                assert!(matches!(case, Case::Submit | Case::Veto));
                anyhow::ensure!(case != Case::Veto, "fixture release veto");
                Ok(())
            });
            assert_eq!(
                result,
                if case == Case::Submit {
                    Ok(())
                } else {
                    Err(BootstrapStdinSubmitError::Refused)
                },
                "{case:?}"
            );
        }
        let result = timeout(Duration::from_secs(5), io.collect())
            .await
            .expect("dropping the only stdin owner must deliver EOF")
            .unwrap();
        assert!(result.status.unwrap().success(), "{result:?}");
        assert!(result.errors.is_empty(), "{result:?}");
        assert!(result.stdout_eof);
        assert_eq!(
            result.stdout.bytes.as_slice(),
            if case == Case::Submit {
                frame.as_slice()
            } else {
                &[]
            },
            "{case:?}"
        );
    }

    let mut child = inert_child();
    let stdin = PrearmedBootstrapStdin::take(&mut child).unwrap();
    stdin
        .wait_writable(Instant::now() + Duration::from_secs(5))
        .await
        .unwrap();
    timeout(Duration::from_secs(5), child.kill())
        .await
        .unwrap()
        .unwrap();
    let generation = CancelGeneration::new();
    assert_eq!(
        stdin.submit_once(frame, &generation, generation.current(), || Ok(())),
        Err(BootstrapStdinSubmitError::DeliveryUnknown)
    );
}
