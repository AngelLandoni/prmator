// Draws PRmator's app icon (1024×1024 PNG): a diff card with removed/added lines on an
// indigo→violet squircle, with a Claude-style sparkle.
//
//   swift assets/icon/make_icon.swift assets/icon/icon.png

import AppKit
import CoreGraphics

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(
        red: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: a
    )
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(
    data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
)!
// Flip so y grows downward, like a design tool.
ctx.translateBy(x: 0, y: size)
ctx.scaleBy(x: 1, y: -1)

func gradient(_ colors: [CGColor], _ locations: [CGFloat]) -> CGGradient {
    CGGradient(colorsSpace: space, colors: colors as CFArray, locations: locations)!
}

// --- Squircle background (macOS icon grid: 824pt body, 100pt margin).
let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let squircle = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

// Drop shadow.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: 18), blur: 40, color: rgb(0x000000, 0.35))
ctx.addPath(squircle)
ctx.setFillColor(rgb(0x1e1b4b))
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(squircle)
ctx.clip()
ctx.drawLinearGradient(
    gradient([rgb(0x312e81), rgb(0x4f46e5), rgb(0x7c3aed)], [0, 0.55, 1]),
    start: CGPoint(x: 100, y: 100), end: CGPoint(x: 924, y: 924), options: []
)
// Soft glow behind the sparkle.
ctx.drawRadialGradient(
    gradient([rgb(0xfbbf24, 0.45), rgb(0xfbbf24, 0)], [0, 1]),
    startCenter: CGPoint(x: 770, y: 250), startRadius: 0,
    endCenter: CGPoint(x: 770, y: 250), endRadius: 330, options: []
)
// Top sheen.
ctx.drawLinearGradient(
    gradient([rgb(0xffffff, 0.10), rgb(0xffffff, 0)], [0, 1]),
    start: CGPoint(x: 0, y: 100), end: CGPoint(x: 0, y: 470), options: []
)
ctx.restoreGState()

// --- Diff card.
let card = CGRect(x: 196, y: 300, width: 560, height: 480)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: 22), blur: 44, color: rgb(0x0f0a2e, 0.55))
ctx.addPath(CGPath(roundedRect: card, cornerWidth: 54, cornerHeight: 54, transform: nil))
ctx.setFillColor(rgb(0xffffff, 0.97))
ctx.fillPath()
ctx.restoreGState()

// Card title bar dots.
for (i, c) in [0xff5f57, 0xfebc2e, 0x28c840].enumerated() {
    ctx.setFillColor(rgb(UInt32(c)))
    ctx.fillEllipse(in: CGRect(x: 240 + CGFloat(i) * 40, y: 340, width: 24, height: 24))
}

// Diff rows: (kind, code bar width). Kind: 0 context, -1 removed, 1 added.
let rows: [(Int, CGFloat)] = [(0, 300), (-1, 360), (1, 400), (1, 250), (0, 330)]
let rowHeight: CGFloat = 64
for (i, (kind, width)) in rows.enumerated() {
    let y = 398 + CGFloat(i) * (rowHeight + 10)
    let rowRect = CGRect(x: 224, y: y, width: 504, height: rowHeight)
    if kind != 0 {
        ctx.addPath(CGPath(roundedRect: rowRect, cornerWidth: 16, cornerHeight: 16, transform: nil))
        ctx.setFillColor(kind > 0 ? rgb(0xdafbe1) : rgb(0xffebe9))
        ctx.fillPath()
    }
    // +/− sign (unchanged lines have none).
    let cx: CGFloat = 262, cy = y + rowHeight / 2
    if kind != 0 {
        ctx.setFillColor(kind > 0 ? rgb(0x1a7f37) : rgb(0xcf222e))
        ctx.fill(CGRect(x: cx - 15, y: cy - 5, width: 30, height: 10))
    }
    if kind > 0 {
        ctx.fill(CGRect(x: cx - 5, y: cy - 15, width: 10, height: 30))
    }
    // Code bar(s).
    let barColor = kind > 0 ? rgb(0x2da44e, 0.75) : kind < 0 ? rgb(0xe5534b, 0.7) : rgb(0xb8bfcc)
    ctx.setFillColor(barColor)
    let bar = CGRect(x: 304, y: cy - 11, width: width * 0.45, height: 22)
    ctx.addPath(CGPath(roundedRect: bar, cornerWidth: 11, cornerHeight: 11, transform: nil))
    ctx.fillPath()
    let bar2 = CGRect(x: 304 + width * 0.45 + 18, y: cy - 11, width: width * 0.4, height: 22)
    ctx.setFillColor(kind == 0 ? rgb(0xd5d9e2) : barColor.copy(alpha: 0.45)!)
    ctx.addPath(CGPath(roundedRect: bar2, cornerWidth: 11, cornerHeight: 11, transform: nil))
    ctx.fillPath()
}

// --- Sparkle (four-pointed star) overlapping the card's top-right corner.
func sparkle(center c: CGPoint, radius r: CGFloat, waist w: CGFloat) -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: c.x, y: c.y - r))
    p.addQuadCurve(to: CGPoint(x: c.x + r, y: c.y), control: CGPoint(x: c.x + w, y: c.y - w))
    p.addQuadCurve(to: CGPoint(x: c.x, y: c.y + r), control: CGPoint(x: c.x + w, y: c.y + w))
    p.addQuadCurve(to: CGPoint(x: c.x - r, y: c.y), control: CGPoint(x: c.x - w, y: c.y + w))
    p.addQuadCurve(to: CGPoint(x: c.x, y: c.y - r), control: CGPoint(x: c.x - w, y: c.y - w))
    p.closeSubpath()
    return p
}

let star = sparkle(center: CGPoint(x: 752, y: 292), radius: 128, waist: 22)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: 10), blur: 30, color: rgb(0xb45309, 0.6))
ctx.addPath(star)
ctx.setFillColor(rgb(0xf59e0b))
ctx.fillPath()
ctx.restoreGState()
ctx.saveGState()
ctx.addPath(star)
ctx.clip()
ctx.drawLinearGradient(
    gradient([rgb(0xfde68a), rgb(0xf59e0b), rgb(0xea580c)], [0, 0.5, 1]),
    start: CGPoint(x: 680, y: 170), end: CGPoint(x: 830, y: 420), options: []
)
ctx.restoreGState()
// Small companion sparkle.
ctx.addPath(sparkle(center: CGPoint(x: 862, y: 430), radius: 44, waist: 8))
ctx.setFillColor(rgb(0xfde68a, 0.95))
ctx.fillPath()

// --- Write PNG.
let image = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: image)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
