#!/usr/bin/env swift
// Slice a pixel-art 4×2 state sheet into APNGs on an integer 128px art grid.
// Usage: swift scripts/prepare-character-pets.swift <sheet.png> <pack-directory>
import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let states = ["idle", "working", "thinking", "interacting", "celebrating", "alarmed", "sleeping", "watching"]
guard CommandLine.arguments.count == 3 else { fatalError("Expected sheet and pack directory") }
let sheetURL = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
guard let source = CGImageSourceCreateWithURL(sheetURL as CFURL, nil),
      let sheet = CGImageSourceCreateImageAtIndex(source, 0, nil) else { fatalError("Cannot read sheet") }
let colorSpace = CGColorSpaceCreateDeviceRGB()

func context(_ width: Int, _ height: Int) -> CGContext {
    CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
              bytesPerRow: width * 4, space: colorSpace,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
}

// Discard disconnected generation specks; keep the complete connected figure,
// including its original semitransparent silhouette and fine fur.
func figure(_ image: CGImage) -> CGImage {
    let w = image.width, h = image.height
    let ctx = context(w, h)
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    let bytes = ctx.data!.assumingMemoryBound(to: UInt8.self)
    var visited = [Bool](repeating: false, count: w * h)
    var largest = [Int]()
    for start in 0..<(w * h) where !visited[start] && bytes[start * 4 + 3] > 0 {
        var queue = [start], head = 0
        visited[start] = true
        while head < queue.count {
            let pixel = queue[head]; head += 1
            let x = pixel % w, y = pixel / w
            for dy in -1...1 { for dx in -1...1 {
                let nx = x + dx, ny = y + dy
                if nx < 0 || nx >= w || ny < 0 || ny >= h { continue }
                let next = ny * w + nx
                if !visited[next] && bytes[next * 4 + 3] > 0 {
                    visited[next] = true
                    queue.append(next)
                }
            } }
        }
        if queue.count > largest.count { largest = queue }
    }
    precondition(!largest.isEmpty, "Empty sprite")
    let keep = Set(largest)
    var minX = w, minY = h, maxX = 0, maxY = 0
    for pixel in 0..<(w * h) {
        if !keep.contains(pixel) {
            for channel in 0..<4 { bytes[pixel * 4 + channel] = 0 }
        } else {
            minX = min(minX, pixel % w); maxX = max(maxX, pixel % w)
            minY = min(minY, pixel / w); maxY = max(maxY, pixel / w)
        }
    }
    return ctx.makeImage()!.cropping(to: CGRect(x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1))!
}

let figures = states.indices.map { index -> CGImage in
    let col = index % 4, row = index / 4
    let x = col * sheet.width / 4, y = row * sheet.height / 2
    let right = (col + 1) * sheet.width / 4, bottom = (row + 1) * sheet.height / 2
    return figure(sheet.cropping(to: CGRect(x: x, y: y, width: right - x, height: bottom - y))!)
}
let extent = figures.map { max($0.width, $0.height) }.max()!
let scale = 110.0 / Double(extent)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
for (index, sprite) in figures.enumerated() {
    let url = output.appendingPathComponent(states[index] + ".png")
    let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 2, nil)!
    CGImageDestinationSetProperties(destination, [kCGImagePropertyPNGDictionary: [kCGImagePropertyAPNGLoopCount: 0]] as CFDictionary)
    for breath in [0.0, 1.0] {
        // Compose on the native grid, then enlarge by exactly 4×. A one-row
        // breath preserves square pixels instead of introducing smooth scaling.
        let art = context(128, 128)
        art.interpolationQuality = .none
        let w = (Double(sprite.width) * scale).rounded()
        let h = (Double(sprite.height) * scale).rounded() - breath
        art.draw(sprite, in: CGRect(x: ((128 - w) / 2).rounded(), y: 8, width: w, height: h))
        let ctx = context(512, 512)
        ctx.interpolationQuality = .none
        ctx.draw(art.makeImage()!, in: CGRect(x: 0, y: 0, width: 512, height: 512))
        CGImageDestinationAddImage(destination, ctx.makeImage()!, [
            kCGImagePropertyPNGDictionary: [kCGImagePropertyAPNGDelayTime: 0.9]
        ] as CFDictionary)
    }
    precondition(CGImageDestinationFinalize(destination), "Cannot write APNG")
    print("\(url.lastPathComponent): 512×512, 2 frames")
}
