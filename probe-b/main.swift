// logi-mx-battery
//
// Reads the standard BLE Battery Service (0x180F, characteristic 0x2A19) from a
// paired-and-connected Bluetooth peripheral via CoreBluetooth and prints its
// percentage to stdout.
//
// CoreBluetooth requires:
//   1. A bundle Info.plist with NSBluetoothAlwaysUsageDescription.
//   2. The user to grant Bluetooth permission via the TCC prompt on first run
//      (System Settings > Privacy & Security > Bluetooth).
//
// Build:
//   swiftc -framework CoreBluetooth -framework Foundation \
//     -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist \
//     -Xlinker Info.plist main.swift -o logi-mx-battery
//
// Then bundle into a .app (see build.sh) and codesign --force --sign -.

import Foundation
import CoreBluetooth

// When run via `open` from inside an .app bundle, stdout is not connected to
// the calling terminal. The wrapper sets LOGI_MX_OUT to a file path; we
// redirect stdout there so it can be read back.
if let path = ProcessInfo.processInfo.environment["LOGI_MX_OUT"] {
    freopen(path, "w", stdout)
    setbuf(stdout, nil)
}
if let path = ProcessInfo.processInfo.environment["LOGI_MX_ERR"] {
    freopen(path, "w", stderr)
    setbuf(stderr, nil)
}

struct Args {
    var name: String = "MX Master 3 Mac"
    var timeout: Double = 8.0
    var verbose = false
}

func parseArgs() -> Args {
    var a = Args()
    var it = CommandLine.arguments.dropFirst().makeIterator()
    while let arg = it.next() {
        switch arg {
        case "--name", "-n":
            if let s = it.next() { a.name = s }
        case "--timeout", "-t":
            if let s = it.next(), let v = Double(s) { a.timeout = v }
        case "--verbose", "-v":
            a.verbose = true
        case "--help", "-h":
            print("Usage: logi-mx-battery [--name 'MX Master 3 Mac'] [--timeout 8] [-v]")
            exit(0)
        default:
            fputs("unknown arg: \(arg)\n", stderr); exit(64)
        }
    }
    return a
}

let batteryServiceUUID = CBUUID(string: "180F")
let batteryLevelUUID   = CBUUID(string: "2A19")

final class BatteryReader: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    let args: Args
    var manager: CBCentralManager!
    var connecting: CBPeripheral?
    var done = false
    init(args: Args) { self.args = args }
    func log(_ s: String) { if args.verbose { fputs("# \(s)\n", stderr) } }

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn: break
        case .unauthorized:
            fputs("error: Bluetooth permission denied. Approve in System Settings > Privacy & Security > Bluetooth.\n", stderr)
            exit(20)
        case .unsupported:
            fputs("error: Bluetooth unsupported.\n", stderr); exit(21)
        default:
            log("waiting (state=\(central.state.rawValue))…"); return
        }

        let connected = central.retrieveConnectedPeripherals(withServices: [batteryServiceUUID])
        log("connected peripherals advertising 0x180F: \(connected.count)")
        let want = args.name.lowercased()
        let target = connected.first { $0.name?.lowercased() == want }
                   ?? (connected.count == 1 ? connected.first : nil)
        guard let p = target else {
            fputs("error: peripheral '\(args.name)' not found among connected BLE devices with battery service\n", stderr)
            for p in connected { fputs("  - \(p.name ?? "?") \(p.identifier)\n", stderr) }
            exit(2)
        }
        connecting = p
        p.delegate = self
        central.connect(p, options: nil)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        log("connected to \(peripheral.name ?? "?")")
        peripheral.discoverServices([batteryServiceUUID])
    }
    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        fputs("error: failed to connect: \(error?.localizedDescription ?? "?")\n", stderr); exit(3)
    }
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if let e = error { fputs("error: discover services: \(e)\n", stderr); exit(4) }
        for s in peripheral.services ?? [] where s.uuid == batteryServiceUUID {
            peripheral.discoverCharacteristics([batteryLevelUUID], for: s); return
        }
        fputs("error: battery service not found on peripheral\n", stderr); exit(5)
    }
    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let e = error { fputs("error: discover characteristics: \(e)\n", stderr); exit(6) }
        for c in service.characteristics ?? [] where c.uuid == batteryLevelUUID {
            peripheral.readValue(for: c); return
        }
        fputs("error: battery level characteristic not found\n", stderr); exit(7)
    }
    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        if let e = error { fputs("error: read value: \(e)\n", stderr); exit(8) }
        guard let v = characteristic.value, let b = v.first else {
            fputs("error: empty battery value\n", stderr); exit(9)
        }
        print(Int(b))
        done = true
        exit(0)
    }
}

let args = parseArgs()
let reader = BatteryReader(args: args)
reader.manager = CBCentralManager(delegate: reader, queue: nil)
DispatchQueue.main.asyncAfter(deadline: .now() + args.timeout) {
    if !reader.done {
        fputs("error: timed out after \(args.timeout)s\n", stderr); exit(124)
    }
}
RunLoop.main.run()
