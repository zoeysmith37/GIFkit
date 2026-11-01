// Finder thumbnails of GIFkit projects: a Quick Look extension inside
// GIFkit.app. A project is a zip whose pictures are stored uncompressed, so
// the stored thumbnail (or, for older projects, the first frame) is read
// straight out of it.

import AppKit
import QuickLookThumbnailing

/// The bytes of the first of `names` stored (uncompressed) in the zip `d`.
func storedEntry(_ d: Data, _ names: [String]) -> Data? {
    let b = [UInt8](d)
    func u16(_ i: Int) -> Int { i + 2 <= b.count ? Int(b[i]) | Int(b[i + 1]) << 8 : 0 }
    func u32(_ i: Int) -> Int { u16(i) | u16(i + 2) << 16 }
    // the end record, near the end (after an optional comment)
    guard b.count >= 22 else { return nil }
    var end = -1
    var i = b.count - 22
    while i >= max(0, b.count - 22 - 65535) {
        if u32(i) == 0x06054b50 { end = i; break }
        i -= 1
    }
    guard end >= 0 else { return nil }
    let count = u16(end + 10)
    var p = u32(end + 16)
    var found: [String: (Int, Int)] = [:]
    for _ in 0..<count {
        guard p + 46 <= b.count, u32(p) == 0x02014b50 else { break }
        let method = u16(p + 10), size = u32(p + 20)
        let nlen = u16(p + 28), elen = u16(p + 30), clen = u16(p + 32)
        let local = u32(p + 42)
        if p + 46 + nlen <= b.count, let name = String(bytes: b[(p + 46)..<(p + 46 + nlen)], encoding: .utf8), method == 0 {
            found[name] = (local, size)
        }
        p += 46 + nlen + elen + clen
    }
    for n in names {
        guard let e = found[n] else { continue }
        let local = e.0, size = e.1
        guard local + 30 <= b.count, u32(local) == 0x04034b50 else { continue }
        let start = local + 30 + u16(local + 26) + u16(local + 28)
        if start + size <= b.count { return Data(b[start..<(start + size)]) }
    }
    return nil
}

class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(for request: QLFileThumbnailRequest, _ handler: @escaping (QLThumbnailReply?, Error?) -> Void) {
        guard let data = try? Data(contentsOf: request.fileURL, options: .mappedIfSafe),
              let pic = storedEntry(data, ["thumbnail.jpg", "thumbnail.png", "pics/00000.png", "frames/00000.png"]),
              let img = NSImage(data: pic), img.size.width > 0, img.size.height > 0 else {
            handler(nil, nil)
            return
        }
        let most = request.maximumSize
        let k = min(most.width / img.size.width, most.height / img.size.height, 1)
        let size = CGSize(width: max(1, (img.size.width * k).rounded()), height: max(1, (img.size.height * k).rounded()))
        handler(QLThumbnailReply(contextSize: size, currentContextDrawing: { () -> Bool in
            img.draw(in: CGRect(origin: .zero, size: size))
            return true
        }), nil)
    }
}
