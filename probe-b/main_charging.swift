// logi-mx-battery (charging-state probe variant)
//
// Full GATT enumeration: discover all services and characteristics on the MX
// Master 3 Mac, dump readable values, and look for a charging-state signal
// (BLE BAS 1.1 Battery Level Status char 0x2BED, Power State field).
//
// Build: see build_charging.sh
// Run:   ./logi-mx-charging  (wrapper script)

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
    var timeout: Double = 30.0
    var verbose = true
}
func parseArgs() -> Args {
    var a = Args()
    var it = CommandLine.arguments.dropFirst().makeIterator()
    while let arg = it.next() {
        switch arg {
        case "--name", "-n": if let s = it.next() { a.name = s }
        case "--timeout", "-t": if let s = it.next(), let v = Double(s) { a.timeout = v }
        case "--quiet", "-q": a.verbose = false
        default: fputs("unknown arg: \(arg)\n", stderr); exit(64)
        }
    }
    return a
}

let batteryServiceUUID = CBUUID(string: "180F")
let batteryLevelUUID   = CBUUID(string: "2A19")
let batteryLevelStatusUUID = CBUUID(string: "2BED")
let deviceInfoServiceUUID = CBUUID(string: "180A")

func hex(_ data: Data) -> String {
    return data.map { String(format: "%02x", $0) }.joined(separator: " ")
}

func propStr(_ p: CBCharacteristicProperties) -> String {
    var parts: [String] = []
    if p.contains(.broadcast) { parts.append("broadcast") }
    if p.contains(.read) { parts.append("read") }
    if p.contains(.writeWithoutResponse) { parts.append("writeNR") }
    if p.contains(.write) { parts.append("write") }
    if p.contains(.notify) { parts.append("notify") }
    if p.contains(.indicate) { parts.append("indicate") }
    if p.contains(.authenticatedSignedWrites) { parts.append("signedWrite") }
    if p.contains(.extendedProperties) { parts.append("ext") }
    if p.contains(.notifyEncryptionRequired) { parts.append("notifyEnc") }
    if p.contains(.indicateEncryptionRequired) { parts.append("indicateEnc") }
    return parts.joined(separator: ",")
}

// Decode BLE BAS 1.1 Battery Level Status (char 0x2BED).
// Layout: Flags(uint16 LE), PowerState(uint16 LE), [optional fields per flags].
// PowerState bits (Bluetooth Assigned Numbers / BAS 1.1):
//   bit 0:    Battery Present
//   bits 1-2: Wired External Power Source Connected (0=no,1=yes,2=unknown,3=RFU)
//   bits 3-4: Wireless External Power Source Connected (0=no,1=yes,2=unknown,3=RFU)
//   bits 5-6: Charge State (0=unknown,1=charging,2=discharging-active,3=discharging-inactive)
//   bits 7-8: Charge Level (0=unknown,1=good,2=low,3=critical)
//   bits 9-11: Charging Type (0=unknown,1=constant current,2=constant voltage,3=trickle,4=float)
//   bits 12-14: Charging Fault Reason (battery,external,other)
func decodeBatteryLevelStatus(_ d: Data) -> String {
    guard d.count >= 4 else { return "(too short: \(d.count) bytes)" }
    let flags = UInt16(d[0]) | (UInt16(d[1]) << 8)
    let ps = UInt16(d[2]) | (UInt16(d[3]) << 8)
    let present = (ps & 0x1) != 0
    let wired = (ps >> 1) & 0x3
    let wireless = (ps >> 3) & 0x3
    let chargeState = (ps >> 5) & 0x3
    let chargeLevel = (ps >> 7) & 0x3
    let chargingType = (ps >> 9) & 0x7
    let fault = (ps >> 12) & 0x7
    let wiredStr = ["no","YES","unknown","rfu"][Int(wired)]
    let wirelessStr = ["no","YES","unknown","rfu"][Int(wireless)]
    let chargeStateStr = ["unknown","CHARGING","discharging-active","discharging-inactive"][Int(chargeState)]
    let chargeLevelStr = ["unknown","good","low","critical"][Int(chargeLevel)]
    var s = String(format: "flags=0x%04x powerState=0x%04x", flags, ps)
    s += " | present=\(present) wiredExt=\(wiredStr) wirelessExt=\(wirelessStr)"
    s += " chargeState=\(chargeStateStr) chargeLevel=\(chargeLevelStr)"
    s += " chargingType=\(chargingType) fault=\(fault)"
    if d.count > 4 {
        s += " | extra=\(hex(d.subdata(in: 4..<d.count)))"
    }
    return s
}

final class Probe: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    let args: Args
    var manager: CBCentralManager!
    var peripheral: CBPeripheral?
    var pendingCharDiscovers = 0
    var pendingDescDiscovers = 0
    var pendingReads = 0
    var serviceList: [CBService] = []
    var characteristicValues: [CBUUID: Data] = [:]
    var notificationLog: [(CBUUID, Date, Data)] = []
    var subscribedFor2A19 = false
    var subscribedFor2BED = false
    var done = false
    var phase = "init"
    var enumerationComplete = false
    var notifyDeadline: Date?

    init(args: Args) { self.args = args }

    func log(_ s: String) { fputs("# \(s)\n", stderr) }
    func out(_ s: String) { print(s) }

    func centralManagerDidUpdateState(_ c: CBCentralManager) {
        switch c.state {
        case .poweredOn: break
        case .unauthorized:
            fputs("error: Bluetooth permission denied.\n", stderr); exit(20)
        case .unsupported:
            fputs("error: Bluetooth unsupported.\n", stderr); exit(21)
        default: log("waiting state=\(c.state.rawValue)"); return
        }
        // Try retrieving by any common service UUID; CB requires a service hint.
        let connected = c.retrieveConnectedPeripherals(withServices: [batteryServiceUUID, deviceInfoServiceUUID])
        log("connected peripherals advertising 0x180F or 0x180A: \(connected.count)")
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
        c.connect(p, options: nil)
    }

    func centralManager(_ c: CBCentralManager, didConnect p: CBPeripheral) {
        log("connected to \(p.name ?? "?") \(p.identifier)")
        phase = "discoverServices"
        p.discoverServices(nil)  // ALL services
    }

    func centralManager(_ c: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        fputs("error: failed to connect: \(error?.localizedDescription ?? "?")\n", stderr); exit(3)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        if let e = error { fputs("error: discoverServices: \(e)\n", stderr); exit(4) }
        let svcs = p.services ?? []
        out("=== SERVICES (\(svcs.count)) ===")
        for s in svcs {
            out(String(format: "service uuid=%@ primary=%@",
                       s.uuid.uuidString, s.isPrimary ? "true" : "false"))
        }
        serviceList = svcs
        pendingCharDiscovers = svcs.count
        for s in svcs {
            p.discoverCharacteristics(nil, for: s)
        }
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        defer {
            pendingCharDiscovers -= 1
            if pendingCharDiscovers == 0 { afterCharDiscovery() }
        }
        if let e = error { log("discoverChars(\(service.uuid)): \(e)"); return }
        let chars = service.characteristics ?? []
        out("--- service \(service.uuid.uuidString): \(chars.count) chars ---")
        for c in chars {
            out("  char uuid=\(c.uuid.uuidString) props=[\(propStr(c.properties))]")
        }
    }

    func afterCharDiscovery() {
        guard let p = peripheral else { return }
        // Discover descriptors for every char, and read every readable char.
        var allChars: [CBCharacteristic] = []
        for s in serviceList {
            for c in s.characteristics ?? [] { allChars.append(c) }
        }
        pendingDescDiscovers = allChars.count
        for c in allChars { p.discoverDescriptors(for: c) }

        for c in allChars where c.properties.contains(.read) {
            pendingReads += 1
            p.readValue(for: c)
        }
        if pendingReads == 0 && pendingDescDiscovers == 0 {
            afterAllReads()
        }
    }

    func peripheral(_ p: CBPeripheral, didDiscoverDescriptorsFor c: CBCharacteristic, error: Error?) {
        defer {
            pendingDescDiscovers -= 1
            checkEnumDone()
        }
        if let e = error { log("discoverDesc(\(c.uuid)): \(e)"); return }
        let descs = c.descriptors ?? []
        if !descs.isEmpty {
            out("    descriptors of \(c.uuid.uuidString):")
            for d in descs {
                out("      desc uuid=\(d.uuid.uuidString)")
            }
        }
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor c: CBCharacteristic, error: Error?) {
        if let e = error {
            log("read \(c.uuid): error \(e)")
        } else if let v = c.value {
            characteristicValues[c.uuid] = v
            // If we're in the notify phase, log it as a notification.
            if enumerationComplete {
                notificationLog.append((c.uuid, Date(), v))
                let label = renderValue(uuid: c.uuid, data: v)
                out(String(format: "[NOTIFY %@] %@ -> %@", iso(Date()), c.uuid.uuidString, label))
            } else {
                let label = renderValue(uuid: c.uuid, data: v)
                out("    READ \(c.uuid.uuidString) = \(label)")
            }
        }
        if !enumerationComplete {
            pendingReads -= 1
            checkEnumDone()
        }
    }

    func iso(_ d: Date) -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.string(from: d)
    }

    func renderValue(uuid: CBUUID, data: Data) -> String {
        let h = hex(data)
        // 0x2A19 Battery Level: 1 byte percent
        if uuid == batteryLevelUUID, let b = data.first {
            return "[\(h)]  battery%=\(Int(b))"
        }
        // 0x2BED Battery Level Status
        if uuid == batteryLevelStatusUUID {
            return "[\(h)]  \(decodeBatteryLevelStatus(data))"
        }
        // Try ascii for device-info-style chars
        let su = uuid.uuidString
        if ["2A24","2A25","2A26","2A27","2A28","2A29","2A23","2A50"].contains(su) {
            if let s = String(data: data, encoding: .utf8), !s.isEmpty {
                return "[\(h)]  ascii=\"\(s)\""
            }
        }
        return "[\(h)]"
    }

    func checkEnumDone() {
        if pendingCharDiscovers == 0 && pendingDescDiscovers == 0 && pendingReads == 0 && !enumerationComplete {
            afterAllReads()
        }
    }

    func afterAllReads() {
        enumerationComplete = true
        out("=== ENUMERATION COMPLETE ===")

        // Highlight charging signal if present.
        if let v = characteristicValues[batteryLevelStatusUUID] {
            out(">>> CHARGING SIGNAL FOUND: char 0x2BED present.")
            out(">>> 2BED raw=[\(hex(v))]")
            out(">>> 2BED decoded: \(decodeBatteryLevelStatus(v))")
        } else {
            out(">>> char 0x2BED (Battery Level Status) NOT present on this peripheral.")
        }
        if let v = characteristicValues[batteryLevelUUID], let b = v.first {
            out(">>> current 0x2A19 battery%=\(Int(b))")
        }

        // Subscribe to 2A19 (and 2BED if available) for ~remaining timeout to capture notifications.
        guard let p = peripheral else { exit(0) }
        for s in serviceList {
            for c in s.characteristics ?? [] {
                if c.uuid == batteryLevelUUID && c.properties.contains(.notify) {
                    p.setNotifyValue(true, for: c); subscribedFor2A19 = true
                    out("=== subscribed to 0x2A19 notifications ===")
                }
                if c.uuid == batteryLevelStatusUUID && (c.properties.contains(.notify) || c.properties.contains(.indicate)) {
                    p.setNotifyValue(true, for: c); subscribedFor2BED = true
                    out("=== subscribed to 0x2BED notifications ===")
                }
            }
        }
        if !subscribedFor2A19 && !subscribedFor2BED {
            out("=== no notifyable battery chars; finishing ===")
            exit(0)
        }
        // Stay alive until --timeout to gather notifications.
    }

    func peripheral(_ p: CBPeripheral, didUpdateNotificationStateFor c: CBCharacteristic, error: Error?) {
        if let e = error { log("notifyState(\(c.uuid)): \(e)") }
        else { log("notifyState(\(c.uuid)): isNotifying=\(c.isNotifying)") }
    }
}

let args = parseArgs()
let probe = Probe(args: args)
probe.manager = CBCentralManager(delegate: probe, queue: nil)

DispatchQueue.main.asyncAfter(deadline: .now() + args.timeout) {
    fputs("# timeout reached, exiting\n", stderr)
    exit(0)
}
RunLoop.main.run()
