//! kqueue vnode watches on real files and folders (macOS only).
#![cfg(target_os = "macos")]

use dispatch2::{DispatchQueue, DispatchQueueAttr};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use wgbar::watch::{file_id, Watch};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wgbar-watch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn watch(path: &std::path::Path) -> (Watch, Receiver<()>) {
    let queue = DispatchQueue::new("org.wgbar.test.watch", DispatchQueueAttr::SERIAL);
    let (tx, rx) = channel();
    let w = Watch::new(path.to_str().unwrap(), &queue, move || {
        let _ = tx.send(());
    })
    .expect("watch opens");
    (w, rx)
}

fn fired(rx: &Receiver<()>) -> bool {
    let got = rx.recv_timeout(Duration::from_secs(2)).is_ok();
    while rx.try_recv().is_ok() {} // coalesced extras
    got
}

#[test]
fn folder_reports_files_added_and_removed() {
    let dir = temp_dir("dir");
    let (_w, rx) = watch(&dir);
    std::fs::write(dir.join("t.name"), "utun9").unwrap();
    assert!(fired(&rx), "file created");
    std::fs::remove_file(dir.join("t.name")).unwrap();
    assert!(fired(&rx), "file removed");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn file_reports_in_place_writes_and_replacement() {
    let dir = temp_dir("file");
    let file = dir.join("preferences.plist");
    std::fs::write(&file, "a").unwrap();
    let path = file.to_str().unwrap().to_string();
    let (w, rx) = watch(&file);
    assert!(w.is_current(&path));
    std::fs::write(&file, "bb").unwrap();
    assert!(fired(&rx), "in-place write");
    // Atomic save: write elsewhere, rename over. The watched inode is gone; is_current says so.
    std::fs::write(dir.join("tmp"), "ccc").unwrap();
    std::fs::rename(dir.join("tmp"), &file).unwrap();
    assert!(fired(&rx), "replaced by rename");
    assert!(!w.is_current(&path), "now names another inode");
    assert!(file_id(&path).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_path_is_none_and_drop_closes_the_fd() {
    let queue = DispatchQueue::new("org.wgbar.test.watch2", DispatchQueueAttr::SERIAL);
    assert!(Watch::new("/nonexistent/wgbar", &queue, || {}).is_none());
    let dir = temp_dir("drop");
    let count = || std::fs::read_dir("/dev/fd").unwrap().count();
    let before = count();
    for _ in 0..50 {
        drop(Watch::new(dir.to_str().unwrap(), &queue, || {}).unwrap());
    }
    // Cancel handlers close fds asynchronously on the queue; let them run.
    queue.exec_sync(|| {});
    std::thread::sleep(Duration::from_millis(100));
    assert!(count() <= before + 1, "watch fds are closed on drop");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ticker_fires_repeatedly_and_stops_on_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wgbar::watch::Ticker;
    let queue = DispatchQueue::new("org.wgbar.test.ticker", DispatchQueueAttr::SERIAL);
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let ticker = Ticker::new(
        &queue,
        Duration::from_millis(20),
        Duration::ZERO,
        move || {
            c.fetch_add(1, Ordering::SeqCst);
        },
    );
    std::thread::sleep(Duration::from_millis(250));
    drop(ticker);
    queue.exec_sync(|| {});
    let fired = count.load(Ordering::SeqCst);
    assert!(fired >= 5, "about 12 ticks in 250 ms, got {fired}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(count.load(Ordering::SeqCst), fired, "no ticks after drop");
}
