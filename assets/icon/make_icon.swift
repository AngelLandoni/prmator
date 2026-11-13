// Draws PRmator's app icon (1024×1024 PNG). Flat and precise, like bubblegum's: a matte
// black tile, one crisp white line mark, a single red accent. Several directions can be
// rendered; `scripts/make-app.sh` uses the default one.
//
//   swift assets/icon/make_icon.swift OUTPUT.png [pr|diff|review]   (pr is the icon)

import AppKit

let size: CGFloat = 1024
let args = CommandLine.arguments.dropFirst()
let output = args.first ?? "icon.png"
let variant = args.dropFirst().first ?? "pr"

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha)
}

let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(
    data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
    space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

let white: UInt32 = 0xf2f2f2
let red: UInt32 = 0xe31937
let weight: CGFloat = 30

// macOS icon grid: an 824pt rounded square centered on a 1024pt canvas (y grows up).
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)

func drawTile() {
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: color(0x000000, 0.35))
    ctx.addPath(tilePath)
    ctx.setFillColor(color(0x0a0a0b))
    ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(tilePath)
    ctx.clip()
    let g = CGGradient(
        colorsSpace: colorSpace, colors: [color(0x1c1c1f), color(0x0a0a0b)] as CFArray,
        locations: [0, 1])!
    ctx.drawLinearGradient(
        g, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
    ctx.restoreGState()
}

func circle(_ c: CGPoint, _ r: CGFloat) -> CGRect {
    CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)
}

func fill(_ path: CGPath, _ hex: UInt32) {
    ctx.addPath(path)
    ctx.setFillColor(color(hex))
    ctx.fillPath()
}

func stroke(_ path: CGPath, _ hex: UInt32, cap: CGLineCap = .round) {
    fill(path.copy(strokingWithWidth: weight, lineCap: cap, lineJoin: .round, miterLimit: 10), hex)
}

func ring(_ c: CGPoint, _ r: CGFloat, _ hex: UInt32) {
    stroke(CGPath(ellipseIn: circle(c, r), transform: nil), hex, cap: .butt)
}

func line(_ points: [CGPoint]) -> CGPath {
    let p = CGMutablePath()
    p.addLines(between: points)
    return p
}

drawTile()

switch variant {
case "diff":
    // A diff, reduced: lines of code, one of them changed (red marker in the gutter).
    let rows: [(CGFloat, CGFloat)] = [(660, 300), (580, 220), (500, 340), (420, 260), (340, 180)]
    for (i, (y, w)) in rows.enumerated() {
        stroke(line([CGPoint(x: 400, y: y), CGPoint(x: 400 + w, y: y)]), i == 2 ? red : white)
    }
    stroke(line([CGPoint(x: 300, y: 500), CGPoint(x: 340, y: 500)]), red)
case "review":
    // A comment bubble with a +/− diff inside: review on a change.
    // One outline: rounded box, the tail cut into its bottom-left edge.
    let bubble = CGMutablePath()
    let (x0, x1, y0, y1, cr): (CGFloat, CGFloat, CGFloat, CGFloat, CGFloat) = (262, 762, 362, 742, 90)
    bubble.move(to: CGPoint(x: 452, y: y0))
    bubble.addArc(tangent1End: CGPoint(x: x1, y: y0), tangent2End: CGPoint(x: x1, y: y1), radius: cr)
    bubble.addArc(tangent1End: CGPoint(x: x1, y: y1), tangent2End: CGPoint(x: x0, y: y1), radius: cr)
    bubble.addArc(tangent1End: CGPoint(x: x0, y: y1), tangent2End: CGPoint(x: x0, y: y0), radius: cr)
    bubble.addArc(tangent1End: CGPoint(x: x0, y: y0), tangent2End: CGPoint(x: x1, y: y0), radius: cr)
    bubble.addLine(to: CGPoint(x: 362, y: y0))
    bubble.addLine(to: CGPoint(x: 322, y: 272))
    bubble.closeSubpath()
    stroke(bubble, white)
    stroke(line([CGPoint(x: 362, y: 600), CGPoint(x: 442, y: 600)]), red)
    stroke(line([CGPoint(x: 362, y: 500), CGPoint(x: 442, y: 500)]), white)
    stroke(line([CGPoint(x: 402, y: 460), CGPoint(x: 402, y: 540)]), white)
    stroke(line([CGPoint(x: 500, y: 600), CGPoint(x: 662, y: 600)]), white)
    stroke(line([CGPoint(x: 500, y: 500), CGPoint(x: 622, y: 500)]), white)
default:
    // "pr": the pull request mark, reduced. The base branch on the left; on the right the
    // change, coming back over to it, with its commit in red.
    let r: CGFloat = 58
    let left = CGPoint(x: 372, y: 0), right = CGPoint(x: 652, y: 0)
    let top: CGFloat = 690, bottom: CGFloat = 334
    ring(CGPoint(x: left.x, y: top), r, white)
    ring(CGPoint(x: left.x, y: bottom), r, white)
    stroke(line([CGPoint(x: left.x, y: top - r - weight / 2), CGPoint(x: left.x, y: bottom + r + weight / 2)]), white, cap: .butt)

    ring(CGPoint(x: right.x, y: bottom), r, red)
    let path = CGMutablePath()
    path.move(to: CGPoint(x: right.x, y: bottom + r + weight / 2))
    path.addLine(to: CGPoint(x: right.x, y: top - 70))
    path.addArc(tangent1End: CGPoint(x: right.x, y: top), tangent2End: CGPoint(x: right.x - 70, y: top), radius: 70)
    path.addLine(to: CGPoint(x: 492, y: top))
    stroke(path, white, cap: .butt)
    // Arrowhead pointing back at the base branch.
    stroke(line([CGPoint(x: 552, y: top + 62), CGPoint(x: 490, y: top), CGPoint(x: 552, y: top - 62)]), white)
}

let image = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: image)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")
