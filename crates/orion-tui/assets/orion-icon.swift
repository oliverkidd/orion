// orion's icon — Orion.app's, and so its notifications' — drawn the way the splash
// draws its sky (src/splash.rs): the hunter's seven stars as flat dots on
// dotted sticks, the sword and M42's rose cross, a grain of dust along the
// belt and round Barnard's Loop, a sparse starfield, all in shades of one
// accent on black. No glow and no gradient. The small sizes are drawn for
// themselves, not scaled down: under 128 px the dust goes, at 32 and under
// the sticks go and the stars grow.
//
//   swift orion-icon.swift orion.iconset && iconutil -c icns orion.iconset -o orion.icns
//   swift orion-icon.swift preview.png 256        one size, to look at

import AppKit

typealias RGB = (CGFloat, CGFloat, CGFloat)
func col(_ c: RGB) -> NSColor { NSColor(srgbRed: c.0/255, green: c.1/255, blue: c.2/255, alpha: 1) }
func mix(_ a: RGB, _ b: RGB, _ u: CGFloat) -> RGB { (a.0+(b.0-a.0)*u, a.1+(b.1-a.1)*u, a.2+(b.2-a.2)*u) }
func shade(_ c: RGB, _ k: CGFloat) -> RGB { (min(c.0*k, 255), min(c.1*k, 255), min(c.2*k, 255)) }

let white: RGB = (255, 255, 255), gray: RGB = (128, 128, 128)
let accent: RGB = (128, 168, 220)
let betelgeuse: RGB = (255, 178, 140), rigel: RGB = (190, 220, 255), m42: RGB = (225, 150, 190)
let stick = shade(mix(accent, gray, 0.25), 0.8)
let rim: RGB = (58, 62, 70)

// (x, y, bright, colour) — splash.rs STARS: (0, 0) is the belt, +y down.
let stars: [(CGFloat, CGFloat, Bool, RGB)] = [
    (-0.56, -0.70, true, betelgeuse),
    (0.50, -0.60, true, mix(accent, white, 0.6)),
    (-0.30, 0.08, true, mix(accent, white, 0.78)),
    (0.00, 0.03, true, mix(accent, white, 0.78)),
    (0.30, -0.02, false, mix(accent, white, 0.78)),
    (-0.44, 0.78, false, mix(accent, white, 0.45)),
    (0.54, 0.74, true, rigel),
]
let edges = [(0, 1), (0, 2), (1, 4), (2, 3), (3, 4), (2, 5), (4, 6), (5, 6)]
let sword: [(CGFloat, CGFloat)] = [(-0.30, 0.08), (-0.08, 0.22), (-0.06, 0.40), (-0.04, 0.56)]

func hash(_ x: Int, _ y: Int, _ salt: UInt32) -> UInt32 {
    var h = UInt32(truncatingIfNeeded: x) &* 374_761_393 ^ UInt32(truncatingIfNeeded: y) &* 668_265_263 ^ salt &* 2_246_822_519
    h = (h ^ (h >> 13)) &* 1_274_126_177
    return h ^ (h >> 16)
}
func hash01(_ x: Int, _ y: Int, _ salt: UInt32) -> CGFloat { CGFloat(hash(x, y, salt) & 0xffff) / 65535 }
func vnoise(_ x: CGFloat, _ y: CGFloat) -> CGFloat {
    let xi = Int(floor(x)), yi = Int(floor(y)), fx = x - floor(x), fy = y - floor(y)
    let sx = fx*fx*(3 - 2*fx), sy = fy*fy*(3 - 2*fy)
    let a = hash01(xi, yi, 991), b = hash01(xi + 1, yi, 991), c = hash01(xi, yi + 1, 991), d = hash01(xi + 1, yi + 1, 991)
    return a + (b - a)*sx + (c - a)*sy + (a - b - c + d)*sx*sy
}
/// splash.rs `density`: the cloud along the belt and sword, Barnard's Loop round them.
func density(_ x: CGFloat, _ y: CGFloat) -> CGFloat {
    let r = max(sqrt(x*x + y*y), 0.001)
    let belt = exp(-(x*x*2.4 + (y - 0.04)*(y - 0.04)*16))
    let swd = exp(-((x + 0.07)*(x + 0.07)*18 + (y - 0.38)*(y - 0.38)*7.5))
    let loopR = sqrt((x*1.05)*(x*1.05) + (y*0.88)*(y*0.88))
    let barnard = exp(-pow((loopR - 0.84)*5, 2))*0.36
    let core = exp(-r*r*9)*0.12
    let wisp = 0.55 + 0.45*vnoise(x*3 + 7, y*3 + 3)
    return wisp*((belt*0.75 + swd*0.7 + core)*0.17 + barnard)
}

func square(_ p: CGPoint, _ side: CGFloat, _ c: RGB) {
    col(c).setFill()
    NSRect(x: p.x - side/2, y: p.y - side/2, width: side, height: side).fill()
}
func dot(_ p: CGPoint, _ r: CGFloat, _ c: RGB) {
    col(c).setFill()
    NSBezierPath(ovalIn: NSRect(x: p.x - r, y: p.y - r, width: 2*r, height: 2*r)).fill()
}

func draw(_ s: CGFloat) {
    let u = s/1024, px = Int(s)
    let inset = 100*u
    let tile = NSBezierPath(roundedRect: NSRect(x: inset, y: inset, width: s - 2*inset, height: s - 2*inset),
                            xRadius: 185*u, yRadius: 185*u)
    NSColor.black.setFill(); tile.fill()
    NSGraphicsContext.saveGraphicsState()
    tile.addClip()
    let c = CGPoint(x: s/2, y: s/2 - 10*u), k = 356*u
    func at(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: c.x + x*k, y: c.y - y*k) }

    if px >= 128 {
        // The sky on the splash's own grid: a cell twice as tall as wide.
        let cw = 17*u, ch = 34*u
        let cols = Int((s/2)/cw) + 1, rows = Int((s/2)/ch) + 1
        for gy in -rows...rows {
            for gx in -cols...cols {
                let p = CGPoint(x: c.x + CGFloat(gx)*cw, y: c.y - CGFloat(gy)*ch)
                let d = density(CGFloat(gx)*cw/(k*1.07), CGFloat(gy)*ch/(k*1.07))
                if d >= 0.05 {
                    let v = (d - 0.05)/0.95
                    if hash01(gx, gy, 4242) < 1 - 0.7*(1 - min(v*2.2, 1)) {
                        let level: CGFloat = d >= 0.254 ? 0.34 : d >= 0.118 ? 0.26 : 0.19
                        square(p, (d >= 0.254 ? 6.5 : 5)*u, shade(accent, level))
                    }
                    continue
                }
                let h = hash(gx, gy, 12_345)
                if h % 90 == 0 {
                    square(p, 6.5*u, shade(mix(accent, white, 0.6), 0.6))
                } else if hash(gx, gy, 777) % 44 == 0 {
                    square(p, 5*u, shade(accent, 0.4))
                }
            }
        }
    }
    func pt(_ i: Int) -> CGPoint { at(stars[i].0, stars[i].1) }
    if px > 32 {
        var lines = edges.map { (pt($0.0), pt($0.1)) }
        for i in 1..<sword.count { lines.append((at(sword[i-1].0, sword[i-1].1), at(sword[i].0, sword[i].1))) }
        let step = (px >= 128 ? 34 : 54)*u, side = (px >= 128 ? 9 : 16)*u
        for (p, q) in lines {
            let n = max(Int(hypot(q.x - p.x, q.y - p.y)/step), 2)
            for i in 1..<n {
                let t = CGFloat(i)/CGFloat(n)
                square(CGPoint(x: p.x + (q.x - p.x)*t, y: p.y + (q.y - p.y)*t), side, stick)
            }
        }
        // M42, the fuzzy middle of the sword.
        let m = at(sword[2].0, sword[2].1), arm = (px >= 128 ? 15 : 22)*u, w = (px >= 128 ? 6 : 9)*u
        col(m42).setFill()
        NSRect(x: m.x - arm, y: m.y - w/2, width: 2*arm, height: w).fill()
        NSRect(x: m.x - w/2, y: m.y - arm, width: w, height: 2*arm).fill()
    }
    let grow: CGFloat = px <= 16 ? 2.0 : px <= 32 ? 1.6 : px < 128 ? 1.25 : 1
    for (i, st) in stars.enumerated() { dot(pt(i), (st.2 ? 32 : 21)*u*grow, st.3) }
    NSGraphicsContext.restoreGraphicsState()
    col(rim).setStroke(); tile.lineWidth = max(6*u, 0.5); tile.stroke()
}

func render(_ size: Int, _ out: String) {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    draw(CGFloat(size))
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
}

let args = CommandLine.arguments
if args[1].hasSuffix(".iconset") {
    try! FileManager.default.createDirectory(atPath: args[1], withIntermediateDirectories: true)
    for pt in [16, 32, 128, 256, 512] {
        render(pt, "\(args[1])/icon_\(pt)x\(pt).png")
        render(pt*2, "\(args[1])/icon_\(pt)x\(pt)@2x.png")
    }
} else {
    render(args.count > 2 ? Int(args[2])! : 1024, args[1])
}
