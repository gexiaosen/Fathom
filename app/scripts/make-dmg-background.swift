// Generate the DMG background at 2x. Run once after changing copy or layout:
//   swiftc -O scripts/make-dmg-background.swift -o /tmp/fathom-dmg-bg && /tmp/fathom-dmg-bg
//
// The window is 600×360 pt. The app icon sits at (150,160), Applications at
// (450,160); those coordinates must match packaging/dmg/DS_Store.
import AppKit

let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
let out = repoRoot.appendingPathComponent("packaging/dmg/background.png")

let width: CGFloat = 600
let height: CGFloat = 360
let scale: CGFloat = 2

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    NSColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255,
        alpha: alpha
    )
}

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(width * scale),
    pixelsHigh: Int(height * scale),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
)!
rep.size = NSSize(width: width, height: height)

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext

// Warm paper-like background, matching the app's chrome rather than a
// generic macOS blue installer.
NSGradient(colorsAndLocations:
    (color(0xFFFDF9), 0),
    (color(0xF8F5EE), 0.48),
    (color(0xECE8DF), 1)
)!
.draw(in: NSRect(x: 0, y: 0, width: width, height: height), angle: -90)

let midY = height - 160
let accent = color(0x2F6FED)

let arrow = NSBezierPath()
arrow.move(to: NSPoint(x: 244, y: midY - 5))
arrow.line(to: NSPoint(x: 326, y: midY - 5))
arrow.line(to: NSPoint(x: 326, y: midY - 17))
arrow.line(to: NSPoint(x: 352, y: midY))
arrow.line(to: NSPoint(x: 326, y: midY + 17))
arrow.line(to: NSPoint(x: 326, y: midY + 5))
arrow.line(to: NSPoint(x: 244, y: midY + 5))
arrow.close()
arrow.lineJoinStyle = .round
arrow.lineCapStyle = .round
arrow.lineWidth = 8
accent.withAlphaComponent(0.82).setFill()
accent.withAlphaComponent(0.82).setStroke()
arrow.fill()
arrow.stroke()

func draw(_ text: String, size: CGFloat, weight: NSFont.Weight, tint: NSColor,
          centerY: CGFloat, tracking: CGFloat = 0) {
    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: size, weight: weight),
        .foregroundColor: tint,
        .kern: tracking,
    ]
    let string = NSAttributedString(string: text, attributes: attributes)
    let box = string.size()
    string.draw(at: NSPoint(x: (width - box.width) / 2, y: centerY))
}

draw("将 Fathom 拖入「应用程序」", size: 15, weight: .medium,
     tint: color(0x2B3140), centerY: 92, tracking: 0.2)
draw("Drag Fathom into Applications", size: 11, weight: .regular,
     tint: color(0x7A8291), centerY: 70, tracking: 0.1)

NSGraphicsContext.restoreGraphicsState()

try FileManager.default.createDirectory(
    at: out.deletingLastPathComponent(),
    withIntermediateDirectories: true
)
guard let png = rep.representation(using: .png, properties: [:]) else {
    fatalError("PNG encoding failed")
}
try png.write(to: out)
print(out.path)
