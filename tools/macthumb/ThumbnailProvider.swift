// Finder thumbnails of GIFkit projects: a Quick Look extension inside
// GIFkit.app. The stored thumbnail (or, for older projects, the first
// frame) is read straight out of the project (see Zip.swift).

import AppKit
import QuickLookThumbnailing

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
