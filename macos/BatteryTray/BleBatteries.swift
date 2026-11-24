import CoreBluetooth
import Foundation

/// The Battery Service (0x180F) of the Bluetooth LE devices connected to the Mac, for those whose level macOS shows
/// nowhere else: mice and keyboards that report it the standard way, and the like. Only devices the Mac is connected to
/// already are read, and only read: nothing is written to them, no notifications are asked for. Once a minute the app
/// joins the link the system has with each of them for one reading and lets go of it right after, so the link stays
/// the system's: a device switched off or disconnected in System Settings goes as it would without the app, and the app
/// never brings one back by itself. macOS asks once for the Bluetooth permission, with the reason in Info.plist;
/// refused, this stays quiet. The levels go to the core, which leaves out a device another provider already lists
/// (AirPods from system_profiler, an MX mouse over HID++, a game controller).
@MainActor
final class BleBatteries: NSObject {
    private static let batteryService = CBUUID(string: "180F")
    private static let batteryLevel = CBUUID(string: "2A19")

    private var central: CBCentralManager?
    private var timer: Timer?
    /// The devices being read, by their identifier, with their last level
    private var peripherals: [UUID: CBPeripheral] = [:]
    private var levels: [UUID: Int] = [:]
    /// The devices the app has just let go of: their disconnection is the app's own, not the device going away
    private var released: Set<UUID> = []

    func start() {
        // Creating the manager is what asks for the permission, once
        central = CBCentralManager(delegate: self, queue: .main, options: [CBCentralManagerOptionShowPowerAlertKey: false])
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { _ in
            MainActor.assumeIsolated { AppDelegate.shared?.bleBatteries.poll() }
        }
    }

    /// Lets go of every device (the app is quitting).
    func stop() {
        timer?.invalidate()
        timer = nil
        for peripheral in peripherals.values {
            release(peripheral)
        }
        peripherals = [:]
    }

    /// Finds the connected devices with a Battery Service and reads each one's level.
    func poll() {
        guard let central, central.state == .poweredOn else { return }
        let connected = central.retrieveConnectedPeripherals(withServices: [Self.batteryService])
        let ids = Set(connected.map(\.identifier))
        for (id, gone) in peripherals where !ids.contains(id) {
            // No longer connected to the Mac: a connection still being made for a reading is called off
            release(gone)
            forget(id)
        }
        for peripheral in connected {
            if peripherals[peripheral.identifier] == nil {
                peripherals[peripheral.identifier] = peripheral
                peripheral.delegate = self
            }
            if peripheral.state == .connected {
                read(peripheral)
            } else {
                // Joins the link the system already has with it, for this reading
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

    /// The app's part of the link goes; the system's stays.
    private func release(_ peripheral: CBPeripheral) {
        guard let central, peripheral.state != .disconnected else { return }
        released.insert(peripheral.identifier)
        central.cancelPeripheralConnection(peripheral)
    }

    private func forget(_ id: UUID) {
        peripherals[id] = nil
        levels[id] = nil
        released.remove(id)
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

    nonisolated func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral,
                                    error: Error?) {
        // Tried again at the next poll, while the Mac is still connected to it
    }

    nonisolated func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral,
                                    error: Error?) {
        MainActor.assumeIsolated {
            // The app let go of it after a reading: its level stays. Otherwise the device went away
            if released.remove(peripheral.identifier) == nil {
                forget(peripheral.identifier)
                report()
            }
        }
    }
}

extension BleBatteries: CBPeripheralDelegate {
    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        MainActor.assumeIsolated {
            guard let service = peripheral.services?.first(where: { $0.uuid == Self.batteryService }) else {
                release(peripheral)
                return
            }
            peripheral.discoverCharacteristics([Self.batteryLevel], for: service)
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService,
                                error: Error?) {
        MainActor.assumeIsolated {
            if service.characteristics?.contains(where: { $0.uuid == Self.batteryLevel }) == true {
                read(peripheral)
            } else {
                release(peripheral)
            }
        }
    }

    nonisolated func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic,
                                error: Error?) {
        MainActor.assumeIsolated {
            guard characteristic.uuid == Self.batteryLevel else { return }
            // One reading per minute: the app lets go of the link until the next one
            release(peripheral)
            guard let value = characteristic.value?.first, value <= 100 else { return }
            if levels[peripheral.identifier] != Int(value) {
                levels[peripheral.identifier] = Int(value)
                report()
            }
        }
    }
}
