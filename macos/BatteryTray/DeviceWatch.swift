import CoreAudio
import Foundation
import IOKit

/// Tells the core when the system's devices change, so a device shows as soon as it comes: an HID device that is
/// plugged in or connects over Bluetooth (receivers, mice, keyboards, controllers), and an audio device that comes or
/// goes, which is how AirPods and other Bluetooth headphones appear. Neither needs a permission.
@MainActor
final class DeviceWatch {
    private var port: IONotificationPortRef?
    private var iterators: [io_iterator_t] = []

    func start() {
        guard let port = IONotificationPortCreate(0) else { return }
        self.port = port
        CFRunLoopAddSource(CFRunLoopGetMain(), IONotificationPortGetRunLoopSource(port).takeUnretainedValue(), .defaultMode)
        for kind in [kIOFirstMatchNotification, kIOTerminatedNotification] {
            var iterator: io_iterator_t = 0
            let added = IOServiceAddMatchingNotification(port, kind, IOServiceMatching("IOHIDDevice"), { _, iterator in
                // Each notification has to empty its iterator, or none comes again
                DeviceWatch.drain(iterator)
                DispatchQueue.main.async { MainActor.assumeIsolated { DeviceWatch.changed() } }
            }, nil, &iterator)
            if added == KERN_SUCCESS {
                // The devices there already: emptied, which arms the notification without counting as a change
                DeviceWatch.drain(iterator)
                iterators.append(iterator)
            }
        }
        var devices = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain)
        AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &devices, DispatchQueue.main) { _, _ in
            MainActor.assumeIsolated { DeviceWatch.changed() }
        }
    }

    nonisolated private static func drain(_ iterator: io_iterator_t) {
        while case let service = IOIteratorNext(iterator), service != 0 {
            IOObjectRelease(service)
        }
    }

    private static func changed() {
        Core.call(["cmd": "devices_changed"])
    }
}
