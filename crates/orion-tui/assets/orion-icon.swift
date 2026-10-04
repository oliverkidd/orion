import AppKit

let size = 1024.0
let img = NSImage(size: NSSize(width: size, height: size))
func c(_ hex: UInt32, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 0xff)/255, green: CGFloat((hex >> 8) & 0xff)/255, blue: CGFloat(hex & 0xff)/255, alpha: a)
}
img.lockFocus()
let ctx = NSGraphicsContext.current!.cgContext
// macOS app-icon grid: 824pt rounded square centred in 1024.
let inset = 100.0
let tile = NSRect(x: inset, y: inset, width: size - 2*inset, height: size - 2*inset)
let shape = NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)
ctx.saveGState()
// soft drop shadow under the tile
ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: NSColor(white: 0, alpha: 0.45).cgColor)
c(0x0b0620).setFill(); shape.fill()
ctx.restoreGState()
ctx.saveGState()
shape.addClip()
// night-sky gradient: indigo top-left to near-black
NSGradient(colors: [c(0x2a1a5e), c(0x120a2e), c(0x05030f)], atLocations: [0, 0.55, 1], colorSpace: .sRGB)!
    .draw(in: shape, angle: -60)

let cx = size/2, cy = size/2 + 10, scale = 330.0
func pt(_ x: Double, _ y: Double) -> CGPoint { CGPoint(x: cx + x*scale, y: cy - y*scale) }
func glow(_ p: CGPoint, _ r: Double, _ col: NSColor) {
    let g = NSGradient(colors: [col, col.withAlphaComponent(0)])!
    g.draw(fromCenter: p, radius: 0, toCenter: p, radius: r, options: [])
}
// nebula: rose core under the belt (the sword), violet haze around
glow(pt(0.02, 0.38), 330, c(0x875fff, 0.22))
glow(pt(0.02, 0.42), 150, c(0xff87d7, 0.30))
glow(pt(-0.45, -0.6), 260, c(0x5f5fd7, 0.14))

// faint background starfield (deterministic)
var seed: UInt64 = 0x9e3779b97f4a7c15
func rnd() -> Double { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; return Double(seed % 10000) / 10000 }
for _ in 0..<70 {
    let p = CGPoint(x: inset + rnd()*(size-2*inset), y: inset + rnd()*(size-2*inset))
    let r = 1.5 + rnd()*2.5
    c(0xd7d7ff, 0.25 + rnd()*0.45).setFill()
    NSBezierPath(ovalIn: NSRect(x: p.x - r, y: p.y - r, width: 2*r, height: 2*r)).fill()
}

let stars: [(Double, Double, UInt32, Double)] = [
    (-0.56, -0.70, 0xff8787, 30), // Betelgeuse
    (0.50, -0.60, 0xd7d7ff, 22),  // Bellatrix
    (-0.30, 0.08, 0xffffff, 22),  // Alnitak
    (0.00, 0.03, 0xffffff, 24),   // Alnilam
    (0.30, -0.02, 0xffffff, 22),  // Mintaka
    (-0.44, 0.78, 0xafafff, 20),  // Saiph
    (0.54, 0.74, 0xafffff, 30),   // Rigel
]
let edges = [(0,1),(0,2),(1,4),(2,3),(3,4),(2,5),(4,6),(5,6)]
// sticks: violet→pink gradient strokes like the wordmark
for (a, b) in edges {
    let p = pt(stars[a].0, stars[a].1), q = pt(stars[b].0, stars[b].1)
    ctx.saveGState()
    let path = CGMutablePath(); path.move(to: p); path.addLine(to: q)
    ctx.addPath(path); ctx.setLineWidth(9); ctx.setLineCap(.round)
    ctx.replacePathWithStrokedPath(); ctx.clip()
    let grad = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [c(0x875fff, 0.85).cgColor, c(0xff87ff, 0.85).cgColor] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: p, end: q, options: [])
    ctx.restoreGState()
}
for (x, y, col, r) in stars {
    let p = pt(x, y)
    glow(p, r * 3.4, c(col, 0.55))
    c(col).setFill()
    NSBezierPath(ovalIn: NSRect(x: p.x - r*0.62, y: p.y - r*0.62, width: r*1.24, height: r*1.24)).fill()
    NSColor.white.setFill()
    NSBezierPath(ovalIn: NSRect(x: p.x - r*0.32, y: p.y - r*0.32, width: r*0.64, height: r*0.64)).fill()
}
ctx.restoreGState()
// thin inner rim
c(0xffffff, 0.10).setStroke(); shape.lineWidth = 3; shape.stroke()
img.unlockFocus()

let rep = NSBitmapImageRep(data: img.tiffRepresentation!)!
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
