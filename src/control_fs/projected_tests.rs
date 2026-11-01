use super::*;
use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt};

type Hook = Box<dyn FnOnce(&Path)>;
thread_local! { static AFTER_OPEN: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
pub(super) fn after_open(path: &Path) {
    let hook = AFTER_OPEN.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(path);
    }
}

#[test]
fn a_projection_replaced_after_open_is_refused_without_reading_the_replacement() {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = temp.path().join("projection");
    let replacement = temp.path().join("replacement");
    fs::write(&path, b"retained snapshot").unwrap();
    fs::write(&replacement, b"different pathname").unwrap();
    AFTER_OPEN.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |path| fs::rename(replacement, path).unwrap()))
    });
    assert!(read_trusted_projected_file(&path, 1024).is_err());
    assert_eq!(fs::read(path).unwrap(), b"different pathname");
}
