// probe.swift — Read battery % from Logitech MX Master 3 Mac via HID++ 2.0
// Build:  swiftc -O -o probe probe.swift -framework IOKit -framework CoreFoundation
//
// Sequence:
//   1. Open IOHIDDevice for VID 0x046D / PID 0xB023.
//   2. Send HID++ "feature root" getFeature(0x1004) → returns featureIndex for UnifiedBattery.
//   3. Send HID++ feature 0x1004 fn 0 (getCapabilities? or getStatus?). Per Solaar:
//        - fn 0 (0x00): get capabilities
//        - fn 1 (0x10): get status -> byte0 = stateOfCharge%
//      So we use fn 1 (function nibble 1 -> nibble << 4 | swid).
//   4. Print SoC%.
// We use HID++ long report (0x11, 20 bytes).

import Foundation
import IOKit
import IOKit.hid
import CoreFoundation

// ---- Logging ----
func log(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

// ---- HID++ constants ----
let HIDPP_LONG: UInt8 = 0x11
let HIDPP_SHORT: UInt8 = 0x10
let DEV_INDEX: UInt8 = 0xFF        // direct connection (BLE / non-receiver)
let SOFTWARE_ID: UInt8 = 0x08      // arbitrary 4-bit value (1..0xF)

// Feature root (always at index 0x00) function getFeature
let ROOT_FEATURE_INDEX: UInt8 = 0x00
let ROOT_FN_GET_FEATURE: UInt8 = 0x00     // function id 0

let FEATURE_UNIFIED_BATTERY: UInt16 = 0x1004
let FEATURE_BATTERY_STATUS:  UInt16 = 0x1000

// ---- Globals for callback ----
final class State {
    var lastResponse: [UInt8]? = nil
    let lock = NSLock()
    let cond = NSCondition()
    var got: Bool = false

    func deliver(_ bytes: [UInt8]) {
        cond.lock()
        lastResponse = bytes
        got = true
        cond.signal()
        cond.unlock()
    }

    func waitForResponse(timeout: TimeInterval) -> [UInt8]? {
        cond.lock()
        defer { cond.unlock() }
        let deadline = Date().addingTimeInterval(timeout)
        while !got {
            if !cond.wait(until: deadline) { return nil }
        }
        let r = lastResponse
        got = false
        lastResponse = nil
        return r
    }
}

let state = State()

func hex(_ bytes: [UInt8]) -> String {
    return bytes.map { String(format: "%02X", $0) }.joined(separator: " ")
}

// Input report callback — runs on the run loop we register on.
let inputCB: IOHIDReportCallback = { context, result, sender, type, reportID, report, reportLength in
    var bytes = [UInt8](repeating: 0, count: Int(reportLength) + 1)
    // The report buffer does NOT include the report ID prefix; reportID is separate.
    bytes[0] = UInt8(reportID)
    for i in 0..<Int(reportLength) {
        bytes[i + 1] = report[i]
    }
    log("[in ] \(hex(bytes))")
    state.deliver(bytes)
}

// ---- Find the MX Master HID device ----
func openDevice(vid: Int, pid: Int) -> IOHIDDevice? {
    let mgr = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
    let match: [String: Any] = [
        kIOHIDVendorIDKey as String: vid,
        kIOHIDProductIDKey as String: pid,
    ]
    IOHIDManagerSetDeviceMatching(mgr, match as CFDictionary)
    let openRes = IOHIDManagerOpen(mgr, IOOptionBits(kIOHIDOptionsTypeNone))
    if openRes != kIOReturnSuccess {
        log("IOHIDManagerOpen failed: \(String(format: "0x%08X", openRes))")
    }
    guard let devs = IOHIDManagerCopyDevices(mgr) as? Set<IOHIDDevice>, !devs.isEmpty else {
        log("No matching HID devices")
        return nil
    }
    for d in devs {
        let prod = (IOHIDDeviceGetProperty(d, kIOHIDProductKey as CFString) as? String) ?? "?"
        let trans = (IOHIDDeviceGetProperty(d, kIOHIDTransportKey as CFString) as? String) ?? "?"
        let maxOut = (IOHIDDeviceGetProperty(d, kIOHIDMaxOutputReportSizeKey as CFString) as? Int) ?? -1
        let maxIn  = (IOHIDDeviceGetProperty(d, kIOHIDMaxInputReportSizeKey as CFString) as? Int) ?? -1
        let usage  = (IOHIDDeviceGetProperty(d, kIOHIDPrimaryUsageKey as CFString) as? Int) ?? -1
        let usageP = (IOHIDDeviceGetProperty(d, kIOHIDPrimaryUsagePageKey as CFString) as? Int) ?? -1
        log("device: \(prod) (\(trans)) maxIn=\(maxIn) maxOut=\(maxOut) usagePage=\(usageP) usage=\(usage)")
    }
    return devs.first
}

func sendHIDPP(_ device: IOHIDDevice, frame: [UInt8]) -> IOReturn {
    log("[out] \(hex(frame))")
    let reportID = CFIndex(frame[0])
    let body = Array(frame.dropFirst()) // payload without report ID prefix
    return body.withUnsafeBufferPointer { buf in
        IOHIDDeviceSetReport(device,
                             kIOHIDReportTypeOutput,
                             reportID,
                             buf.baseAddress!,
                             body.count)
    }
}

// Build a HID++ long-report frame (20 bytes total: reportID + 19 payload).
func longFrame(devIdx: UInt8, featIdx: UInt8, funcID: UInt8, swid: UInt8, params: [UInt8]) -> [UInt8] {
    var f = [UInt8](repeating: 0, count: 20)
    f[0] = HIDPP_LONG
    f[1] = devIdx
    f[2] = featIdx
    f[3] = (funcID << 4) | (swid & 0x0F)
    for (i, b) in params.enumerated() where i < 16 {
        f[4 + i] = b
    }
    return f
}

func isResponseFor(_ resp: [UInt8], featIdx: UInt8, funcID: UInt8, swid: UInt8) -> Bool {
    guard resp.count >= 4 else { return false }
    // resp[0] = report id (0x10/0x11), resp[1] = devIdx, resp[2] = featIdx, resp[3] = (fn<<4)|swid
    if resp[2] == 0x8F { return true } // HID++ 1.0 ERROR
    if resp[2] != featIdx { return false }
    let expected = (funcID << 4) | (swid & 0x0F)
    return resp[3] == expected
}

// Returns response or nil
func roundTrip(_ device: IOHIDDevice, frame: [UInt8], featIdx: UInt8, funcID: UInt8, swid: UInt8, timeout: TimeInterval = 1.5) -> [UInt8]? {
    // Drain any stale response
    state.cond.lock(); state.got = false; state.lastResponse = nil; state.cond.unlock()

    let res = sendHIDPP(device, frame: frame)
    if res != kIOReturnSuccess {
        log("SetReport error: \(String(format: "0x%08X", res))")
        return nil
    }

    // Wait — but we need the run loop to actually run for the input callback to fire.
    // We process the run loop in small ticks until match-or-timeout.
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        // run for 50ms
        CFRunLoopRunInMode(.defaultMode, 0.05, true)
        state.cond.lock()
        let got = state.got
        let r = state.lastResponse
        state.cond.unlock()
        if got, let r = r {
            // Consume
            state.cond.lock(); state.got = false; state.lastResponse = nil; state.cond.unlock()
            // Filter: ignore non-HID++ reports (e.g. mouse movement on report 1/2)
            if r.count > 0 && (r[0] == HIDPP_LONG || r[0] == HIDPP_SHORT) {
                if isResponseFor(r, featIdx: featIdx, funcID: funcID, swid: swid) {
                    return r
                } else {
                    log("[skip] not for us: \(hex(r))")
                }
            } else {
                log("[skip] non-HIDPP report id 0x\(String(format: "%02X", r.first ?? 0))")
            }
        }
    }
    return nil
}

// ---- Main probe ----
func main() -> Int32 {
    let VID = 0x046D
    let PID = 0xB023

    guard let dev = openDevice(vid: VID, pid: PID) else {
        log("ERROR: device not found")
        return 2
    }

    // Open the device exclusively? Try without first.
    var openOpts: IOOptionBits = IOOptionBits(kIOHIDOptionsTypeNone)
    var openRes = IOHIDDeviceOpen(dev, openOpts)
    if openRes != kIOReturnSuccess {
        log("IOHIDDeviceOpen (no opts) failed: 0x\(String(format: "%08X", openRes)) — trying seize")
        openOpts = IOOptionBits(kIOHIDOptionsTypeSeizeDevice)
        openRes = IOHIDDeviceOpen(dev, openOpts)
        if openRes != kIOReturnSuccess {
            log("ERROR: IOHIDDeviceOpen seize also failed: 0x\(String(format: "%08X", openRes))")
            return 3
        }
    }

    // Allocate input buffer big enough for the largest input report (HID++ long = 19 bytes payload).
    let bufLen = 64
    let buf = UnsafeMutablePointer<UInt8>.allocate(capacity: bufLen)
    buf.initialize(repeating: 0, count: bufLen)
    IOHIDDeviceRegisterInputReportCallback(dev, buf, bufLen, inputCB, nil)
    IOHIDDeviceScheduleWithRunLoop(dev, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)

    // ---- Step 1: getFeature(0x1004) on root ----
    var swid: UInt8 = SOFTWARE_ID
    let getFeatParams: [UInt8] = [
        UInt8((FEATURE_UNIFIED_BATTERY >> 8) & 0xFF),
        UInt8(FEATURE_UNIFIED_BATTERY & 0xFF),
        0x00,
    ]
    var frame = longFrame(devIdx: DEV_INDEX, featIdx: ROOT_FEATURE_INDEX,
                          funcID: ROOT_FN_GET_FEATURE, swid: swid, params: getFeatParams)
    var resp = roundTrip(dev, frame: frame, featIdx: ROOT_FEATURE_INDEX, funcID: ROOT_FN_GET_FEATURE, swid: swid)
    var unifiedIdx: UInt8 = 0
    if let r = resp, r.count >= 5, r[2] != 0x8F, r[4] != 0 {
        unifiedIdx = r[4]
        log("UnifiedBattery (0x1004) feature index = 0x\(String(format: "%02X", unifiedIdx))")
    } else {
        if let r = resp { log("getFeature 0x1004 returned: \(hex(r)) — feature absent or error") }
        else { log("No response to getFeature 0x1004") }
    }

    // ---- Step 2: read battery ----
    if unifiedIdx != 0 {
        // Try fn 1 (getStatus) first per Solaar hidpp20.py UnifiedBattery
        // Function indices for 0x1004:
        //   0: get_capabilities
        //   1: get_status -> stateOfCharge (byte 0), endOfDischarge (byte 1), batteryLevel (byte 2), chargingStatus (byte 3), externalPowerStatus(byte 4)
        for fn in [UInt8(1), UInt8(0)] {
            swid = (swid % 0xF) + 1
            if swid == 0 { swid = 1 }
            let f = longFrame(devIdx: DEV_INDEX, featIdx: unifiedIdx, funcID: fn, swid: swid, params: [])
            let r = roundTrip(dev, frame: f, featIdx: unifiedIdx, funcID: fn, swid: swid)
            if let r = r {
                if r.count >= 5 && r[2] != 0x8F {
                    if fn == 1 {
                        let soc = r[4]
                        log("UnifiedBattery getStatus payload: \(hex(Array(r[4..<min(r.count, 12)])))")
                        print("\(soc)")
                        IOHIDDeviceClose(dev, openOpts)
                        return 0
                    } else {
                        log("getCapabilities: \(hex(r))")
                    }
                } else {
                    log("UnifiedBattery fn\(fn) error/short: \(hex(r))")
                }
            } else {
                log("UnifiedBattery fn\(fn): no response")
            }
        }
    }

    // ---- Fallback: feature 0x1000 (BatteryStatus) ----
    log("Falling back to feature 0x1000 (BatteryStatus)")
    swid = (swid % 0xF) + 1
    let getFeat1000: [UInt8] = [
        UInt8((FEATURE_BATTERY_STATUS >> 8) & 0xFF),
        UInt8(FEATURE_BATTERY_STATUS & 0xFF),
        0x00,
    ]
    frame = longFrame(devIdx: DEV_INDEX, featIdx: ROOT_FEATURE_INDEX,
                      funcID: ROOT_FN_GET_FEATURE, swid: swid, params: getFeat1000)
    resp = roundTrip(dev, frame: frame, featIdx: ROOT_FEATURE_INDEX, funcID: ROOT_FN_GET_FEATURE, swid: swid)
    var bsIdx: UInt8 = 0
    if let r = resp, r.count >= 5, r[2] != 0x8F, r[4] != 0 {
        bsIdx = r[4]
        log("BatteryStatus (0x1000) feature index = 0x\(String(format: "%02X", bsIdx))")
    }
    if bsIdx != 0 {
        // fn 0: getBatteryLevelStatus -> byte 0 = discharge level%, byte 1 = next level%, byte 2 = status
        swid = (swid % 0xF) + 1
        let f = longFrame(devIdx: DEV_INDEX, featIdx: bsIdx, funcID: 0, swid: swid, params: [])
        if let r = roundTrip(dev, frame: f, featIdx: bsIdx, funcID: 0, swid: swid),
           r.count >= 5, r[2] != 0x8F {
            let level = r[4]
            log("BatteryStatus getBatteryLevelStatus: \(hex(r))")
            print("\(level)")
            IOHIDDeviceClose(dev, openOpts)
            return 0
        }
    }

    log("ERROR: no battery info retrieved")
    IOHIDDeviceClose(dev, openOpts)
    return 4
}

exit(main())
