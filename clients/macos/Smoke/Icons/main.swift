import AppKit

func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw NSError(domain: "LNSIconSmoke", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
}

func raster(_ image: NSImage, pixels: Int) throws -> NSBitmapImageRep {
    var bounds = NSRect(x: 0, y: 0, width: CGFloat(pixels), height: CGFloat(pixels))
    guard let cgImage = image.cgImage(forProposedRect: &bounds, context: nil, hints: nil) else {
        throw NSError(domain: "LNSIconSmoke", code: 2, userInfo: [NSLocalizedDescriptionKey: "AppKit cannot render the icon"])
    }
    return NSBitmapImageRep(cgImage: cgImage)
}

do {
    let icons = try AppIcons(bundle: .main)
    try require(icons.menuBar.isTemplate && icons.menuBar.size == NSSize(width: 16, height: 16), "Menu-bar icon must be a 16-point template")
    let template = try raster(icons.menuBar, pixels: 32)
    var ink = 0
    var clear = 0
    for y in 0..<template.pixelsHigh {
        for x in 0..<template.pixelsWide {
            guard let color = template.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { throw CocoaError(.coderInvalidValue) }
            if color.alphaComponent > 0.9 { ink += 1 }
            if color.alphaComponent < 0.1 { clear += 1 }
        }
    }
    try require(ink > 100 && clear > 100, "Menu-bar icon lost its visible logo or transparent background")
    try require(!icons.dock.isTemplate, "Dock icon must retain its background")
    let dock = try raster(icons.dock, pixels: 1024)
    try require(dock.pixelsWide == 1024 && dock.pixelsHigh == 1024, "Dock icon has no Retina representation")
    var dark = 0
    var light = 0
    for y in stride(from: 0, to: dock.pixelsHigh, by: 8) {
        for x in stride(from: 0, to: dock.pixelsWide, by: 8) {
            guard let color = dock.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { throw CocoaError(.coderInvalidValue) }
            guard color.alphaComponent > 0.9 else { continue }
            try require(abs(color.redComponent - color.greenComponent) < 0.03 && abs(color.blueComponent - color.greenComponent) < 0.03,
                "Dock icon contains unexpected colored pixels")
            if color.redComponent < 0.1 { dark += 1 }
            if color.redComponent > 0.9 { light += 1 }
        }
    }
    try require(dark > 1000 && light > 1000, "Dock icon must show the LNS mark on a light background")
    try require((dock.colorAt(x: 0, y: 0)?.alphaComponent ?? 1) < 0.1, "Dock icon lost its transparent corners")
    print("PASS: relocated app loads and renders the original template and Retina Dock icon")
} catch {
    FileHandle.standardError.write(Data("FAIL: \(error.localizedDescription)\n".utf8))
    exit(1)
}
