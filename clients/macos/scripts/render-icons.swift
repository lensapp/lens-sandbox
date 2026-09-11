import AppKit

func renderIcon(_ logo: NSImage, pixels: Int) throws -> Data {
    guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
        let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw CocoaError(.coderInvalidValue)
    }
    NSGraphicsContext.saveGraphicsState()
    defer { NSGraphicsContext.restoreGraphicsState() }
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    let edge = CGFloat(pixels)
    let bounds = NSRect(x: 0, y: 0, width: edge, height: edge)
    context.cgContext.clear(bounds)
    NSColor(calibratedWhite: 0.96, alpha: 1).setFill()
    NSBezierPath(roundedRect: bounds.insetBy(dx: edge * 0.06, dy: edge * 0.06),
        xRadius: edge * 0.19, yRadius: edge * 0.19).fill()
    logo.draw(in: bounds.insetBy(dx: edge * 0.18, dy: edge * 0.18),
        from: .zero, operation: .sourceOver, fraction: 1)
    context.flushGraphics()
    guard let png = bitmap.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
    return png
}

guard CommandLine.arguments.count == 3,
      let logo = NSImage(contentsOfFile: CommandLine.arguments[1]), logo.isValid else {
    throw CocoaError(.fileReadCorruptFile)
}
logo.isTemplate = false
let output = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let suffix = scale == 2 ? "@2x" : ""
        let name = "icon_\(points)x\(points)\(suffix).png"
        try renderIcon(logo, pixels: points * scale).write(to: output.appendingPathComponent(name))
    }
}
