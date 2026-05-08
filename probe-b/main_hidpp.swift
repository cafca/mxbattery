// HID++ 2.0 over Logitech vendor GATT characteristic (probe-c).
//
// Connects to the MX Master 3 Mac via CoreBluetooth, subscribes to the vendor
// notify characteristic and 0x2A19, sends:
//   - Root.getFeature(0x1004) to resolve UnifiedBattery feature index.
//   - <unifiedBattery>.getStatus to read battery percent / charging status.
// Then enters a passive listening window so the human can unplug/replug the
// charging cable while we capture spontaneous HID++ events.
//
// Run via the .app bundle to inherit Bluetooth TCC. Stdout is redirected to a
// temp file by the wrapper.

import Foundation
import CoreBluetooth

if let path = ProcessInfo.processInfo.environment["LOGI_MX_OUT"] {
    freopen(path, "w", stdout); setbuf(stdout, nil)
}
if let path = ProcessInfo.processInfo.environment["LOGI_MX_ERR"] {
    freopen(path, "w", stderr); setbuf(stderr, nil)
}

struct Args {
    var name: String = "MX Master 3 Mac"
    var listenSeconds: Double = 300.0
    var totalTimeout: Double = 340.0
    var sendReportID: Bool = false  // try without leading 0x11 first
}
func parseArgs() -> Args {
    var a = Args()
    var it = CommandLine.arguments.dropFirst().makeIterator()
    while let arg = it.next() {
        switch arg {
        case "--name": if let s = it.next() { a.name = s }
        case "--listen": if let s = it.next(), let v = Double(s) { a.listenSeconds = v }
        case "--timeout": if let s = it.next(), let v = Double(s) { a.totalTimeout = v }
        case "--with-report-id": a.sendReportID = true
        default: fputs("unknown arg: \(arg)\n", stderr); exit(64)
        }
    }
    return a
}

let batteryServiceUUID = CBUUID(string: "180F")
let batteryLevelUUID   = CBUUID(string: "2A19")
let vendorServiceUUID  = CBUUID(string: "00010000-0000-1000-8000-011F2000046D")
let vendorCharUUID     = CBUUID(string: "00010001-0000-1000-8000-011F2000046D")
let deviceInfoServiceUUID = CBUUID(string: "180A")

func hex(_ data: Data) -> String {
    return data.map { String(format: "%02x", $0) }.joined(separator: " ")
}
func hex(_ bytes: [UInt8]) -> String {
    return bytes.map { String(format: "%02x", $0) }.joined(separator: " ")
}
func iso(_ d: Date) -> String {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f.string(from: d)
}

// HID++ 2.0 framing
let HIDPP_LONG: UInt8  = 0x11
let HIDPP_SHORT: UInt8 = 0x10
let DEV_INDEX_DIRECT: UInt8 = 0xFF
let SOFTWARE_ID: UInt8 = 0x08
let ROOT_FEATURE_INDEX: UInt8 = 0x00
let ROOT_FN_GET_FEATURE: UInt8 = 0x00
let FEATURE_UNIFIED_BATTERY: UInt16 = 0x1004
let FEATURE_BATTERY_STATUS:  UInt16 = 0x1000

// Build a HID++ long payload (19 bytes, no report ID prefix), suitable for
// writing to the vendor GATT characteristic. Set `withReportID` true to
// prepend the 0x11 report ID byte (20 bytes total).
func longPayload(devIdx: UInt8, featIdx: UInt8, funcID: UInt8, swid: UInt8, params: [UInt8], withReportID: Bool, forceLen: Int = 0, omitDevIdx: Bool = false) -> Data {
    let defaultLen = withReportID ? 20 : 19
    let total = forceLen > 0 ? forceLen : defaultLen
    var f = [UInt8](repeating: 0, count: total)
    var off = 0
    if withReportID && total >= 1 { f[0] = HIDPP_LONG; off = 1 }
    if !omitDevIdx {
        if off < total { f[off] = devIdx }
        off += 1
    }
    if off < total     { f[off]     = featIdx }
    if off + 1 < total { f[off + 1] = (funcID << 4) | (swid & 0x0F) }
    let pBase = off + 2
    for (i, b) in params.enumerated() where (pBase + i) < total {
        f[pBase + i] = b
    }
    return Data(f)
}

// Decode a HID++ frame. Accepts either 19-byte (no report id) or 20-byte (with).
struct HidppFrame {
    let raw: Data
    let hadReportID: Bool
    let devIdx: UInt8
    let featIdx: UInt8
    let fn: UInt8
    let swid: UInt8
    let params: Data  // bytes after the header
    var isError10: Bool { featIdx == 0x8F } // HID++ 1.0 ERROR_MSG flag
    var isError20: Bool {
        // HID++ 2.0 ERROR: featureIdx == 0xFF in some encodings, but normally
        // recognized by the device responding with FF and a function 0xF.
        return featIdx == 0xFF
    }
    static func parse(_ d: Data, omitDevIdx: Bool = false) -> HidppFrame? {
        // Accept 19 or 20 byte. If 20 and first is 0x10/0x11, treat as
        // long-form with report id.
        if d.count == 20 && (d[0] == HIDPP_LONG || d[0] == HIDPP_SHORT) {
            return HidppFrame(raw: d, hadReportID: true,
                              devIdx: d[1], featIdx: d[2],
                              fn: d[3] >> 4, swid: d[3] & 0x0F,
                              params: d.subdata(in: 4..<d.count))
        }
        if omitDevIdx && d.count >= 3 {
            return HidppFrame(raw: d, hadReportID: false,
                              devIdx: 0xFF,
                              featIdx: d[0],
                              fn: d[1] >> 4, swid: d[1] & 0x0F,
                              params: d.subdata(in: 2..<d.count))
        }
        if d.count >= 4 && d.count <= 19 {
            return HidppFrame(raw: d, hadReportID: false,
                              devIdx: d[0], featIdx: d[1],
                              fn: d[2] >> 4, swid: d[2] & 0x0F,
                              params: d.subdata(in: 3..<d.count))
        }
        return nil
    }
}

func decodeUnifiedBatteryStatus(_ params: Data) -> String {
    // params byte 0 = state-of-charge%, byte 1 = state-of-discharge%, byte 2 = status
    // byte 3 = battery level enum, byte 4 = external power flags
    guard params.count >= 5 else { return "(short, \(params.count) bytes)" }
    let soc = params[0]
    let sod = params[1]
    let status = params[2]
    let level = params[3]
    let ext = params[4]
    let statusStr = ["discharging","charging","charging-slow","charging-complete","charging-error"]
    let statusS = Int(status) < statusStr.count ? statusStr[Int(status)] : "unknown(\(status))"
    let levelStr = ["full","good","low","critical","unknown"]
    let levelS = Int(level) < levelStr.count ? levelStr[Int(level)] : "unknown(\(level))"
    return "soc=\(soc)% sod=\(sod)% status=\(statusS)(\(status)) level=\(levelS)(\(level)) externalPower=0x\(String(format: "%02x", ext))"
}

func decodeBatteryStatus0x1000(_ params: Data) -> String {
    guard params.count >= 4 else { return "(short, \(params.count) bytes)" }
    let level = params[0]
    let next = params[1]
    let status = params[2]
    let statusStr = ["discharging","recharging","charge-in-final-state","charge-complete","recharge-below-optimal","invalid-battery","thermal-error"]
    let statusS = Int(status) < statusStr.count ? statusStr[Int(status)] : "unknown(\(status))"
    return "level=\(level)% next=\(next)% status=\(statusS)(\(status))"
}

final class Probe: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    let args: Args
    var manager: CBCentralManager!
    var peripheral: CBPeripheral?
    var vendorChar: CBCharacteristic?
    var battChar: CBCharacteristic?
    var phase: String = "init"

    // HID++ state machine
    var swidNext: UInt8 = SOFTWARE_ID
    var unifiedBatteryIdx: UInt8 = 0
    var batteryStatusIdx: UInt8 = 0
    var withReportID: Bool
    var devIdx: UInt8 = DEV_INDEX_DIRECT
    var pendingExpectations: [(featIdx: UInt8, fn: UInt8, swid: UInt8, label: String, ts: Date)] = []
    var attemptedAlternates: Set<String> = []

    var listenStartedAt: Date?
    var listenEndsAt: Date?
    var startedAt: Date

    init(args: Args) {
        self.args = args
        self.withReportID = args.sendReportID
        self.startedAt = Date()
    }

    func out(_ s: String) {
        print("[\(iso(Date()))] \(s)")
        fflush(stdout)
    }
    func log(_ s: String) {
        fputs("# \(s)\n", stderr)
    }

    @objc func centralManagerDidUpdateState(_ c: CBCentralManager) {
        fputs("# delegate: state update raw=\(c.state.rawValue)\n", stderr)
        switch c.state {
        case .poweredOn: break
        case .unauthorized:
            fputs("error: Bluetooth permission denied.\n", stderr); exit(20)
        case .unsupported:
            fputs("error: Bluetooth unsupported.\n", stderr); exit(21)
        default: log("waiting state=\(c.state.rawValue)"); return
        }
        let connected = c.retrieveConnectedPeripherals(withServices: [batteryServiceUUID, deviceInfoServiceUUID, vendorServiceUUID])
        log("retrieveConnectedPeripherals: \(connected.count)")
        let want = args.name.lowercased()
        let target = connected.first { ($0.name ?? "").lowercased() == want }
                   ?? (connected.count == 1 ? connected.first : nil)
        guard let p = target else {
            fputs("error: peripheral '\(args.name)' not found\n", stderr)
            for p in connected { fputs("  - \(p.name ?? "?") \(p.identifier)\n", stderr) }
            exit(2)
        }
        peripheral = p
        p.delegate = self
        out("connecting to \(p.name ?? "?") \(p.identifier)")
        c.connect(p, options: nil)
    }

    func centralManager(_ c: CBCentralManager, didConnect p: CBPeripheral) {
        out("connected. discovering services.")
        phase = "discoverServices"
        p.discoverServices([batteryServiceUUID, vendorServiceUUID])
    }

    func centralManager(_ c: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        fputs("error: failed to connect: \(error?.localizedDescription ?? "?")\n", stderr); exit(3)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        if let e = error { fputs("error: discoverServices: \(e)\n", stderr); exit(4) }
        let svcs = p.services ?? []
        for s in svcs {
            out("service \(s.uuid.uuidString)")
            p.discoverCharacteristics(nil, for: s)
        }
    }

    var charsDiscovered = 0
    var servicesAwaited = 0
    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let e = error { fputs("error: discoverChars(\(service.uuid)): \(e)\n", stderr); return }
        for c in service.characteristics ?? [] {
            out("  char \(c.uuid.uuidString) props=\(c.properties.rawValue)")
            if c.uuid == vendorCharUUID { vendorChar = c }
            if c.uuid == batteryLevelUUID { battChar = c }
        }
        // After both vendor and battery services are scanned, subscribe + send.
        if vendorChar != nil && battChar != nil && phase == "discoverServices" {
            phase = "subscribe"
            startInteractions()
        }
    }

    func startInteractions() {
        guard let p = peripheral else { return }
        if let vc = vendorChar { p.setNotifyValue(true, for: vc) }
        if let bc = battChar   { p.setNotifyValue(true, for: bc); p.readValue(for: bc) }
    }

    var subscribedVendor = false
    var subscribedBatt = false
    func peripheral(_ p: CBPeripheral, didUpdateNotificationStateFor c: CBCharacteristic, error: Error?) {
        if let e = error { out("notify-state-error \(c.uuid): \(e)") }
        else { out("notify-state \(c.uuid.uuidString) isNotifying=\(c.isNotifying)") }
        if c.uuid == vendorCharUUID && c.isNotifying { subscribedVendor = true }
        if c.uuid == batteryLevelUUID && c.isNotifying { subscribedBatt = true }
        if subscribedVendor && subscribedBatt && phase == "subscribe" {
            phase = "resolve_1004"
            sendResolveFeature(0x1004)
        }
    }

    func nextSwid() -> UInt8 {
        var s = swidNext
        s = (s % 0x0F) + 1   // cycle in 1..15
        if s == 0 { s = 1 }
        swidNext = s
        return s
    }

    func sendFrame(_ data: Data, label: String, expectFeatIdx: UInt8, expectFn: UInt8, expectSwid: UInt8) {
        guard let p = peripheral, let c = vendorChar else { return }
        let exp = (featIdx: expectFeatIdx, fn: expectFn, swid: expectSwid, label: label, ts: Date())
        pendingExpectations.append(exp)
        // Vendor char allows .write and .writeWithoutResponse. Use withResponse first.
        let wt: CBCharacteristicWriteType = c.properties.contains(.write) ? .withResponse : .withoutResponse
        out(">>> SEND (\(label)) [\(hex(data))] (\(data.count) bytes, write=\(wt == .withResponse ? "withResponse" : "withoutResponse"), reportID=\(withReportID), devIdx=0x\(String(format:"%02x", devIdx)), len=\(currentForceLen), omitDi=\(currentOmitDevIdx))")
        p.writeValue(data, for: c, type: wt)
        // Schedule a response-timeout retry: if no matching HID++ frame arrives in 2s, swap variant.
        responseTimer?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self = self else { return }
            if !self.pendingExpectations.isEmpty {
                self.out("# response timeout (\(label)) — trying next framing variant")
                self.tryNextVariant()
            }
        }
        responseTimer = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0, execute: work)
    }

    func sendResolveFeature(_ featureID: UInt16) {
        let swid = nextSwid()
        let params: [UInt8] = [
            UInt8((featureID >> 8) & 0xFF),
            UInt8(featureID & 0xFF),
        ]
        let data = longPayload(devIdx: devIdx, featIdx: ROOT_FEATURE_INDEX, funcID: ROOT_FN_GET_FEATURE, swid: swid, params: params, withReportID: withReportID, forceLen: currentForceLen, omitDevIdx: currentOmitDevIdx)
        sendFrame(data, label: "root.getFeature(0x\(String(format:"%04x", featureID)))", expectFeatIdx: ROOT_FEATURE_INDEX, expectFn: ROOT_FN_GET_FEATURE, expectSwid: swid)
    }

    func sendUnifiedBatteryStatus() {
        let swid = nextSwid()
        let data = longPayload(devIdx: devIdx, featIdx: unifiedBatteryIdx, funcID: 1, swid: swid, params: [], withReportID: withReportID, forceLen: currentForceLen, omitDevIdx: currentOmitDevIdx)
        sendFrame(data, label: "unifiedBattery.getStatus", expectFeatIdx: unifiedBatteryIdx, expectFn: 1, expectSwid: swid)
    }

    // Probe for a range of HID++ feature IDs. Useful for confirming which
    // battery-related features the device supports.
    var probeFeatureQueue: [UInt16] = [
        0x0001, // FeatureSet
        0x0003, // DeviceInformation
        0x0005, // DeviceNameAndType
        0x0020, // ConfigChange
        0x1001, // BatteryUnifiedLevelStatus (older alt)
        0x1004, // UnifiedBattery
        0x1000, // BatteryStatus
        0x1010, // ChargingControl
        0x1F20, // RGB?
    ]
    var probedFeatures: [UInt16: UInt8] = [:]

    func probeNextFeature() {
        guard !probeFeatureQueue.isEmpty else {
            out("=== feature probe complete: \(probedFeatures) ===")
            return
        }
        let f = probeFeatureQueue.removeFirst()
        let swid = nextSwid()
        let params: [UInt8] = [UInt8((f >> 8) & 0xFF), UInt8(f & 0xFF)]
        let data = longPayload(devIdx: devIdx, featIdx: ROOT_FEATURE_INDEX, funcID: ROOT_FN_GET_FEATURE, swid: swid, params: params, withReportID: withReportID, forceLen: currentForceLen, omitDevIdx: currentOmitDevIdx)
        sendFrame(data, label: "probe(0x\(String(format:"%04x", f)))", expectFeatIdx: ROOT_FEATURE_INDEX, expectFn: ROOT_FN_GET_FEATURE, expectSwid: swid)
    }

    func sendBatteryStatus0x1000() {
        let swid = nextSwid()
        let data = longPayload(devIdx: devIdx, featIdx: batteryStatusIdx, funcID: 0, swid: swid, params: [], withReportID: withReportID, forceLen: currentForceLen, omitDevIdx: currentOmitDevIdx)
        sendFrame(data, label: "0x1000.getBatteryLevelStatus", expectFeatIdx: batteryStatusIdx, expectFn: 0, expectSwid: swid)
    }

    var lastWriteOk = false
    func peripheral(_ p: CBPeripheral, didWriteValueFor c: CBCharacteristic, error: Error?) {
        if let e = error {
            out("WRITE-ERROR \(c.uuid.uuidString): \(e)")
            // Try alternate framing if the first variant failed
            lastWriteOk = false
            tryNextVariant()
        } else {
            out("write-ok \(c.uuid.uuidString)")
            lastWriteOk = true
        }
    }

    // Variant table: (withReportID, devIdx, len, omitDevIdx).
    // Standard HID++ long-report on BLE: 19 bytes [devIdx=0xFF, featIdx, fn|swid, params...].
    var variantQueue: [(rid: Bool, di: UInt8, len: Int, omitDi: Bool)] = [
        (false, 0xFF, 19, false),  // standard
        (false, 0x01, 19, false),  // some receiver-bridge encodings
        (true,  0xFF, 20, false),  // with HID 0x11 prefix (rare on BLE)
        (false, 0x00, 18, true),   // last-ditch: no devIdx
    ]
    var currentForceLen: Int = 19
    var currentOmitDevIdx: Bool = false

    // Retry on response timeout (write succeeded but no HID++ reply on vendor char).
    var responseTimer: DispatchWorkItem?

    func tryNextVariant() {
        if variantQueue.isEmpty {
            out("# no more framing variants to try")
            return
        }
        let v = variantQueue.removeFirst()
        out("# variant: rid=\(v.rid) devIdx=0x\(String(format:"%02x", v.di)) len=\(v.len) omitDi=\(v.omitDi)")
        withReportID = v.rid
        devIdx = v.di
        currentForceLen = v.len
        currentOmitDevIdx = v.omitDi
        replayCurrentStep()
    }

    func replayCurrentStep() {
        pendingExpectations.removeAll()
        switch phase {
        case "resolve_1004": sendResolveFeature(0x1004)
        case "read_1004":    sendUnifiedBatteryStatus()
        case "resolve_1000": sendResolveFeature(0x1000)
        case "read_1000":    sendBatteryStatus0x1000()
        default: break
        }
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor c: CBCharacteristic, error: Error?) {
        if let e = error {
            out("READ/NOTIFY-ERR \(c.uuid.uuidString): \(e)")
            return
        }
        guard let v = c.value else { return }
        if c.uuid == batteryLevelUUID {
            let pct = v.first.map { Int($0) } ?? -1
            out("RECV 0x2A19 [\(hex(v))]  battery%=\(pct)")
            return
        }
        if c.uuid == vendorCharUUID {
            handleVendor(data: v)
            return
        }
        out("RECV \(c.uuid.uuidString) [\(hex(v))]")
    }

    func handleVendor(data v: Data) {
        let frame = HidppFrame.parse(v, omitDevIdx: currentOmitDevIdx)
        var line = "RECV vendor [\(hex(v))] (\(v.count) bytes)"
        if let f = frame {
            line += " | devIdx=0x\(String(format: "%02x", f.devIdx)) featIdx=0x\(String(format: "%02x", f.featIdx)) fn=\(f.fn) swid=\(f.swid) params=[\(hex(f.params))]"
        }
        out(line)
        guard let f = frame else { return }

        // Match against pending expectation
        var matched: Int? = nil
        for (i, exp) in pendingExpectations.enumerated() {
            // HID++ 2.0 error response: featIdx=0xFF, fn=0xF, swid matches; params byte 0 = orig featIdx, byte1 = orig fn|swid, byte 2 = err code
            if f.featIdx == 0xFF && f.fn == 0xF && f.swid == exp.swid {
                matched = i; break
            }
            if f.featIdx == exp.featIdx && f.fn == exp.fn && f.swid == exp.swid {
                matched = i; break
            }
        }

        if let i = matched {
            let exp = pendingExpectations.remove(at: i)
            responseTimer?.cancel()
            advanceState(matchedExpectation: exp, frame: f)
        } else {
            // Unsolicited / event
            if listenStartedAt != nil {
                out(">>> EVENT (unsolicited): featIdx=0x\(String(format:"%02x", f.featIdx)) fn=\(f.fn) params=[\(hex(f.params))]")
                if f.featIdx == unifiedBatteryIdx && unifiedBatteryIdx != 0 {
                    out(">>> EVENT decoded as UnifiedBattery: \(decodeUnifiedBatteryStatus(f.params))")
                }
                if f.featIdx == batteryStatusIdx && batteryStatusIdx != 0 {
                    out(">>> EVENT decoded as BatteryStatus0x1000: \(decodeBatteryStatus0x1000(f.params))")
                }
            } else {
                out("# unmatched/unexpected: featIdx=0x\(String(format:"%02x", f.featIdx)) fn=\(f.fn) swid=\(f.swid)")
            }
        }
    }

    func advanceState(matchedExpectation exp: (featIdx: UInt8, fn: UInt8, swid: UInt8, label: String, ts: Date), frame f: HidppFrame) {
        let dt = Date().timeIntervalSince(exp.ts)
        out("MATCH \(exp.label) (\(String(format: "%.3f", dt))s)")
        if f.featIdx == 0xFF && f.fn == 0xF {
            out("  HID++ 2.0 ERROR: \(hex(f.params))  (origFeatIdx=0x\(String(format:"%02x", f.params.first ?? 0)) origFn|swid=0x\(String(format:"%02x", f.params.dropFirst().first ?? 0)) errCode=\(f.params.dropFirst(2).first.map { String($0) } ?? "?"))")
        }
        if f.featIdx == 0x8F {
            out("  HID++ 1.0 ERROR: \(hex(f.params))")
        }
        switch phase {
        case "resolve_1004":
            if f.featIdx == 0x00 && f.fn == 0 {
                let idx = f.params.first ?? 0
                out("  -> 0x1004 feature index = 0x\(String(format: "%02x", idx))")
                if idx != 0 {
                    unifiedBatteryIdx = idx
                    phase = "read_1004"
                    sendUnifiedBatteryStatus()
                } else {
                    out("  -> 0x1004 not supported, trying 0x1000")
                    phase = "resolve_1000"
                    sendResolveFeature(0x1000)
                }
            }
        case "read_1004":
            out("  UnifiedBattery getStatus: \(decodeUnifiedBatteryStatus(f.params))")
            beginListenWindow()
        case "resolve_1000":
            if f.featIdx == 0x00 && f.fn == 0 {
                let idx = f.params.first ?? 0
                out("  -> 0x1000 feature index = 0x\(String(format: "%02x", idx))")
                if idx != 0 {
                    batteryStatusIdx = idx
                    phase = "read_1000"
                    sendBatteryStatus0x1000()
                } else {
                    out("  -> 0x1000 not supported either")
                    beginListenWindow()
                }
            }
        case "read_1000":
            out("  BatteryStatus 0x1000: \(decodeBatteryStatus0x1000(f.params))")
            beginListenWindow()
        case "final_read":
            if unifiedBatteryIdx != 0 {
                out("  FINAL UnifiedBattery: \(decodeUnifiedBatteryStatus(f.params))")
            } else if batteryStatusIdx != 0 {
                out("  FINAL BatteryStatus 0x1000: \(decodeBatteryStatus0x1000(f.params))")
            }
            out("=== DONE ===")
            exit(0)
        default:
            break
        }
    }

    func beginListenWindow() {
        listenStartedAt = Date()
        listenEndsAt = Date().addingTimeInterval(args.listenSeconds)
        out("================================================================")
        out("READY: please unplug the charging cable now, then wait 30s, then plug it back in, wait 30s. We are listening for HID++ events.")
        out("Listening for \(Int(args.listenSeconds))s. End at \(iso(listenEndsAt!))")
        out("================================================================")
        DispatchQueue.main.asyncAfter(deadline: .now() + args.listenSeconds) { [weak self] in
            self?.endListenWindow()
        }
    }

    func endListenWindow() {
        out("=== listen window ended; sending final getStatus for before/after comparison ===")
        phase = "final_read"
        if unifiedBatteryIdx != 0 {
            sendUnifiedBatteryStatus()
        } else if batteryStatusIdx != 0 {
            sendBatteryStatus0x1000()
        } else {
            out("=== no feature index resolved; nothing to read. exiting. ===")
            exit(0)
        }
    }
}

let args = parseArgs()
print("[\(iso(Date()))] startup args: name=\(args.name) listen=\(args.listenSeconds) timeout=\(args.totalTimeout) reportID=\(args.sendReportID)")
fflush(stdout)
fputs("# startup\n", stderr)
let probe = Probe(args: args)
probe.manager = CBCentralManager(delegate: probe, queue: nil)
fputs("# manager allocated state=\(probe.manager.state.rawValue)\n", stderr)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
    fputs("# 0.5s state=\(probe.manager.state.rawValue)\n", stderr)
}
DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
    fputs("# 2.0s state=\(probe.manager.state.rawValue)\n", stderr)
}

DispatchQueue.main.asyncAfter(deadline: .now() + args.totalTimeout) {
    fputs("# total timeout reached, exiting\n", stderr)
    exit(0)
}
RunLoop.main.run()
