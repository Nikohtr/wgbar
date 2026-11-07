//! Dispatch sources the app is driven by, macOS only:
//! - [`Watch`]: file-system change notifications (kqueue vnode events), so the app reacts to
//!   tunnels going up or down and to network preference changes when they happen, instead of
//!   waking every 2 s to look.
//! - [`Ticker`]: a repeating timer that fires on a given queue, so the On-Demand poll runs on
//!   its own serial queue without waking the main thread.

use std::ffi::CString;
use std::os::unix::fs::MetadataExt;
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::{
    dispatch_source_type_t, dispatch_source_vnode_flags_t, DispatchObject, DispatchQueue,
    DispatchRetained, DispatchSource, DispatchTime,
};

/// Something on disk to watch: a directory (entries added, removed or renamed) or a file
/// (written, replaced or deleted).
pub struct Watch {
    path: String,
    /// Device and inode at the time the watch was set: a path now naming another object (a
    /// file replaced by rename, a folder re-created) needs a new watch.
    id: (u64, u64),
    source: DispatchRetained<DispatchSource>,
}

/// Device and inode of `path`, if it exists.
pub fn file_id(path: &str) -> Option<(u64, u64)> {
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl Watch {
    /// Start watching `path`; `on_change` runs on `queue` after each change (several changes in
    /// quick succession may be reported once). `None` when the path cannot be opened (missing,
    /// or no permission).
    pub fn new(
        path: &str,
        queue: &DispatchQueue,
        on_change: impl Fn() + Send + 'static,
    ) -> Option<Watch> {
        let c = CString::new(path).ok()?;
        // SAFETY: `c` is a valid path. O_EVTONLY: for notifications only, so the watch never
        // keeps a volume from unmounting.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let Some(id) = file_id(path) else {
            // SAFETY: `fd` was opened above and is not used elsewhere.
            unsafe { libc::close(fd) };
            return None;
        };
        let mask = dispatch_source_vnode_flags_t::DISPATCH_VNODE_WRITE.0
            | dispatch_source_vnode_flags_t::DISPATCH_VNODE_EXTEND.0
            | dispatch_source_vnode_flags_t::DISPATCH_VNODE_DELETE.0
            | dispatch_source_vnode_flags_t::DISPATCH_VNODE_RENAME.0
            | dispatch_source_vnode_flags_t::DISPATCH_VNODE_REVOKE.0;
        let vnode = &raw const dispatch2::_dispatch_source_type_vnode as dispatch_source_type_t;
        // SAFETY: a vnode source takes an open fd as handle and vnode flags as mask.
        let source = unsafe { DispatchSource::new(vnode, fd as usize, mask as usize, Some(queue)) };
        let event = RcBlock::new(on_change);
        // The fd belongs to the source until it is cancelled.
        let cancel = RcBlock::new(move || {
            // SAFETY: the fd was opened for this source, and the cancel handler runs once.
            unsafe { libc::close(fd) };
        });
        // SAFETY: dispatch copies both blocks. `on_change` is Send, as it runs on `queue`.
        unsafe {
            source.set_event_handler_with_block(block_ptr(&event));
            source.set_cancel_handler_with_block(block_ptr(&cancel));
        }
        source.activate();
        Some(Watch {
            path: path.to_string(),
            id,
            source,
        })
    }

    /// Still watching what `path` names now?
    pub fn is_current(&self, path: &str) -> bool {
        self.path == path && file_id(path) == Some(self.id)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.source.cancel();
    }
}

fn block_ptr(block: &RcBlock<dyn Fn()>) -> dispatch2::dispatch_block_t {
    NonNull::from(&**block).as_ptr()
}

/// A repeating timer on a dispatch queue; stops when dropped. On a serial queue the handler
/// never overlaps itself: ticks that fall due while it runs are coalesced into one.
pub struct Ticker {
    source: DispatchRetained<DispatchSource>,
}

impl Ticker {
    /// Fire `on_tick` on `queue` every `interval`, first after one interval; `leeway` is how
    /// late the system may fire to batch it with other wakeups.
    pub fn new(
        queue: &DispatchQueue,
        interval: std::time::Duration,
        leeway: std::time::Duration,
        on_tick: impl Fn() + Send + 'static,
    ) -> Ticker {
        let timer = &raw const dispatch2::_dispatch_source_type_timer as dispatch_source_type_t;
        // SAFETY: a timer source takes no handle and no mask.
        let source = unsafe { DispatchSource::new(timer, 0, 0, Some(queue)) };
        let nanos = |d: std::time::Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        let start = DispatchTime::NOW.time(i64::try_from(nanos(interval)).unwrap_or(i64::MAX));
        source.set_timer(start, nanos(interval), nanos(leeway));
        let tick = RcBlock::new(on_tick);
        // SAFETY: dispatch copies the block. `on_tick` is Send, as it runs on `queue`.
        unsafe { source.set_event_handler_with_block(block_ptr(&tick)) };
        source.activate();
        Ticker { source }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.source.cancel();
    }
}
