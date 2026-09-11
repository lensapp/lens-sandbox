import AppKit

struct AppIcons {
    let menuBar: NSImage
    let dock: NSImage

    init(bundle: Bundle) throws {
        menuBar = try Self.load("lnsTemplate", extension: "png", bundle: bundle)
        menuBar.size = NSSize(width: 16, height: 16)
        menuBar.isTemplate = true
        dock = try Self.load("LNS", extension: "icns", bundle: bundle)
        dock.isTemplate = false
    }

    private static func load(_ name: String, extension suffix: String, bundle: Bundle) throws -> NSImage {
        guard let url = bundle.url(forResource: name, withExtension: suffix),
              let image = NSImage(contentsOf: url), image.isValid else {
            throw CocoaError(.fileReadCorruptFile, userInfo: [NSLocalizedDescriptionKey: "Cannot load \(name).\(suffix) from LNS.app. Rebuild the app bundle."])
        }
        return image
    }
}
