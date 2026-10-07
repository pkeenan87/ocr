// C ABI over Apple Vision's Swift-only RecognizeDocumentsRequest.
//
// Contract: each exported function blocks the calling thread until the async
// Vision work finishes. Call only from threads Rust owns, never from a Swift
// concurrency executor (blocking one of those can deadlock the pool).
// Returned strings are malloc'd JSON; release them with ocr_bridge_free.

import CoreGraphics
import Foundation
import Vision

private final class ResultBox: @unchecked Sendable {
    var json = ""
}

struct BlockOut: Encodable {
    /// "title" | "paragraph" | "list" | "table"
    let kind: String
    /// Normalized bounding box, origin at top-left: [x, y, w, h].
    let bbox: [Double]
    var text: String? = nil
    var items: [String]? = nil
    var rows: [[String]]? = nil
}

struct Options: Decodable {
    var languages: [String] = ["en-US"]
    var autoDetectLanguage = false
    var languageCorrection = true
    var customWords: [String] = []
}

struct PageOut: Encodable {
    var ok: Bool
    var error: String? = nil
    var blocks: [BlockOut] = []
}

private func topLeftBox(_ region: NormalizedRegion) -> [Double] {
    // Vision regions are normalized with a bottom-left origin.
    let r = region.normalizedPath.boundingBox
    return [r.minX, 1.0 - r.maxY, r.width, r.height]
}

private func overlaps(_ a: [Double], _ b: [Double]) -> Bool {
    let ix = max(0, min(a[0] + a[2], b[0] + b[2]) - max(a[0], b[0]))
    let iy = max(0, min(a[1] + a[3], b[1] + b[3]) - max(a[1], b[1]))
    let area = a[2] * a[3]
    return area > 0 && (ix * iy) / area > 0.5
}

private func recognize(path: String, options: Options) async -> PageOut {
    do {
        var request = RecognizeDocumentsRequest()
        request.textRecognitionOptions.recognitionLanguages = options.languages.map { Locale.Language(identifier: $0) }
        request.textRecognitionOptions.automaticallyDetectLanguage = options.autoDetectLanguage
        request.textRecognitionOptions.useLanguageCorrection = options.languageCorrection
        request.textRecognitionOptions.customWords = options.customWords
        let observations = try await request.perform(on: URL(fileURLWithPath: path))
        var blocks: [BlockOut] = []
        for obs in observations {
            let doc = obs.document
            var structured: [BlockOut] = []
            for table in doc.tables {
                let rows = table.rows.map { row in
                    row.map { $0.content.text.transcript }
                }
                structured.append(BlockOut(kind: "table", bbox: topLeftBox(table.boundingRegion), rows: rows))
            }
            for list in doc.lists {
                let items = list.items.map { $0.itemString }
                structured.append(BlockOut(kind: "list", bbox: topLeftBox(list.boundingRegion), items: items))
            }
            if let title = doc.title {
                blocks.append(BlockOut(kind: "title", bbox: topLeftBox(title.boundingRegion), text: title.transcript))
            }
            // Paragraphs can repeat text already captured inside tables/lists; drop those.
            for para in doc.paragraphs {
                let box = topLeftBox(para.boundingRegion)
                if structured.contains(where: { overlaps(box, $0.bbox) }) { continue }
                if let t = doc.title, para.transcript == t.transcript { continue }
                blocks.append(BlockOut(kind: "paragraph", bbox: box, text: para.transcript))
            }
            blocks.append(contentsOf: structured)
        }
        // Reading order: top-to-bottom, then left-to-right.
        blocks.sort { ($0.bbox[1], $0.bbox[0]) < ($1.bbox[1], $1.bbox[0]) }
        return PageOut(ok: true, blocks: blocks)
    } catch {
        return PageOut(ok: false, error: String(describing: error))
    }
}

@_cdecl("ocr_bridge_recognize_document")
public func ocr_bridge_recognize_document(
    _ path: UnsafePointer<CChar>, _ optionsJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    let options = optionsJSON
        .flatMap { try? JSONDecoder().decode(Options.self, from: Data(String(cString: $0).utf8)) } ?? Options()
    let box = ResultBox()
    let done = DispatchSemaphore(value: 0)
    Task.detached {
        let page = await recognize(path: p, options: options)
        let data = (try? JSONEncoder().encode(page)) ?? Data(#"{"ok":false,"error":"encode failed"}"#.utf8)
        box.json = String(decoding: data, as: UTF8.self)
        done.signal()
    }
    done.wait()
    return strdup(box.json)
}

@_cdecl("ocr_bridge_free")
public func ocr_bridge_free(_ ptr: UnsafeMutablePointer<CChar>?) {
    free(ptr)
}
