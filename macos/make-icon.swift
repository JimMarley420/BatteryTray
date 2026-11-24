// Draws the app's icon for macOS into an .iconset folder, every size iconutil wants: the Windows icon's battery (a
// light outline, a green charge) on the rounded square of a Mac app icon.
//   swift make-icon.swift <folder.iconset>

import AppKit

let folder = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? "AppIcon.iconset")
try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)

/// The icon at `pixels` square, drawn in a 1024-point design.
func draw(_ pixels: Int) -> Data? {
    guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
                                     samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                     bytesPerRow: 0, bitsPerPixel: 0),
          let context = NSGraphicsContext(bitmapImageRep: rep)
    else { return nil }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    let scale = CGFloat(pixels) / 1024
    context.cgContext.scaleBy(x: scale, y: scale)

    // The rounded square of macOS 11 and later: 824 points of 1024, with room for its shadow
    let tile = NSRect(x: 100, y: 100, width: 824, height: 824)
    let shape = NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.35)
    shadow.shadowOffset = NSSize(width: 0, height: -10)
    shadow.shadowBlurRadius = 24
    NSGraphicsContext.saveGraphicsState()
    shadow.set()
    NSColor(white: 0.16, alpha: 1).setFill()
    shape.fill()
    NSGraphicsContext.restoreGraphicsState()
    NSGradient(starting: NSColor(white: 0.24, alpha: 1), ending: NSColor(white: 0.12, alpha: 1))?.draw(in: shape, angle: -90)

    // The battery: body, terminal, charge
    let body = NSBezierPath(roundedRect: NSRect(x: 222, y: 352, width: 520, height: 320), xRadius: 78, yRadius: 78)
    body.lineWidth = 44
    NSColor(white: 0.94, alpha: 1).setStroke()
    body.stroke()
    NSColor(white: 0.94, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 764, y: 448, width: 44, height: 128), xRadius: 14, yRadius: 14).fill()
    NSColor(srgbRed: 0x6C / 255, green: 0xCB / 255, blue: 0x5F / 255, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 290, y: 420, width: 250, height: 184), xRadius: 22, yRadius: 22).fill()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])
}

for points in [16, 32, 128, 256, 512] {
    for factor in [1, 2] {
        let name = factor == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
        try draw(points * factor)?.write(to: folder.appendingPathComponent(name))
    }
}
