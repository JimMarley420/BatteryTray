import CoreBluetooth
import Foundation

/// The Battery Service (0x180F) of the Bluetooth LE devices connected to the Mac, for those whose level macOS shows
/// nowhere else: mice and keyboards that report it the standard way, and the like. Only devices that are connected
/// already are read (the app joins their link, it never connects a device that is away), and only read: nothing is
/// written to them, no notifications are asked for. macOS asks once for the Bluetooth permission, with the reason in
/// Info.plist; refused, this stays quiet. The levels go to the core, which leaves out a device another provider
/// already lists (AirPods from system_profiler, an MX mouse over HID++, a game controller).
@MainActor
final class BleBatteries: NSObject {
    private static let batteryService = CBUUID(string: "180F")
    private static let batteryLevel = CBUUID(string: "2A19")

    private var central: CBCentralManager?
    private var timer: Timer?
    /// The devices being read, by their identifier, with their last level
    private var peripherals: [UUID: CBPeripheral] = [:]
    private var levels: [UUID: Int] = [:]

    func start() {
        // Creating the manager is what asks for the permission, once
        central = CBCentralManager(delegate: self, queue: .main, options: [CBCentralManagerOptionShowPowerAlertKey: false])
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { _ in
            MainActor.assumeIsolated { AppDelegate.shared?.bleBatteries.poll() }
        }
    }

    /// Finds the connected devices with a Battery Service and reads each one's level.
    func poll() {
        guard let central, central.state == .poweredOn else { return }
        let connected = central.retrieveConnectedPeripherals(withServices: [Self.batteryService])
        let ids = Set(connected.map(\.identifier))
        for gone in peripherals.keys where !ids.contains(gone) {
            peripherals[gone] = nil
            levels[gone] = nil
        }
        for peripheral in connected {
            if peripherals[peripheral.identifier] == nil {
                peripherals[peripheral.identifier] = peripheral
                peripheral.delegate = self
            }
            if peripheral.state == .connected {
                read(peripheral)
            } else {
                // Joins the link the system already has with it
                central.connect(peripheral)
            }
        }
        report()
    }

    private func read(_ peripheral: CBPeripheral) {
        let characteristic = peripheral.services?.first { $0.uuid == Self.batteryService }?
            .characteristics?.first { $0.uuid == Self.batteryLevel }
        if let characteristic {
            peripheral.readValue(for: characteristic)
        } else {
            peripheral.discoverServices([Self.batteryService])
        }
    }

    private func report() {
        let readings: [[String: Any]] = peripherals.compactMap { id, peripheral in
            guard let level = levels[id], let name = peripheral.name, !name.isEmpty else { return nil }
            return ["id": "ble:" + id.uuidString, "name": name, "kind": "", "level": level]
        }
        Core.call(["cmd": "external", "source": "ble", "readings": readings])
    }
}

extension BleBatteries: CBCentralManagerDelegate {
    nonisolated func centralManagerDidUpdateState(_ central: CBCentralManager) {
        MainActor.assumeIsolated { poll() }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        MainActor.assumeIsolated { read(peripheral) }
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral,
                                    error: Error?) {
        MainActor.assumeIsolated {
            peripherals[peripheral.identifier] = nil
            levels[peripheral.identifier] = nil
            report()
        }
    }
}

extension BleBatteries: CBPeripheralDelegate {
    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        MainActor.assumeIsolated {
            guard let service = peripheral.services?.first(where: { $0.uuid == Self.batteryService }) else { return }
            peripheral.discoverCharacteristics([Self.batteryLevel], for: service)
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService,
                                error: Error?) {
        MainActor.assumeIsolated { read(peripheral) }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic,
                                error: Error?) {
        MainActor.assumeIsolated {
            guard characteristic.uuid == Self.batteryLevel, let value = characteristic.value?.first, value <= 100 else {
                return
            }
            if levels[peripheral.identifier] != Int(value) {
                levels[peripheral.identifier] = Int(value)
                report()
            }
        }
    }
}
