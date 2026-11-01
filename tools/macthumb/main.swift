// swiftc Zip.swift main.swift -o ziptest && ./ziptest project.gifkit
import Foundation

let path = CommandLine.arguments[1]
guard let d = FileManager.default.contents(atPath: path),
      let pic = storedEntry(d, ["thumbnail.jpg", "thumbnail.png", "pics/00000.png", "frames/00000.png"]) else {
    print("no picture found in \(path)")
    exit(1)
}
let jpeg = pic.starts(with: [0xFF, 0xD8, 0xFF])
let png = pic.starts(with: [0x89, 0x50, 0x4E, 0x47])
print("found a \(jpeg ? "JPEG" : png ? "PNG" : "?") of \(pic.count) bytes")
exit(jpeg || png ? 0 : 1)
