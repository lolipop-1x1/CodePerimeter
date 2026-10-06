import AppKit

// 仅用于重建匿名品牌图标，不属于通知 helper 的运行入口。
let destination = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: destination,
    withIntermediateDirectories: true, attributes: nil)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = points * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil,
            pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let size = CGFloat(pixels)
        NSColor(calibratedRed: 0.13, green: 0.15, blue: 0.16, alpha: 1).setFill()
        NSBezierPath(roundedRect: NSRect(x: size * 0.06, y: size * 0.06,
            width: size * 0.88, height: size * 0.88),
            xRadius: size * 0.19, yRadius: size * 0.19).fill()
        NSColor(calibratedRed: 0.55, green: 0.83, blue: 0.66, alpha: 1).setFill()
        let shield = NSBezierPath()
        shield.move(to: NSPoint(x: size * 0.5, y: size * 0.80))
        shield.line(to: NSPoint(x: size * 0.75, y: size * 0.70))
        shield.line(to: NSPoint(x: size * 0.73, y: size * 0.45))
        shield.curve(to: NSPoint(x: size * 0.5, y: size * 0.20),
            controlPoint1: NSPoint(x: size * 0.72, y: size * 0.31),
            controlPoint2: NSPoint(x: size * 0.59, y: size * 0.24))
        shield.curve(to: NSPoint(x: size * 0.27, y: size * 0.45),
            controlPoint1: NSPoint(x: size * 0.41, y: size * 0.24),
            controlPoint2: NSPoint(x: size * 0.28, y: size * 0.31))
        shield.line(to: NSPoint(x: size * 0.25, y: size * 0.70))
        shield.close()
        shield.fill()
        NSColor(calibratedRed: 0.13, green: 0.15, blue: 0.16, alpha: 1).setFill()
        NSBezierPath(roundedRect: NSRect(x: size * 0.47, y: size * 0.35,
            width: size * 0.06, height: size * 0.29),
            xRadius: size * 0.03, yRadius: size * 0.03).fill()
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        try bitmap.representation(using: .png, properties: [:])!
            .write(to: destination.appendingPathComponent("icon_\(points)x\(points)\(suffix).png"))
    }
}
