use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use super::{Error, Store};

#[test]
fn metadata_returns_while_the_index_is_locked() {
    let mut store = Store::new(Vec::new(), None).unwrap();
    store.append(vec![vec![]]).unwrap();
    store.complete().unwrap();
    let reader = store.reader();
    let (sent, received) = mpsc::channel();
    std::thread::scope(|scope| {
        let locked = store.shared.lock().unwrap();
        scope.spawn(|| {
            sent.send((store.row_count(), reader.row_count(), reader.is_complete()))
                .unwrap();
        });
        let observed = received.recv_timeout(Duration::from_secs(5));
        drop(locked);
        let (store_count, reader_count, complete) = observed.expect("metadata waited for the lock");
        assert_eq!(store_count.unwrap(), 1);
        assert_eq!(reader_count.unwrap(), 1);
        assert!(complete.unwrap());
    });
}

#[test]
fn metadata_reports_a_poisoned_index_without_locking() {
    let store = Store::new(Vec::new(), None).unwrap();
    let reader = store.reader();
    std::thread::scope(|scope| {
        assert!(
            scope
                .spawn(|| {
                    let _locked = store.shared.lock().unwrap();
                    panic!("interrupt the index operation");
                })
                .join()
                .is_err()
        );
    });
    assert!(matches!(store.row_count(), Err(Error::Poisoned)));
    assert!(matches!(reader.row_count(), Err(Error::Poisoned)));
    assert!(matches!(reader.is_complete(), Err(Error::Poisoned)));
}

#[test]
fn close_defers_cleanup_only_until_the_active_file_reference_is_released() {
    let folder = tempfile::tempdir().unwrap();
    let mut store = Store::new(Vec::new(), Some(folder.path())).unwrap();
    let reader = store.reader();
    let active_file = Arc::clone(store.shared.lock().unwrap().file.as_ref().unwrap());
    store.close().unwrap();
    assert!(matches!(reader.rows(0..1), Err(Error::Closed)));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    drop(active_file);
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[test]
fn drop_defers_cleanup_only_until_the_active_file_reference_is_released() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::new(Vec::new(), Some(folder.path())).unwrap();
    let reader = store.reader();
    let active_file = Arc::clone(store.shared.lock().unwrap().file.as_ref().unwrap());
    drop(store);
    assert!(matches!(reader.rows(0..1), Err(Error::Closed)));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    drop(active_file);
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}
