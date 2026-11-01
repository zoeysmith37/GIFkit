// Reads pictures straight out of a GIFkit project (a zip whose pictures are
// stored uncompressed). Shared by the Quick Look extension and its test.

import Foundation

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
