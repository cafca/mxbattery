//! CoreBluetooth backend — BAS + HID++ charging state (Tasks 11–12).
//!
//! Sequence:
//! 1. `centralManagerDidUpdateState` (poweredOn) → retrieve already-connected
//!    peripherals advertising services [180F, 180A, vendor] → connect each.
//! 2. `didConnectPeripheral` → discoverServices.
//! 3. `didDiscoverServices` → discoverCharacteristics for each service.
//! 4. `didDiscoverCharacteristicsForService` — once all three services have
//!    reported chars, read 0x2A24 + 0x2A50 to seed the DeviceProbe.
//! 5. `didUpdateValueForCharacteristic`:
//!    - 2A24 / 2A50 → fill probe; when both set, run filter.  If pass: emit
//!      Connected, setNotify for 2A19 + vendor, read 2A19 for initial value,
//!      and send HID++ GetFeature(0x1000) to resolve the battery-status idx.
//!    - 2A19 → emit Percent.
//!    - vendor → dispatch via `handle_vendor_bytes`; on feature-resolve,
//!      send GetBatteryLevelStatus; on charging event, emit Charging.
//! 6. `didDisconnectPeripheral` → emit Disconnected, keep manager alive.
//!
//! CoreBluetooth fires callbacks on the main dispatch queue (queue = nil).
//! All Objective-C objects must therefore be accessed only from that queue;
//! the delegate stores them in a `Mutex<PeriphState>` to satisfy `Send+Sync`
//! across the broadcast channel boundary without actually moving ObjC objects
//! off the main thread.

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_core_bluetooth::{
    CBCentralManager, CBCentralManagerDelegate, CBCharacteristic, CBCharacteristicWriteType,
    CBManagerState, CBPeripheral, CBPeripheralDelegate, CBService, CBUUID,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSData, NSObject, NSObjectProtocol, NSString};
use tokio::sync::broadcast;

use crate::battery::{BatteryEvent, VendorOutcome};
use crate::config::DeviceFilter;
use crate::device_filter::{matches, DeviceProbe};
use crate::hidpp::FrameShape;

// ---------------------------------------------------------------------------
// UUID constants (canonical lowercase without dashes for 16-bit short forms)
// ---------------------------------------------------------------------------

pub const UUID_180F: &str = "180F"; // Battery Service
pub const UUID_180A: &str = "180A"; // Device Information Service
pub const UUID_VENDOR_SVC: &str = "00010000-0000-1000-8000-011F2000046D";
pub const UUID_2A19: &str = "2A19"; // Battery Level
pub const UUID_2A24: &str = "2A24"; // Model Number String
pub const UUID_2A50: &str = "2A50"; // PnP ID
pub const UUID_VENDOR_CHAR: &str = "00010001-0000-1000-8000-011F2000046D";

/// The full set of service UUIDs we enumerate characteristics for.
/// Used in `didDiscoverServices` to count how many are actually present.
const KNOWN_SERVICE_UUIDS: [&str; 3] = [UUID_180F, UUID_180A, UUID_VENDOR_SVC];

// ---------------------------------------------------------------------------
// Helper: build a CBUUID from a string literal
// ---------------------------------------------------------------------------
fn cbuuid(s: &str) -> Retained<CBUUID> {
    unsafe { CBUUID::UUIDWithString(&NSString::from_str(s)) }
}

/// Normalise a CoreBluetooth UUID string for comparison.
/// CoreBluetooth returns short UUIDs in UPPER-CASE ("2A19") and long UUIDs
/// with dashes in their canonical form.  We compare case-insensitively.
fn uuid_matches(uuid: &CBUUID, candidate: &str) -> bool {
    let s = unsafe { uuid.UUIDString() };
    let rust = s.to_string();
    rust.eq_ignore_ascii_case(candidate)
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Advance the swid counter, wrapping around 1..=15 (skipping 0).
fn next_swid(counter: &mut u8) -> u8 {
    let swid = *counter;
    *counter = if swid >= 15 { 1 } else { swid + 1 };
    swid
}

/// Write a payload to the vendor characteristic with WriteWithResponse.
/// # Safety
/// `char_ptr` must be a valid `CBCharacteristic` pointer valid for the
/// current main-queue callback.  Only call from the main dispatch queue.
unsafe fn write_vendor(
    peripheral: &CBPeripheral,
    char_ptr: *mut CBCharacteristic,
    payload: Vec<u8>,
) {
    if char_ptr.is_null() {
        return;
    }
    let c = &*char_ptr;
    let data = NSData::with_bytes(&payload);
    peripheral.writeValue_forCharacteristic_type(
        &data,
        c,
        CBCharacteristicWriteType::CBCharacteristicWriteWithResponse,
    );
}

// ---------------------------------------------------------------------------
// Shape-probe timeout via dispatch2 (GCD main queue)
// ---------------------------------------------------------------------------

/// Advance to the next frame shape.  Returns `true` if a new shape is
/// available; `false` if all three shapes have been exhausted.
fn try_next_shape(st: &mut PeriphState) -> bool {
    st.pending_resolve_swid = None;
    st.feature_index_1000 = None;
    match st.shape {
        FrameShape::NoDevIdx18 => {
            st.shape = FrameShape::WithDevIdx19;
            true
        }
        FrameShape::WithDevIdx19 => {
            st.shape = FrameShape::WithReportId20;
            true
        }
        FrameShape::WithReportId20 => {
            tracing::warn!(
                "HID++ frame-shape probe exhausted — cannot determine battery-status feature"
            );
            false
        }
    }
}

// NOTE: A 2-second response timeout for shape probing is not yet implemented.
// The shape is only advanced on a CBATTErrorDomain Code=13 write error.
// To add the timeout, we would need to schedule a callback on the GCD main
// queue after 2 seconds (e.g. via `dispatch_after_f`).  The obstacle is that
// `dispatch2 = "0.3"` requires `objc2 >= 0.6`, which conflicts with this
// project's `objc2 = "0.5"`.  When the project upgrades to `objc2 0.6`,
// replace the error-only path below with a proper `DispatchQueue::main().after`
// call and set `pending_resolve_swid = None` in the timeout if it has not been
// cleared already. (DONE_WITH_CONCERNS: 2-second timeout not implemented)

// ---------------------------------------------------------------------------
// Per-peripheral state
// ---------------------------------------------------------------------------

/// All mutable state for a single connected peripheral.
/// Stored behind `Mutex` inside the ivar so `Delegate` can be `Send+Sync`.
struct PeriphState {
    /// Stable string identifier (NSUUID → string) used as HashMap key and
    /// sent in log messages.  Does NOT hold the Objective-C NSUUID object so
    /// the struct is `Send`.
    #[allow(dead_code)]
    id: String,
    /// Peripheral name for the Connected event.
    name: String,
    /// How many of the expected services were actually advertised by this
    /// peripheral (set once in `didDiscoverServices`; 0 until then).
    expected_services: usize,
    /// How many of the expected services have completed char-discovery.
    services_done: usize,
    /// Whether we've passed the filter and emitted Connected.
    connected_emitted: bool,
    /// Retained handles to interesting characteristics.
    /// SAFETY NOTE: these are only accessed on the main dispatch queue
    /// (the queue on which CoreBluetooth fires all delegate callbacks).
    /// Because the Mutex is only locked from that same queue, no data race
    /// occurs even though `Retained<CBCharacteristic>` is `!Send`.
    /// We box them behind a raw pointer to satisfy the type system.
    char_battery_level: Option<*mut CBCharacteristic>,
    char_model_number: Option<*mut CBCharacteristic>,
    char_pnp_id: Option<*mut CBCharacteristic>,
    char_vendor: Option<*mut CBCharacteristic>,
    /// Device filter probe being built up.
    probe: DeviceProbe,
    // --- HID++ state ---
    shape: FrameShape,
    feature_index_1000: Option<u8>,
    swid_counter: u8,
    pending_resolve_swid: Option<u8>,
}

// SAFETY: raw pointers are only accessed on the main queue (single-threaded
// access pattern enforced by CoreBluetooth). The `Mutex` serialises access
// across the Rust type system.
unsafe impl Send for PeriphState {}
unsafe impl Sync for PeriphState {}

impl PeriphState {
    fn new(id: String, name: String, has_vendor_svc: bool) -> Self {
        Self {
            probe: DeviceProbe {
                peripheral_identifier: id.clone(),
                model_number: None,
                pnp_vid: None,
                pnp_pid: None,
                has_logitech_vendor_service: has_vendor_svc,
            },
            id,
            name,
            expected_services: 0,
            services_done: 0,
            connected_emitted: false,
            char_battery_level: None,
            char_model_number: None,
            char_pnp_id: None,
            char_vendor: None,
            shape: FrameShape::NoDevIdx18,
            feature_index_1000: None,
            swid_counter: 1,
            pending_resolve_swid: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared inner state (lives behind Arc, shared between `Delegate` ivar and
// `CbBackend` so that `subscribe()` can get the sender).
// ---------------------------------------------------------------------------

struct Inner {
    tx: broadcast::Sender<BatteryEvent>,
    filter: DeviceFilter,
    /// Map from peripheral UUID string → per-peripheral state.
    peripherals: HashMap<String, PeriphState>,
    /// Strong references to CBPeripheral objects we want to connect to. CoreBluetooth
    /// requires the caller to hold a retain on the peripheral until didConnect fires;
    /// without this, the peripheral is released between `connectPeripheral_options` and
    /// the callback, and the callback never runs.
    /// Stored as raw pointers (with retain count incremented via `Retained::into_raw`)
    /// because `Retained<CBPeripheral>` is `!Send` but `Inner` must be `Send`.
    /// Released by `Inner::Drop`.
    /// SAFETY: only accessed on the main dispatch queue.
    retained_peripherals: HashMap<String, *mut CBPeripheral>,
}

impl Inner {
    fn new(filter: DeviceFilter) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            filter,
            peripherals: HashMap::new(),
            retained_peripherals: HashMap::new(),
        }
    }

    fn send(&self, ev: BatteryEvent) {
        // try_send: if no receivers or buffer full, silently drop.
        let _ = self.tx.send(ev);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        for (_, raw) in self.retained_peripherals.drain() {
            // SAFETY: raw was produced by Retained::into_raw; from_raw rebuilds
            // the Retained which then releases the +1 on drop.
            unsafe {
                let _ = Retained::<CBPeripheral>::from_raw(raw);
            }
        }
    }
}

// SAFETY: the raw `*mut CBPeripheral` pointers are only ever dereferenced on the
// CoreBluetooth main dispatch queue (the same queue that locks the Mutex), so
// there is no cross-thread access to the Objective-C objects despite the type
// system's `!Send + !Sync` inference for raw pointers.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

// ---------------------------------------------------------------------------
// Objective-C delegate class
// ---------------------------------------------------------------------------

/// Ivar type: `Arc<Mutex<Inner>>` — cloneable, `Send+Sync`.
type DelegateIvars = Arc<Mutex<Inner>>;

declare_class!(
    /// The combined CBCentralManagerDelegate + CBPeripheralDelegate.
    struct Delegate;

    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "MXBatteryDelegate";
    }

    impl DeclaredClass for Delegate {
        type Ivars = DelegateIvars;
    }

    unsafe impl NSObjectProtocol for Delegate {}

    // -----------------------------------------------------------------------
    // CBCentralManagerDelegate
    // -----------------------------------------------------------------------
    unsafe impl CBCentralManagerDelegate for Delegate {
        /// Called whenever the BT radio state changes.
        #[method(centralManagerDidUpdateState:)]
        unsafe fn centralManagerDidUpdateState(&self, central: &CBCentralManager) {
            if central.state() != CBManagerState::PoweredOn {
                tracing::debug!("BT state {:?} — waiting", central.state().0);
                return;
            }
            tracing::debug!("BT powered-on; retrieving connected peripherals");

            // Retrieve peripherals already connected that expose at least one
            // of the three services we care about.
            let services = NSArray::from_id_slice(&[
                cbuuid(UUID_180F),
                cbuuid(UUID_180A),
                cbuuid(UUID_VENDOR_SVC),
            ]);
            let found =
                central.retrieveConnectedPeripheralsWithServices(&services);

            tracing::debug!("{} connected peripheral(s) found", found.len());
            for p in found.iter() {
                let pid = p.identifier().UUIDString().to_string();
                tracing::debug!("connecting peripheral {}", pid);

                // Retain the peripheral so it survives until didConnect fires.
                // CoreBluetooth does not take a strong ref on connectPeripheral_options;
                // if we drop the only ref (held by `found`) at end of this method,
                // the callback never fires.
                // SAFETY: p is a non-null reference to a live ObjC object.
                let retained: Retained<CBPeripheral> = unsafe {
                    Retained::retain(p as *const CBPeripheral as *mut CBPeripheral)
                        .expect("retain returned nil for live peripheral")
                };
                let raw: *mut CBPeripheral = Retained::into_raw(retained);
                {
                    let mut inner = self.ivars().lock().unwrap();
                    if let Some(prev) = inner.retained_peripherals.insert(pid.clone(), raw) {
                        // Drop any previous retain.
                        unsafe {
                            let _ = Retained::<CBPeripheral>::from_raw(prev);
                        }
                    }
                }

                p.setDelegate(Some(ProtocolObject::from_ref(self)));
                central.connectPeripheral_options(p, None);
            }
        }

        #[method(centralManager:didConnectPeripheral:)]
        unsafe fn centralManager_didConnectPeripheral(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            let pname = peripheral
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "Unknown".to_string());
            tracing::debug!("connected: {} ({})", pname, pid);

            // Determine if the peripheral advertises the vendor service.
            // We won't know until we discover services, so start with false;
            // it will be patched in didDiscoverServices.
            {
                // Always reset state on (re-)connect so that stale characteristic
                // pointers and flags from a previous connection are discarded even
                // if CoreBluetooth fires didConnectPeripheral before didDisconnect.
                let mut inner = self.ivars().lock().unwrap();
                inner.peripherals.insert(pid.clone(), PeriphState::new(pid.clone(), pname, false));
            }

            // Set delegate again (defensive) and discover services.
            peripheral.setDelegate(Some(ProtocolObject::from_ref(self)));
            let services = NSArray::from_id_slice(&[
                cbuuid(UUID_180F),
                cbuuid(UUID_180A),
                cbuuid(UUID_VENDOR_SVC),
            ]);
            peripheral.discoverServices(Some(&services));
        }

        #[method(centralManager:didFailToConnectPeripheral:error:)]
        unsafe fn centralManager_didFailToConnectPeripheral_error(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            tracing::warn!("failed to connect peripheral {}: {:?}", pid, error.map(|e| e.to_string()));
        }

        #[method(centralManager:didDisconnectPeripheral:error:)]
        unsafe fn centralManager_didDisconnectPeripheral_error(
            &self,
            _central: &CBCentralManager,
            peripheral: &CBPeripheral,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            tracing::debug!("disconnected: {} err={:?}", pid, error.map(|e| e.to_string()));
            let mut inner = self.ivars().lock().unwrap();
            if let Some(st) = inner.peripherals.remove(&pid) {
                if st.connected_emitted {
                    inner.send(BatteryEvent::Disconnected);
                }
            }
            // CoreBluetooth will re-fire didConnectPeripheral when the device
            // comes back; we clean state and let the reconnect re-run steps 2–5.
        }
    }

    // -----------------------------------------------------------------------
    // CBPeripheralDelegate
    // -----------------------------------------------------------------------
    unsafe impl CBPeripheralDelegate for Delegate {
        #[method(peripheral:didDiscoverServices:)]
        unsafe fn peripheral_didDiscoverServices(
            &self,
            peripheral: &CBPeripheral,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            if let Some(e) = error {
                tracing::warn!("didDiscoverServices error for {}: {}", pid, e);
                return;
            }

            let services = match peripheral.services() {
                Some(s) => s,
                None => return,
            };

            // Check which of the expected services are actually present and
            // how many of them were returned, so char-discovery completion can
            // be detected without hard-coding a count of 3.
            let has_vendor = services
                .iter()
                .any(|s| uuid_matches(&s.UUID(), UUID_VENDOR_SVC));

            let expected_count = services
                .iter()
                .filter(|s| {
                    let u = s.UUID();
                    KNOWN_SERVICE_UUIDS.iter().any(|&k| uuid_matches(&u, k))
                })
                .count();

            {
                let mut inner = self.ivars().lock().unwrap();
                if let Some(st) = inner.peripherals.get_mut(&pid) {
                    st.probe.has_logitech_vendor_service = has_vendor;
                    st.expected_services = expected_count;
                }
            }

            for s in services.iter() {
                peripheral.discoverCharacteristics_forService(None, s);
            }
        }

        #[method(peripheral:didDiscoverCharacteristicsForService:error:)]
        unsafe fn peripheral_didDiscoverCharacteristicsForService_error(
            &self,
            peripheral: &CBPeripheral,
            service: &CBService,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            if let Some(e) = error {
                tracing::warn!("didDiscoverCharacteristics error for {}: {}", pid, e);
                return;
            }

            let chars = match service.characteristics() {
                Some(c) => c,
                None => return,
            };

            let svc_uuid = service.UUID();
            let is_180f = uuid_matches(&svc_uuid, UUID_180F);
            let is_180a = uuid_matches(&svc_uuid, UUID_180A);
            let is_vendor = uuid_matches(&svc_uuid, UUID_VENDOR_SVC);

            // Store references to the interesting characteristics.
            // raw pointers are used only on the main queue (enforced by CB).
            let mut inner = self.ivars().lock().unwrap();
            let st = match inner.peripherals.get_mut(&pid) {
                Some(s) => s,
                None => return,
            };

            for c in chars.iter() {
                // `c` is `&CBCharacteristic` (not `&Retained`); cast to
                // *mut so we can store it as an untyped raw pointer.
                // The NSArray retains the characteristic, so the pointer
                // remains valid as long as `chars` (and its backing
                // peripheral) is alive — which holds until disconnect.
                let cuuid = c.UUID();
                let raw = c as *const CBCharacteristic as *mut CBCharacteristic;
                if is_180f && uuid_matches(&cuuid, UUID_2A19) {
                    st.char_battery_level = Some(raw);
                } else if is_180a && uuid_matches(&cuuid, UUID_2A24) {
                    st.char_model_number = Some(raw);
                } else if is_180a && uuid_matches(&cuuid, UUID_2A50) {
                    st.char_pnp_id = Some(raw);
                } else if is_vendor && uuid_matches(&cuuid, UUID_VENDOR_CHAR) {
                    st.char_vendor = Some(raw);
                }
            }

            // Mark this service as done.
            if is_180f || is_180a || is_vendor {
                st.services_done += 1;
            }

            // Once all expected services have reported their chars (and at least
            // one expected service exists), kick off reads for device-info chars.
            if st.expected_services > 0 && st.services_done >= st.expected_services {
                let model_ptr = st.char_model_number;
                let pnp_ptr = st.char_pnp_id;
                drop(inner);

                if let Some(ptr) = model_ptr {
                    let c = &*ptr;
                    peripheral.readValueForCharacteristic(c);
                }
                if let Some(ptr) = pnp_ptr {
                    let c = &*ptr;
                    peripheral.readValueForCharacteristic(c);
                }
            }
        }

        #[method(peripheral:didUpdateValueForCharacteristic:error:)]
        unsafe fn peripheral_didUpdateValueForCharacteristic_error(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let pid = peripheral.identifier().UUIDString().to_string();
            if let Some(e) = error {
                tracing::warn!("didUpdateValue error for {}: {}", pid, e);
                return;
            }

            let cuuid = characteristic.UUID();

            if uuid_matches(&cuuid, UUID_2A19) {
                // ---- Battery Level ----
                let percent = match characteristic.value() {
                    Some(data) if !data.is_empty() => {
                        let raw = data.bytes()[0];
                        raw.min(100)
                    }
                    _ => return,
                };
                tracing::debug!("2A19 battery level: {}% (peripheral {})", percent, pid);
                // Only emit Percent if we passed the device filter and already
                // sent Connected.  A stale OS subscription can deliver a 2A19
                // notification even for a peripheral we rejected, so guard here.
                let inner = self.ivars().lock().unwrap();
                if inner.peripherals.get(&pid).is_some_and(|st| st.connected_emitted) {
                    inner.send(BatteryEvent::Percent(percent));
                }
                return;
            }

            if uuid_matches(&cuuid, UUID_VENDOR_CHAR) {
                let payload = match characteristic.value() {
                    Some(d) if !d.is_empty() => d.bytes().to_vec(),
                    _ => return,
                };
                tracing::debug!(
                    "vendor char update for {}: {:02x?}",
                    pid, &payload
                );

                let outcome = {
                    let mut inner = self.ivars().lock().unwrap();
                    let st = match inner.peripherals.get_mut(&pid) {
                        Some(s) => s,
                        None => return,
                    };
                    let expected = st.pending_resolve_swid.unwrap_or(0);
                    crate::battery::handle_vendor_bytes(
                        st.shape,
                        &payload,
                        expected,
                        &mut st.feature_index_1000,
                    )
                };

                match outcome {
                    VendorOutcome::ResolvedBatteryStatusFeature(idx) => {
                        tracing::debug!(
                            "resolved battery-status feature index 0x{:02x} for {}",
                            idx, pid
                        );
                        // Clear the pending resolve and seed initial charging state.
                        let (shape, vendor_ptr) = {
                            let mut inner = self.ivars().lock().unwrap();
                            let st = match inner.peripherals.get_mut(&pid) {
                                Some(s) => s,
                                None => return,
                            };
                            st.pending_resolve_swid = None;
                            let swid = next_swid(&mut st.swid_counter);
                            let frame = crate::hidpp::encode_get_battery_level_status(
                                st.shape, idx, swid,
                            );
                            (frame, st.char_vendor.unwrap_or(std::ptr::null_mut()))
                        };
                        write_vendor(peripheral, vendor_ptr, shape);
                    }
                    VendorOutcome::Charging(s) => {
                        let inner = self.ivars().lock().unwrap();
                        if inner.peripherals.get(&pid).is_some_and(|st| st.connected_emitted) {
                            inner.send(BatteryEvent::Charging(s));
                        }
                    }
                    VendorOutcome::Ignored => {}
                }
                return;
            }

            // ---- Device information: 2A24 or 2A50 ----
            let is_2a24 = uuid_matches(&cuuid, UUID_2A24);
            let is_2a50 = uuid_matches(&cuuid, UUID_2A50);

            if !is_2a24 && !is_2a50 {
                return; // unknown characteristic, ignore
            }

            let data = match characteristic.value() {
                Some(d) if !d.is_empty() => d,
                _ => return,
            };

            {
                let mut inner = self.ivars().lock().unwrap();
                // Clone filter before taking a mutable borrow on peripherals.
                let filter_clone = inner.filter.clone();

                let st = match inner.peripherals.get_mut(&pid) {
                    Some(s) => s,
                    None => return,
                };

                if is_2a24 {
                    let model = std::str::from_utf8(data.bytes())
                        .map(|s| s.trim_end_matches('\0').to_string())
                        .unwrap_or_default();
                    tracing::debug!("2A24 model number: {:?} ({})", model, pid);
                    st.probe.model_number = Some(model);
                } else {
                    // 0x2A50 PnP ID: 7 bytes
                    // [vendor_source(1), vid_lo, vid_hi, pid_lo, pid_hi, ver_lo, ver_hi]
                    let b = data.bytes();
                    if b.len() >= 5 {
                        let vid = u16::from_le_bytes([b[1], b[2]]);
                        let pid_val = u16::from_le_bytes([b[3], b[4]]);
                        tracing::debug!(
                            "2A50 PnP VID=0x{:04X} PID=0x{:04X} ({})",
                            vid, pid_val, pid
                        );
                        st.probe.pnp_vid = Some(vid);
                        st.probe.pnp_pid = Some(pid_val);
                    } else {
                        tracing::warn!("2A50 too short ({} bytes), skipping", b.len());
                        return;
                    }
                }

                // Only run filter once both device-info reads have landed.
                let model_ready = st.probe.model_number.is_some();
                let pnp_ready =
                    st.probe.pnp_vid.is_some() && st.probe.pnp_pid.is_some();
                if !model_ready || !pnp_ready {
                    return;
                }

                // Already handled?
                if st.connected_emitted {
                    return;
                }

                // Collect values before dropping the mutable borrow.
                let probe_clone = st.probe.clone();
                let bat_ptr = st.char_battery_level;
                let vendor_ptr = st.char_vendor;
                let name = st.name.clone();

                let pass = matches(&filter_clone, &probe_clone);
                if !pass {
                    tracing::info!(
                        "peripheral {} did not pass filter; skipping subscription",
                        pid
                    );
                    // TODO (Task 12): store manager ptr in Inner so we can
                    // call cancelPeripheralConnection here.
                    return;
                }

                st.connected_emitted = true;
                drop(inner);

                // Emit Connected.
                {
                    let inner = self.ivars().lock().unwrap();
                    inner.send(BatteryEvent::Connected { name });
                }

                // Subscribe to notifications and read initial battery value.
                if let Some(ptr) = bat_ptr {
                    let c = &*ptr;
                    peripheral.setNotifyValue_forCharacteristic(true, c);
                    peripheral.readValueForCharacteristic(c);
                }
                if let Some(ptr) = vendor_ptr {
                    let c = &*ptr;
                    peripheral.setNotifyValue_forCharacteristic(true, c);

                    // Issue a GetFeature(0x1000) request to resolve the
                    // battery-status feature index for this peripheral.
                    let (swid, frame) = {
                        let mut inner = self.ivars().lock().unwrap();
                        if let Some(st) = inner.peripherals.get_mut(&pid) {
                            let swid = next_swid(&mut st.swid_counter);
                            st.pending_resolve_swid = Some(swid);
                            let frame = crate::hidpp::encode_get_feature(
                                st.shape,
                                crate::hidpp::FEATURE_BATTERY_STATUS,
                                swid,
                            );
                            (swid, frame)
                        } else {
                            return;
                        }
                    };
                    tracing::debug!(
                        "sending GetFeature(0x1000) swid={} to {}",
                        swid, pid
                    );
                    write_vendor(peripheral, ptr, frame);
                    // NOTE: 2-second response timeout not implemented here; see
                    // DONE_WITH_CONCERNS comment above try_next_shape.
                    // Shape advancement happens only on CBATTErrorDomain Code=13.
                }
                return;
            }
        }

        /// Called after a WriteWithResponse write completes (or fails).
        /// On `CBATTErrorDomain Code=13` (invalid value length) for the vendor
        /// char, advance the frame shape and retry the GetFeature probe.
        #[method(peripheral:didWriteValueForCharacteristic:error:)]
        unsafe fn peripheral_didWriteValueForCharacteristic_error(
            &self,
            peripheral: &CBPeripheral,
            characteristic: &CBCharacteristic,
            error: Option<&objc2_foundation::NSError>,
        ) {
            let cuuid = characteristic.UUID();
            if !uuid_matches(&cuuid, UUID_VENDOR_CHAR) {
                return;
            }
            let error = match error {
                Some(e) => e,
                None => return, // write succeeded
            };

            let pid = peripheral.identifier().UUIDString().to_string();
            let code = error.code();
            tracing::debug!(
                "vendor char write error for {} code={}: {}",
                pid, code, error
            );

            // CBATTErrorDomain Code=13 == kCBATTErrorInvalidAttributeLength
            if code != 13 {
                return;
            }

            let (advanced, frame, vendor_ptr) = {
                let mut inner = self.ivars().lock().unwrap();
                let st = match inner.peripherals.get_mut(&pid) {
                    Some(s) => s,
                    None => return,
                };
                if !try_next_shape(st) {
                    return; // all shapes exhausted
                }
                let swid = next_swid(&mut st.swid_counter);
                st.pending_resolve_swid = Some(swid);
                let frame = crate::hidpp::encode_get_feature(
                    st.shape,
                    crate::hidpp::FEATURE_BATTERY_STATUS,
                    swid,
                );
                (true, frame, st.char_vendor.unwrap_or(std::ptr::null_mut()))
            };

            if advanced {
                write_vendor(peripheral, vendor_ptr, frame);
                // NOTE: 2-second timeout not re-scheduled after error-driven shape advance.
                // See DONE_WITH_CONCERNS comment near try_next_shape.
            }
        }
    }
);

impl Delegate {
    fn new(inner: Arc<Mutex<Inner>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(inner);
        unsafe { msg_send_id![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Public backend
// ---------------------------------------------------------------------------

/// CoreBluetooth BAS-path battery backend.
///
/// Must be created on the AppKit main thread.  All CoreBluetooth callbacks
/// fire on the main dispatch queue (because we pass `queue: nil`), so the
/// delegate's Mutex is always contended from a single OS thread and will
/// never actually block.
///
/// # Safety of `Send + Sync`
///
/// The `_manager` and `_delegate` raw pointers point to Objective-C objects
/// whose reference counts are manually tracked via `retain`/`release`.
/// We must not access these objects from any thread other than the main
/// dispatch queue — the only use of these fields is keeping the objects alive
/// (i.e. not-zero retain count) until `CbBackend` is dropped, which must
/// also happen on the main thread.
pub struct CbBackend {
    inner: Arc<Mutex<Inner>>,
    // Keep the manager and delegate alive for the lifetime of the backend.
    // Stored as raw pointers to avoid the `!Send`/`!Sync` of `Retained<T>`.
    // SAFETY: only accessed (for drop) on the main thread.
    _manager: *mut CBCentralManager,
    _delegate: *mut Delegate,
}

// SAFETY: the raw pointers are only accessed for their retain-count bookkeeping
// (drop) and that occurs on the main thread.  The `inner: Arc<Mutex<Inner>>`
// component is genuinely `Send + Sync`.
unsafe impl Send for CbBackend {}
unsafe impl Sync for CbBackend {}

impl Drop for CbBackend {
    fn drop(&mut self) {
        // Release the retained ObjC objects.
        // SAFETY: must be called on the main thread; see struct doc.
        unsafe {
            // Nil the manager's delegate FIRST so CoreBluetooth cannot fire
            // callbacks against the delegate after it has been released.
            if !self._manager.is_null() {
                let null_delegate: *const objc2::runtime::AnyObject = std::ptr::null();
                let _: () = objc2::msg_send![&*self._manager, setDelegate: null_delegate];
            }
            // Now release the manager, then the delegate.
            if !self._manager.is_null() {
                let _ = objc2::rc::Retained::<CBCentralManager>::from_raw(self._manager);
            }
            if !self._delegate.is_null() {
                let _ = objc2::rc::Retained::<Delegate>::from_raw(self._delegate);
            }
        }
    }
}

impl CbBackend {
    /// Initialise the backend.  Must be called on the main thread.
    pub fn start(filter: DeviceFilter, _mtm: MainThreadMarker) -> Self {
        let inner = Arc::new(Mutex::new(Inner::new(filter)));
        let delegate = Delegate::new(Arc::clone(&inner));

        // CBCentralManager::initWithDelegate:queue: is skipped in the
        // generated bindings (see translation-config.toml) — use msg_send_id!.
        let manager: Retained<CBCentralManager> = unsafe {
            let delegate_proto: &ProtocolObject<dyn CBCentralManagerDelegate> =
                ProtocolObject::from_ref(&*delegate);
            msg_send_id![
                CBCentralManager::alloc(),
                initWithDelegate: delegate_proto,
                queue: std::ptr::null::<objc2::runtime::AnyObject>()
            ]
        };

        // Leak the Retained values into raw pointers; Drop impl will release them.
        let mgr_ptr = Retained::into_raw(manager);
        let del_ptr = Retained::into_raw(delegate);

        CbBackend {
            inner,
            _manager: mgr_ptr,
            _delegate: del_ptr,
        }
    }
}

impl crate::battery::BatteryBackend for CbBackend {
    fn subscribe(&self) -> broadcast::Receiver<BatteryEvent> {
        self.inner.lock().unwrap().tx.subscribe()
    }
}
