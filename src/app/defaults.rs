//! `NSUserDefaults` (domain `org.wgbar.WGBar`) with the same keys and value types as the
//! Swift app, so an existing install keeps its settings. `NSUserDefaults` is thread-safe, so
//! these helpers may be called from any queue.

use std::collections::BTreeMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUserDefaults};
use wgbar::dns_guard::DnsSnapshot;

/// String: path of a wg-quick to use instead of the search list.
pub const WG_QUICK: &str = "wgQuick";
/// String: the config folder chosen with "Config Folder…".
pub const CONF_DIR: &str = "confDir";
/// String: the selected tunnel name.
pub const TUNNEL: &str = "tunnel";
/// Bool: the first launch already registered the login item.
pub const DID_REGISTER_LOGIN_ITEM: &str = "didRegisterLoginItem";
/// Dictionary `{service: {"servers": [String], "search": [String]}}`: DNS to put back.
pub const DNS_SNAPSHOT: &str = "dnsSnapshot";
/// Bool: Connect on Demand is on.
pub const ON_DEMAND: &str = "onDemand";
/// Number (seconds, default 30): idle time before an on-demand tunnel disconnects.
pub const ON_DEMAND_IDLE: &str = "onDemandIdle";
/// String: the git clone WGBar was installed from (written by install.sh).
pub const REPO_DIR: &str = "repoDir";

fn standard() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

/// Swift `defaults.string(forKey:)` (numbers are converted to strings, like there).
pub fn string(key: &str) -> Option<String> {
    standard()
        .stringForKey(&NSString::from_str(key))
        .map(|s| s.to_string())
}

pub fn set_string(key: &str, value: &str) {
    let value = NSString::from_str(value);
    // SAFETY: NSString is a property-list type.
    unsafe { standard().setObject_forKey(Some(&value), &NSString::from_str(key)) };
}

pub fn bool(key: &str) -> bool {
    standard().boolForKey(&NSString::from_str(key))
}

pub fn set_bool(key: &str, value: bool) {
    standard().setBool_forKey(value, &NSString::from_str(key));
}

/// Swift `(defaults.object(forKey:) as? NSNumber)?.doubleValue`.
pub fn number(key: &str) -> Option<f64> {
    let object = standard().objectForKey(&NSString::from_str(key))?;
    object.downcast_ref::<NSNumber>().map(NSNumber::as_f64)
}

pub fn remove(key: &str) {
    standard().removeObjectForKey(&NSString::from_str(key));
}

/// Swift `defaults.dictionary(forKey: "dnsSnapshot") as? DNSSnapshot ?? [:]`: anything that is
/// not exactly a dictionary of dictionaries of string arrays reads as empty.
pub fn dns_snapshot() -> DnsSnapshot {
    standard()
        .dictionaryForKey(&NSString::from_str(DNS_SNAPSHOT))
        .and_then(|dict| snapshot_from(&dict))
        .unwrap_or_default()
}

fn snapshot_from(dict: &NSDictionary<NSString, AnyObject>) -> Option<DnsSnapshot> {
    let mut snapshot = DnsSnapshot::new();
    let (services, settings) = dict.to_vecs();
    for (service, setting) in services.iter().zip(settings) {
        let setting = setting.downcast_ref::<NSDictionary>()?;
        let mut fields = BTreeMap::new();
        let (names, lists) = setting.to_vecs();
        for (name, list) in names.iter().zip(lists) {
            let name = name.downcast_ref::<NSString>()?;
            let values = list
                .downcast_ref::<NSArray>()?
                .iter()
                .map(|v| v.downcast_ref::<NSString>().map(NSString::to_string))
                .collect::<Option<Vec<String>>>()?;
            fields.insert(name.to_string(), values);
        }
        snapshot.insert(service.to_string(), fields);
    }
    Some(snapshot)
}

/// Swift `defaults.set(snapshot, forKey: "dnsSnapshot")`: stored as nested property-list
/// dictionaries and arrays of strings.
pub fn set_dns_snapshot(snapshot: &DnsSnapshot) {
    let services: Vec<Retained<NSString>> =
        snapshot.keys().map(|s| NSString::from_str(s)).collect();
    let settings: Vec<Retained<NSDictionary<NSString, NSArray<NSString>>>> = snapshot
        .values()
        .map(|fields| {
            let names: Vec<Retained<NSString>> =
                fields.keys().map(|n| NSString::from_str(n)).collect();
            let lists: Vec<Retained<NSArray<NSString>>> = fields
                .values()
                .map(|values| {
                    let strings: Vec<Retained<NSString>> =
                        values.iter().map(|v| NSString::from_str(v)).collect();
                    NSArray::from_retained_slice(&strings)
                })
                .collect();
            NSDictionary::from_retained_objects(&refs(&names), &lists)
        })
        .collect();
    let dict = NSDictionary::from_retained_objects(&refs(&services), &settings);
    let value: &AnyObject = &dict;
    // SAFETY: the value is built only from NSDictionary, NSArray and NSString, all
    // property-list types.
    unsafe { standard().setObject_forKey(Some(value), &NSString::from_str(DNS_SNAPSHOT)) };
}

fn refs(strings: &[Retained<NSString>]) -> Vec<&NSString> {
    strings.iter().map(|s| &**s).collect()
}
