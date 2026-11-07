//! `AppDelegate`: the status item, its menu, and everything they trigger.

use std::cell::{Cell, OnceCell, RefCell};
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use block2::RcBlock;
use dispatch2::{
    DispatchQoS, DispatchQueue, DispatchQueueAttr, DispatchRetained, GlobalQueueIdentifier,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationDelegate,
    NSControlStateValueOff, NSControlStateValueOn, NSEventMask, NSEventType, NSMenu,
    NSMenuDelegate, NSMenuItem, NSModalResponseOK, NSOpenPanel, NSSquareStatusItemLength,
    NSStatusBar, NSStatusBarButton, NSStatusItem, NSWorkspace, NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{
    NSHomeDirectory, NSNotification, NSObject, NSOperationQueue, NSString, NSTimer, NSURL,
};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use wgbar::dns_guard::{config_dns_search, config_dns_servers, DnsGuard, RepairResult};
use wgbar::on_demand::{
    is_full_tunnel, native_reader, parse_allowed_ips, Cidr, OnDemandController, OnDemandState,
};
use wgbar::tunnels::{
    any_tunnel_connected, any_tunnel_up, conf_names, network_prefs_modified, run_file,
    tunnel_address, NETWORK_PREFS, RUN_DIR,
};
use wgbar::updater::{UpdateStatus, Updater};
use wgbar::watch::{file_id, Ticker, Watch};

use super::defaults;
use super::ui::{self, ns};
use super::{
    all_config_dns, available_tunnels, brew_bin, conf_dir, conf_path, on_main, on_main_after,
    read_conf, runner, with_delegate,
};

/// The On-Demand controller as seen from its queue.
type OdOwned = Arc<Mutex<Option<OnDemandController>>>;

/// Runs after `stop_on_demand` finished, on the main thread.
type Completion = Box<dyn FnOnce(&AppDelegate) + Send>;

pub struct Ivars {
    status_item: OnceCell<Retained<NSStatusItem>>,
    menu: Retained<NSMenu>,
    status_line: Retained<NSMenuItem>,
    dns_line: Retained<NSMenuItem>,
    toggle_item: Retained<NSMenuItem>,
    login_item: Retained<NSMenuItem>,
    tunnel_item: Retained<NSMenuItem>,
    folder_item: Retained<NSMenuItem>,
    repair_item: Retained<NSMenuItem>,
    update_item: Retained<NSMenuItem>,
    on_demand_item: Retained<NSMenuItem>,
    /// Keeps the wake observer registered for the life of the app.
    wake_observer: OnceCell<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    /// The controller as seen from od_queue: set right after a successful arm/reconcile,
    /// cleared after shutdown. Locked only from closures running on `od_queue`, so a quit can
    /// always drain it.
    od_owned: OdOwned,
    /// Mirror of the controller's state for the main thread (updated via on_change).
    od_state: Cell<OnDemandState>,
    od_tunnel: RefCell<Option<String>>,
    od_queue: DispatchRetained<DispatchQueue>,
    tunnel: RefCell<String>,
    is_up: Cell<bool>,
    /// A start, stop or toggle is in flight. Shared with the On-Demand ticker on `od_queue`,
    /// which skips its poll meanwhile.
    busy: SharedFlag,
    /// Services still pointing at VPN DNS that could not be repaired automatically.
    dns_issue: RefCell<Vec<String>>,
    dns_checking: Cell<bool>,
    prefs_seen: Cell<SystemTime>,
    /// The 1 s On-Demand poll, on `od_queue` (no main-thread wakeup per tick); running only
    /// while On-Demand is active.
    tick_timer: RefCell<Option<Ticker>>,
    /// What the status button shows now, so unchanged values are not re-sent: on macOS 27 every
    /// image or tooltip set is relayed to MenuBarAgent, which draws the item.
    shown_icon: Cell<&'static str>,
    shown_tip: RefCell<String>,
    /// What `refresh` reads, watched for changes (see `sync_watches`); `None` for a path that
    /// does not exist (yet).
    watches: RefCell<Vec<(String, Option<Watch>)>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `AppDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WGBarAppDelegate"]
    #[ivars = Ivars]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _note: &NSNotification) {
            self.did_finish_launching();
        }

        /// Quit: an armed interface with nobody watching would black-hole the VPN subnets.
        /// Take it down. Unconditional, so the queue drains first: an arm still running
        /// finishes and is then shut down, and a shutdown already queued by `stop_on_demand`
        /// completes before the process goes away.
        #[unsafe(method(applicationWillTerminate:))]
        fn application_will_terminate(&self, _note: &NSNotification) {
            let owned = Arc::clone(&self.ivars().od_owned);
            self.ivars().od_queue.exec_sync(move || {
                let od = lock(&owned).take();
                if let Some(mut od) = od {
                    od.shutdown();
                }
            });
        }
    }

    unsafe impl NSMenuDelegate for AppDelegate {
        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            // So the next left-click toggles instead of opening the menu.
            if let Some(item) = self.ivars().status_item.get() {
                item.setMenu(None);
            }
        }
    }

    /// Action methods (targets of the status button and menu items).
    impl AppDelegate {
        #[unsafe(method(clicked:))]
        fn clicked(&self, _sender: Option<&AnyObject>) {
            let event = NSApplication::sharedApplication(self.mtm()).currentEvent();
            if event.is_some_and(|e| e.r#type() == NSEventType::RightMouseUp) {
                self.show_menu();
            } else {
                self.toggle();
            }
        }

        #[unsafe(method(toggle:))]
        fn toggle_action(&self, _sender: Option<&AnyObject>) {
            self.toggle();
        }

        #[unsafe(method(repairDNS:))]
        fn repair_dns(&self, _sender: Option<&AnyObject>) {
            self.check_dns(true);
        }

        #[unsafe(method(selectTunnel:))]
        fn select_tunnel(&self, sender: &NSMenuItem) {
            let title = sender.title().to_string();
            if self.ivars().busy.get() || title == self.tunnel() {
                return;
            }
            defaults::set_string(defaults::TUNNEL, &title);
            *self.ivars().tunnel.borrow_mut() = title;
            if self.od_active() {
                self.restart_on_demand();
            } else {
                self.refresh();
            }
        }

        /// Menu checkbox: enable or disable Connect on Demand for the selected tunnel.
        #[unsafe(method(toggleOnDemand:))]
        fn toggle_on_demand(&self, _sender: Option<&AnyObject>) {
            if self.ivars().busy.get() {
                return;
            }
            if self.od_active() {
                self.stop_on_demand(None);
                return;
            }
            self.start_on_demand(false);
        }

        #[unsafe(method(chooseFolder:))]
        fn choose_folder_action(&self, _sender: Option<&AnyObject>) {
            self.choose_folder();
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates_action(&self, _sender: Option<&AnyObject>) {
            self.check_for_updates();
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let service = main_app_service();
            // SAFETY: plain SMAppService calls on the main app's service object.
            let result = unsafe {
                if service.status() == SMAppServiceStatus::Enabled {
                    service.unregisterAndReturnError()
                } else {
                    service.registerAndReturnError()
                }
            };
            if let Err(error) = result {
                // Swift `"\(error)"`: the NSError description (domain, code, message).
                // SAFETY: `-[NSError description]` takes no arguments and returns an NSString.
                let description: Retained<NSString> = unsafe { msg_send![&*error, description] };
                self.show_error("login item", &description.to_string());
            }
        }
    }
);

/// A flag the main thread sets and a background queue reads.
#[derive(Default)]
struct SharedFlag(Arc<AtomicBool>);

impl SharedFlag {
    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self, value: bool) {
        self.0.store(value, Ordering::Relaxed);
    }
}

/// Lock the controller. Only ever called on `od_queue`, so it never waits on another thread;
/// a panic there (debug builds only: release aborts) must not wedge the app at quit.
fn lock(owned: &Mutex<Option<OnDemandController>>) -> MutexGuard<'_, Option<OnDemandController>> {
    owned.lock().unwrap_or_else(PoisonError::into_inner)
}

fn main_app_service() -> Retained<SMAppService> {
    // SAFETY: `mainAppService` has no preconditions; it describes this app as a login item.
    unsafe { SMAppService::mainAppService() }
}

fn login_item_enabled() -> bool {
    // SAFETY: reading the status has no preconditions.
    unsafe { main_app_service().status() == SMAppServiceStatus::Enabled }
}

fn global_queue(qos: DispatchQoS) -> DispatchRetained<DispatchQueue> {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(qos))
}

/// The `onDemandIdle` default as a `Duration`. Negative or NaN means "disconnect as soon as
/// idle", too large for a `Duration` means "never".
fn idle_duration(seconds: f64) -> Duration {
    if seconds.is_nan() {
        Duration::MAX // Swift: `quietFor >= .nan` is never true, so it never idles out
    } else if seconds <= 0.0 {
        Duration::ZERO
    } else {
        Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX)
    }
}

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let item = |title: &str, action| ui::menu_item(mtm, title, action, "");
        let this = Self::alloc(mtm).set_ivars(Ivars {
            status_item: OnceCell::new(),
            menu: NSMenu::new(mtm),
            status_line: item("", None),
            dns_line: item("", None),
            toggle_item: item("", Some(sel!(toggle:))),
            login_item: item("Launch at Login", Some(sel!(toggleLogin:))),
            tunnel_item: item("Tunnel", None),
            folder_item: item("Config Folder…", Some(sel!(chooseFolder:))),
            repair_item: item("Repair DNS", Some(sel!(repairDNS:))),
            update_item: item("Check for Updates…", Some(sel!(checkForUpdates:))),
            on_demand_item: item("Connect on Demand", Some(sel!(toggleOnDemand:))),
            wake_observer: OnceCell::new(),
            od_owned: Arc::new(Mutex::new(None)),
            od_state: Cell::new(OnDemandState::Off),
            od_tunnel: RefCell::new(None),
            od_queue: DispatchQueue::new("org.wgbar.ondemand", DispatchQueueAttr::SERIAL),
            tunnel: RefCell::new(String::new()),
            is_up: Cell::new(false),
            busy: SharedFlag::default(),
            dns_issue: RefCell::new(Vec::new()),
            dns_checking: Cell::new(false),
            prefs_seen: Cell::new(network_prefs_modified()),
            tick_timer: RefCell::new(None),
            shown_icon: Cell::new(""),
            shown_tip: RefCell::new(String::new()),
            watches: RefCell::new(Vec::new()),
        });
        // SAFETY: `init` on an NSObject subclass whose ivars are already set.
        unsafe { msg_send![super(this), init] }
    }

    fn as_target(&self) -> &AnyObject {
        self
    }

    fn tunnel(&self) -> String {
        self.ivars().tunnel.borrow().clone()
    }

    fn od_active(&self) -> bool {
        self.ivars().od_tunnel.borrow().is_some()
    }

    fn status_button(&self) -> Option<Retained<NSStatusBarButton>> {
        self.ivars().status_item.get()?.button(self.mtm())
    }

    /// The tunnel to control: the remembered choice if its config still exists, else the
    /// first one found.
    fn resolve_tunnel(&self) {
        let all = available_tunnels();
        let saved = defaults::string(defaults::TUNNEL).unwrap_or_default();
        let tunnel = if all.contains(&saved) {
            saved
        } else {
            all.into_iter().next().unwrap_or_default()
        };
        *self.ivars().tunnel.borrow_mut() = tunnel;
    }

    fn did_finish_launching(&self) {
        let mtm = self.mtm();
        let iv = self.ivars();

        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSSquareStatusItemLength);
        if let Some(button) = status_item.button(mtm) {
            // SAFETY: `clicked:` is implemented by `self`, which outlives the (unretained)
            // target reference: the delegate lives for the whole process.
            unsafe {
                button.setTarget(Some(self.as_target()));
                button.setAction(Some(sel!(clicked:)));
            }
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
        let _ = iv.status_item.set(status_item);

        iv.menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        iv.status_line.setEnabled(false);
        iv.dns_line.setEnabled(false);
        for item in [
            &iv.toggle_item,
            &iv.login_item,
            &iv.folder_item,
            &iv.repair_item,
            &iv.update_item,
            &iv.on_demand_item,
        ] {
            // SAFETY: each item's action is a method of `self`, which lives for the process.
            unsafe { item.setTarget(Some(self.as_target())) };
        }
        // Explicit enabled states (busy, DNS check running, full tunnel) only take effect with
        // autoenabling off; the Swift app left it on, so its setEnabled calls were ignored.
        iv.menu.setAutoenablesItems(false);
        let submenu = NSMenu::new(mtm);
        submenu.setAutoenablesItems(false);
        iv.tunnel_item.setSubmenu(Some(&submenu));
        iv.menu.addItem(&iv.status_line);
        iv.menu.addItem(&iv.dns_line);
        iv.menu.addItem(&iv.toggle_item);
        iv.menu.addItem(&iv.repair_item);
        iv.menu.addItem(&NSMenuItem::separatorItem(mtm));
        iv.menu.addItem(&iv.tunnel_item);
        iv.menu.addItem(&iv.on_demand_item);
        iv.menu.addItem(&iv.folder_item);
        iv.menu.addItem(&iv.login_item);
        iv.menu.addItem(&iv.update_item);
        iv.menu.addItem(&NSMenuItem::separatorItem(mtm));
        // No target: `terminate:` travels the responder chain to NSApp. ⌘Q.
        iv.menu.addItem(&ui::menu_item(
            mtm,
            "Quit WGBar",
            Some(sel!(terminate:)),
            "q",
        ));

        // Register as a login item on first launch; the menu checkbox controls it afterwards.
        if !defaults::bool(defaults::DID_REGISTER_LOGIN_ITEM) {
            // SAFETY: plain SMAppService call; failure (e.g. unsigned build) is ignored as in Swift.
            let _ = unsafe { main_app_service().registerAndReturnError() };
            defaults::set_bool(defaults::DID_REGISTER_LOGIN_ITEM, true);
        }

        self.resolve_tunnel();
        self.refresh();
        // The blocks capture nothing (they reach the delegate through `with_delegate`), so they
        // are trivially sendable; scheduled here, they fire on the main run loop.
        // File-system events drive `refresh` (see `sync_watches`); this slow timer only catches
        // what they cannot see, such as a config edited in place (its address is in the tooltip).
        let refresh = RcBlock::new(|_: NonNull<NSTimer>| with_delegate(|d| d.refresh()));
        // SAFETY: the block is sendable (see above). The run loop retains the timer.
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(30.0, true, &refresh) };
        // Let the system coalesce this wakeup with others (Apple suggests >= 10% of the interval).
        timer.setTolerance(5.0);

        // DNS guard: a reboot or crash with the tunnel up leaves VPN DNS behind; so can waking.
        self.check_dns(false);
        let on_wake = RcBlock::new(|_: NonNull<NSNotification>| {
            with_delegate(|d| {
                d.reconcile_on_demand();
                on_main_after(3, |d| d.check_dns(false));
            })
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: a valid notification name, no object filter, the main operation queue (so
        // the block runs on the main thread), and a block that captures nothing (sendable).
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &on_wake,
            )
        };
        let _ = iv.wake_observer.set(observer);

        // Last: start_on_demand sets busy, which would make the launch-time check_dns above
        // bail out.
        if defaults::bool(defaults::ON_DEMAND) {
            self.start_on_demand(true);
        }
    }

    // MARK: State

    fn refresh(&self) {
        let iv = self.ivars();
        // Before reading state: a change after this point is guaranteed to fire an event.
        self.sync_watches();
        self.sync_tick_timer();
        if iv.tunnel.borrow().is_empty() {
            self.resolve_tunnel();
        }
        let was_up = iv.is_up.get();
        let is_up = if self.od_active() {
            iv.od_state.get() == OnDemandState::Connected
        } else {
            let tunnel = self.tunnel();
            !tunnel.is_empty() && Path::new(&run_file(&tunnel)).exists()
        };
        iv.is_up.set(is_up);
        self.update_icon();
        // The tunnel went down outside WGBar, or the network preferences changed: re-check DNS.
        let prefs_now = network_prefs_modified();
        if (was_up && !is_up) || prefs_now != iv.prefs_seen.get() {
            iv.prefs_seen.set(prefs_now);
            on_main_after(3, |d| d.check_dns(false));
        }
    }

    /// Watch what `refresh` reads: the wg-quick run folder (a tunnel going up or down adds or
    /// removes its `.name` file there; until wg-quick first creates the folder, its parent),
    /// the network preferences (DNS changes) and the config folder (configs added or removed).
    /// Re-armed whenever one of them now names a different object, e.g. a replaced file.
    fn sync_watches(&self) {
        let run = if Path::new(RUN_DIR).is_dir() {
            RUN_DIR
        } else {
            "/var/run"
        };
        let paths = [run.to_string(), NETWORK_PREFS.to_string(), conf_dir()];
        let mut watches = self.ivars().watches.borrow_mut();
        let current = watches.len() == paths.len()
            && watches.iter().zip(&paths).all(|((path, watch), want)| {
                path == want
                    && match watch {
                        Some(w) => w.is_current(want),
                        None => file_id(want).is_none(),
                    }
            });
        if current {
            return;
        }
        // Drop the old watches first so their fds are released before new ones are opened.
        watches.clear();
        for path in paths {
            let watch = Watch::new(&path, DispatchQueue::main(), || {
                with_delegate(|d| d.refresh())
            });
            watches.push((path, watch));
        }
    }

    fn update_icon(&self) {
        let iv = self.ivars();
        let busy = iv.busy.get();
        let is_up = iv.is_up.get();
        let od_state = iv.od_state.get();
        let armed =
            self.od_active() && matches!(od_state, OnDemandState::Armed | OnDemandState::Paused);
        let dns_issue = iv.dns_issue.borrow().clone();
        let name = if busy {
            "shield.lefthalf.filled"
        } else if is_up {
            "shield.fill"
        } else if armed {
            "shield"
        } else if dns_issue.is_empty() {
            "shield.slash"
        } else {
            "exclamationmark.shield"
        };
        let description = if busy {
            "WireGuard switching"
        } else if is_up {
            "WireGuard on"
        } else if armed {
            "WireGuard armed"
        } else {
            "WireGuard off"
        };
        let Some(button) = self.status_button() else {
            return;
        };
        // The description is a function of the name, so the name alone identifies the image.
        if iv.shown_icon.get() != name {
            button.setImage(ui::symbol(name, description).as_deref());
            iv.shown_icon.set(name);
        }
        let tip = if self.tunnel().is_empty() {
            format!("No WireGuard configs found in {}", conf_dir())
        } else if !dns_issue.is_empty() && !is_up {
            format!(
                "VPN DNS left behind on {} — right-click → Repair DNS",
                dns_issue.join(", ")
            )
        } else {
            format!("{} — click to toggle", self.status_text())
        };
        if *iv.shown_tip.borrow() != tip {
            button.setToolTip(Some(&ns(&tip)));
            *iv.shown_tip.borrow_mut() = tip;
        }
    }

    /// "ileasing: Connected" etc., shared by the menu status line and the icon tooltip.
    fn status_text(&self) -> String {
        let tunnel = self.tunnel();
        let addr = read_conf(&tunnel)
            .and_then(|text| tunnel_address(&text))
            .map(|a| format!(" — {a}"))
            .unwrap_or_default();
        if !self.od_active() {
            let state = if self.ivars().is_up.get() {
                format!("Connected{addr}")
            } else {
                "Disconnected".to_string()
            };
            return format!("{tunnel}: {state}");
        }
        match self.ivars().od_state.get() {
            OnDemandState::Connected => format!("{tunnel}: Connected (on demand){addr}"),
            OnDemandState::Armed => format!("{tunnel}: Armed, connects on demand"),
            OnDemandState::Paused => format!("{tunnel}: Paused"),
            OnDemandState::Off => format!("{tunnel}: Off (on demand not armed)"),
        }
    }

    // MARK: DNS guard

    /// If no tunnel is up but some network service still points at a VPN DNS server, restore
    /// that service's pre-connect DNS (or clear it, i.e. back to DHCP) and say so.
    fn check_dns(&self, interactive: bool) {
        let iv = self.ivars();
        let armed = if iv.od_state.get() != OnDemandState::Connected {
            iv.od_tunnel.borrow().clone()
        } else {
            None
        };
        if iv.busy.get() || iv.dns_checking.get() || any_tunnel_connected(armed.as_deref()) {
            return;
        }
        let vpn_dns = all_config_dns();
        if vpn_dns.is_empty() {
            return;
        }
        iv.dns_checking.set(true);
        let snapshot = defaults::dns_snapshot();
        let run = runner();
        global_queue(DispatchQoS::Utility).exec_async(move || {
            let guardian = DnsGuard::new(vpn_dns, run);
            let stale = guardian.stale();
            let result = if stale.is_empty() {
                RepairResult::default()
            } else {
                guardian.repair(&snapshot)
            };
            let still_stale = if stale.is_empty() {
                Vec::new()
            } else {
                guardian.stale()
            };
            on_main(move |d| {
                let iv = d.ivars();
                iv.dns_checking.set(false);
                *iv.dns_issue.borrow_mut() = still_stale;
                // Our own repair changed the file; don't loop on it.
                iv.prefs_seen.set(network_prefs_modified());
                d.update_icon();
                if !result.fixed.is_empty() {
                    ui::notify(&format!(
                        "Restored DNS on {} that WireGuard left behind.",
                        result.fixed.join(", ")
                    ));
                }
                if !result.errors.is_empty() && interactive {
                    d.show_error(
                        "",
                        &format!("Could not repair DNS:\n{}", result.errors.join("\n")),
                    );
                } else if stale.is_empty() && interactive {
                    d.show_error_styled(
                        "",
                        "DNS is clean — no VPN servers left behind.",
                        NSAlertStyle::Informational,
                    );
                }
            });
        });
    }

    // MARK: Actions

    fn show_menu(&self) {
        self.resolve_tunnel();
        self.refresh();
        let iv = self.ivars();
        let tunnel = self.tunnel();
        let busy = iv.busy.get();
        let is_up = iv.is_up.get();
        if tunnel.is_empty() {
            iv.status_line
                .setTitle(&ns(&format!("No configs in {}", conf_dir())));
            iv.toggle_item.setTitle(&ns("Connect"));
            iv.toggle_item.setEnabled(false);
        } else {
            iv.status_line.setTitle(&ns(&self.status_text()));
            iv.toggle_item
                .setTitle(&ns(if is_up { "Disconnect" } else { "Connect" }));
            iv.toggle_item.setEnabled(!busy);
        }
        let dns_issue = iv.dns_issue.borrow().clone();
        iv.dns_line.setHidden(is_up || dns_issue.is_empty());
        iv.dns_line.setTitle(&ns(&format!(
            "VPN DNS left behind on {}",
            dns_issue.join(", ")
        )));
        iv.repair_item.setHidden(is_up);
        iv.repair_item.setEnabled(!busy && !iv.dns_checking.get());
        iv.update_item.setEnabled(!busy);
        self.rebuild_tunnel_submenu();
        let conf_text = if tunnel.is_empty() {
            String::new()
        } else {
            read_conf(&tunnel).unwrap_or_default()
        };
        let full = is_full_tunnel(&parse_allowed_ips(&conf_text));
        let od_active = self.od_active();
        iv.on_demand_item.setState(if od_active {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        iv.on_demand_item
            .setEnabled(!busy && !tunnel.is_empty() && (od_active || !full));
        iv.on_demand_item.setToolTip(Some(&ns(if full {
            "Not available for full-tunnel configs (AllowedIPs includes 0.0.0.0/0)"
        } else {
            "Keep the tunnel armed and connect it when traffic to its AllowedIPs appears"
        })));
        iv.folder_item.setToolTip(Some(&ns(&conf_dir())));
        iv.login_item.setState(if login_item_enabled() {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let Some(status_item) = iv.status_item.get() else {
            return;
        };
        // The menu is attached only while open (menuDidClose detaches it), so that a
        // left-click toggles. performClick opens it and tracks it until it closes.
        status_item.setMenu(Some(&iv.menu));
        if let Some(button) = status_item.button(self.mtm()) {
            // SAFETY: a nil sender is allowed.
            unsafe { button.performClick(None) };
        }
    }

    /// "Tunnel ▸" submenu, only shown when there is something to choose between.
    fn rebuild_tunnel_submenu(&self) {
        let iv = self.ivars();
        let all = available_tunnels();
        iv.tunnel_item.setHidden(all.len() < 2);
        let Some(sub) = iv.tunnel_item.submenu() else {
            return;
        };
        sub.removeAllItems();
        let tunnel = self.tunnel();
        let busy = iv.busy.get();
        for name in all {
            let item = ui::menu_item(self.mtm(), &name, Some(sel!(selectTunnel:)), "");
            // SAFETY: `selectTunnel:` is a method of `self`, which lives for the process.
            unsafe { item.setTarget(Some(self.as_target())) };
            item.setState(if name == tunnel {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            item.setEnabled(!busy);
            sub.addItem(&item);
        }
    }

    fn toggle(&self) {
        let iv = self.ivars();
        if iv.busy.get() {
            return;
        }
        if self.od_active() {
            iv.busy.set(true);
            self.update_icon();
            let owned = Arc::clone(&iv.od_owned);
            iv.od_queue.exec_async(move || {
                if let Some(od) = lock(&owned).as_mut() {
                    od.manual_toggle();
                }
                on_main(|d| {
                    d.ivars().busy.set(false);
                    d.refresh();
                });
            });
            return;
        }
        let tunnel = self.tunnel();
        if tunnel.is_empty() {
            self.show_error(
                "",
                &format!(
                    "No WireGuard configs found in {}.\nPut a <name>.conf there (see README) and try again.",
                    conf_dir()
                ),
            );
            return;
        }
        iv.busy.set(true);
        self.update_icon();
        let action = if iv.is_up.get() { "down" } else { "up" };
        let vpn_dns = all_config_dns();
        let run = runner();
        global_queue(DispatchQoS::UserInitiated).exec_async(move || {
            // Remember the DNS settings to put back, before wg-quick overwrites them.
            if action == "up" && !vpn_dns.is_empty() && !any_tunnel_up() {
                defaults::set_dns_snapshot(&DnsGuard::new(vpn_dns, run).snapshot());
            }
            let error = super::wg_quick(action, &tunnel);
            on_main(move |d| {
                d.ivars().busy.set(false);
                d.refresh();
                if let Some(error) = error {
                    d.show_error(action, &error);
                }
                // Give wg-quick's own monitor a moment to restore DNS; fix whatever it did not.
                if action == "down" {
                    on_main_after(3, |d| d.check_dns(false));
                }
            });
        });
    }

    // MARK: On demand

    /// Run the 1 s poll exactly while On-Demand is active. Called from `refresh`, which follows
    /// every On-Demand start and state change, and from `stop_on_demand`.
    /// The poll runs on `od_queue` itself: serial, so it never overlaps a start, stop or toggle
    /// queued there, and skipped while one is in flight (`busy`), as the main-thread timer was.
    fn sync_tick_timer(&self) {
        let iv = self.ivars();
        let mut timer = iv.tick_timer.borrow_mut();
        match (self.od_active(), timer.is_some()) {
            (true, false) => {
                let owned = Arc::clone(&iv.od_owned);
                let busy = Arc::clone(&iv.busy.0);
                *timer = Some(Ticker::new(
                    &iv.od_queue,
                    Duration::from_secs(1),
                    Duration::from_millis(100),
                    move || {
                        if busy.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(od) = lock(&owned).as_mut() {
                            od.tick();
                        }
                    },
                ));
            }
            (false, true) => *timer = None,
            _ => {}
        }
    }

    /// After a wake the interface may have changed under us: re-read its real state.
    fn reconcile_on_demand(&self) {
        let owned = Arc::clone(&self.ivars().od_owned);
        self.ivars().od_queue.exec_async(move || {
            if let Some(od) = lock(&owned).as_mut() {
                od.reconcile();
            }
        });
    }

    fn make_controller(&self, tunnel: &str, cidrs: Vec<Cidr>) -> OnDemandController {
        let idle = defaults::number(defaults::ON_DEMAND_IDLE).unwrap_or(30.0);
        let text = read_conf(tunnel).unwrap_or_default();
        let mut od = OnDemandController::new(
            tunnel.to_string(),
            cidrs,
            idle_duration(idle),
            config_dns_servers(&text).into_iter().next(),
            runner(),
            Arc::new(SystemTime::now),
        );
        // In-process libproc scan instead of spawning lsof every tick.
        od.set_socket_reader(native_reader(runner()));
        // The callbacks run on od_queue.
        let connected_tunnel = tunnel.to_string();
        od.set_on_connected(move || apply_vpn_dns(&connected_tunnel));
        od.set_on_disconnected(restore_dns);
        od.set_on_error(|message| on_main(move |d| d.show_error("", &message)));
        od.set_on_change(|state| {
            on_main(move |d| {
                d.ivars().od_state.set(state);
                d.refresh();
            })
        });
        od
    }

    /// Arm the selected tunnel. At launch, adopt the interface's real state first (it may
    /// still be up).
    fn start_on_demand(&self, at_launch: bool) {
        let tunnel = self.tunnel();
        let fail = |message: &str| {
            defaults::set_bool(defaults::ON_DEMAND, false);
            self.show_error("", message);
        };
        if tunnel.is_empty() {
            return fail(&format!("No WireGuard configs found in {}.", conf_dir()));
        }
        if !OnDemandController::helper_installed() {
            return fail(OnDemandController::NEEDS_HELPER_MESSAGE);
        }
        let Some(text) = read_conf(&tunnel) else {
            return fail(&format!("Could not read {}.", conf_path(&tunnel)));
        };
        let cidrs = parse_allowed_ips(&text);
        if cidrs.is_empty() {
            return fail("The config has no AllowedIPs line.");
        }
        if is_full_tunnel(&cidrs) {
            return fail(
                "Connect on Demand needs a split-tunnel config (specific AllowedIPs, not 0.0.0.0/0).",
            );
        }
        let iv = self.ivars();
        iv.busy.set(true);
        self.update_icon();
        let mut od = self.make_controller(&tunnel, cidrs);
        let owned = Arc::clone(&iv.od_owned);
        iv.od_queue.exec_async(move || {
            // A classic `wg-quick up` interface carries VPN DNS and a monitor that re-applies
            // it: replace it.
            if !at_launch && Path::new(&run_file(&tunnel)).exists() {
                if let Some(error) = super::wg_quick("down", &tunnel) {
                    on_main(move |d| {
                        d.ivars().busy.set(false);
                        d.refresh();
                        d.show_error("down", &error);
                    });
                    return;
                }
            }
            if at_launch {
                od.reconcile();
            }
            let ok = od.state() != OnDemandState::Off || od.arm();
            let state = od.state();
            // Take ownership here, on od_queue: a quit that lands before the main-thread
            // mirrors are set must still find the live controller and shut it down.
            if ok {
                *lock(&owned) = Some(od);
            }
            on_main(move |d| {
                let iv = d.ivars();
                iv.busy.set(false);
                if ok {
                    *iv.od_tunnel.borrow_mut() = Some(tunnel);
                    iv.od_state.set(state);
                    defaults::set_bool(defaults::ON_DEMAND, true);
                } else {
                    defaults::set_bool(defaults::ON_DEMAND, false);
                }
                d.refresh();
            });
        });
    }

    /// Take the on-demand interface down and return to classic mode.
    fn stop_on_demand(&self, completion: Option<Completion>) {
        let iv = self.ivars();
        if !self.od_active() {
            if let Some(completion) = completion {
                completion(self);
            }
            return;
        }
        iv.busy.set(true);
        self.update_icon();
        *iv.od_tunnel.borrow_mut() = None;
        self.sync_tick_timer();
        iv.od_state.set(OnDemandState::Off);
        defaults::set_bool(defaults::ON_DEMAND, false);
        let owned = Arc::clone(&iv.od_owned);
        iv.od_queue.exec_async(move || {
            let od = lock(&owned).take();
            if let Some(mut od) = od {
                od.shutdown();
            }
            on_main(move |d| {
                d.ivars().busy.set(false);
                d.refresh();
                if let Some(completion) = completion {
                    completion(d);
                }
            });
        });
    }

    /// The tunnel or config folder changed while On-Demand is on: re-arm for the new selection.
    fn restart_on_demand(&self) {
        if !self.od_active() {
            return;
        }
        self.stop_on_demand(Some(Box::new(|d| {
            d.resolve_tunnel();
            d.start_on_demand(false);
        })));
    }

    /// Pick the folder holding <name>.conf files (for setups outside the default locations).
    fn choose_folder(&self) {
        // A folder change while an on-demand start/stop is in flight would arm the old tunnel.
        if self.ivars().busy.get() {
            return;
        }
        let mtm = self.mtm();
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&ns(&conf_dir()))));
        panel.setMessage(Some(&ns(
            "Choose the folder containing your WireGuard .conf files",
        )));
        panel.setPrompt(Some(&ns("Use Folder")));
        ui::activate(mtm);
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(dir) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        let dir = dir.to_string();
        if conf_names(&dir).is_empty() {
            self.show_error("", &format!("No .conf files found in {dir}"));
            return;
        }
        defaults::set_string(defaults::CONF_DIR, &dir);
        defaults::remove(defaults::TUNNEL);
        self.ivars().tunnel.borrow_mut().clear();
        self.resolve_tunnel();
        if self.od_active() {
            self.restart_on_demand();
        } else {
            self.refresh();
        }
    }

    // MARK: Updates

    /// Fetch the clone this app was installed from and offer to pull + reinstall.
    fn check_for_updates(&self) {
        if self.ivars().busy.get() {
            return;
        }
        let repo_dir = defaults::string(defaults::REPO_DIR).unwrap_or_default();
        let updater = Arc::new(Updater::new(repo_dir, runner()));
        global_queue(DispatchQoS::UserInitiated).exec_async(move || {
            let status = updater.check();
            on_main(move |d| d.present(status, updater));
        });
    }

    fn present(&self, status: UpdateStatus, updater: Arc<Updater>) {
        match status {
            UpdateStatus::Failed(message) => {
                self.show_error("", &format!("Could not check for updates.\n{message}"));
            }
            UpdateStatus::UpToDate { revision } => {
                self.show_error_styled(
                    "",
                    &format!("WGBar is up to date ({revision})."),
                    NSAlertStyle::Informational,
                );
            }
            UpdateStatus::Available { commits } => {
                let mtm = self.mtm();
                let alert = NSAlert::new(mtm);
                alert.setMessageText(&ns("WGBar update available"));
                let count = commits.len();
                alert.setInformativeText(&ns(&format!(
                    "{count} new commit{}:\n\n{}\n\nUpdate pulls the latest source, rebuilds, and relaunches WGBar.",
                    if count == 1 { "" } else { "s" },
                    commits.join("\n")
                )));
                alert.addButtonWithTitle(&ns("Update"));
                alert.addButtonWithTitle(&ns("Later"));
                ui::activate(mtm);
                if alert.runModal() != NSAlertFirstButtonReturn {
                    return;
                }
                self.apply_update(updater);
            }
        }
    }

    fn apply_update(&self, updater: Arc<Updater>) {
        self.ivars().busy.set(true);
        self.update_icon();
        global_queue(DispatchQoS::UserInitiated).exec_async(move || {
            let failure = match updater.pull() {
                Some(error) => format!("git pull failed.\n{error}"),
                None => match spawn_install(&updater.repo_dir) {
                    Ok(()) => return,
                    Err(error) => format!("Could not start install.sh.\n{error}"),
                },
            };
            on_main(move |d| {
                d.ivars().busy.set(false);
                d.update_icon();
                d.show_error("", &failure);
            });
        });
    }

    fn show_error(&self, what: &str, message: &str) {
        self.show_error_styled(what, message, NSAlertStyle::Warning);
    }

    fn show_error_styled(&self, what: &str, message: &str, style: NSAlertStyle) {
        let title = if what.is_empty() {
            "WGBar".to_string()
        } else {
            format!("wg-quick {what} {} failed", self.tunnel())
        };
        ui::alert(self.mtm(), &title, message, style);
    }
}

/// Runs on od_queue. Remember the current DNS, then set the config's servers everywhere (as
/// wg-quick would).
fn apply_vpn_dns(tunnel: &str) {
    let Some(text) = read_conf(tunnel) else {
        return;
    };
    let servers = config_dns_servers(&text);
    if servers.is_empty() {
        return;
    }
    let guardian = DnsGuard::new(all_config_dns(), runner());
    defaults::set_dns_snapshot(&guardian.snapshot());
    let errors = guardian.apply(&servers, &config_dns_search(&text));
    on_main(move |d| {
        d.ivars().prefs_seen.set(network_prefs_modified());
        if !errors.is_empty() {
            d.show_error(
                "",
                &format!("Could not set VPN DNS:\n{}", errors.join("\n")),
            );
        }
    });
}

/// Runs on od_queue. Put back whatever DNS the services had before connect.
fn restore_dns() {
    let snapshot = defaults::dns_snapshot();
    let result = DnsGuard::new(all_config_dns(), runner()).repair(&snapshot);
    on_main(move |d| {
        d.ivars().prefs_seen.set(network_prefs_modified());
        if !result.errors.is_empty() {
            d.show_error(
                "",
                &format!("Could not restore DNS:\n{}", result.errors.join("\n")),
            );
        }
    });
}

/// install.sh kills and relaunches WGBar, so it must outlive this process and must not write
/// to a pipe we hold. Detach it with its output in a log file.
fn spawn_install(repo_dir: &str) -> std::io::Result<()> {
    let log = format!("{}/Library/Logs/WGBar-update.log", NSHomeDirectory());
    // Swift creates (truncates) the log and ignores failure; the output then is not captured.
    let (stdout, stderr) = match File::create(&log).and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    let inherited =
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let mut path = std::ffi::OsString::from(format!("{}:", brew_bin()));
    path.push(inherited);
    Command::new("/bin/bash")
        .arg("./install.sh")
        .current_dir(repo_dir)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        // Own process group, as Foundation's Process does: a signal to WGBar's group (for
        // example when install.sh stops the app) does not reach the installer.
        .process_group(0)
        .spawn()
        // The child is not waited for: it replaces this process.
        .map(drop)
}
