// C ABI over Apple Vision's Swift-only RecognizeDocumentsRequest.
//
// Every function is stateless: it takes a path and opens the document itself, so calls are
// thread-safe and no PDFKit/CoreGraphics object is shared between calls.
//
// Contract: each exported function blocks the calling thread until the async
// Vision work finishes. Call only from threads Rust owns, never from a Swift
// concurrency executor (blocking one of those can deadlock the pool).
// Returned strings are malloc'd JSON; release them with ocr_bridge_free.

import CoreGraphics
import Foundation
import ImageIO
import PDFKit
import Vision

private final class ResultBox: @unchecked Sendable {
    var json = ""
}

struct ListItemOut: Encodable {
    let marker: String
    let text: String
}

struct BlockOut: Encodable {
    /// "title" | "paragraph" | "list" | "table"
    let kind: String
    /// Normalized bounding box, origin at top-left: [x, y, w, h].
    var bbox: [Double]
    var text: String? = nil
    var items: [ListItemOut]? = nil
    var rows: [[String]]? = nil
}

struct LineOut: Encodable {
    let text: String
    /// Normalized bounding box, origin at top-left: [x, y, w, h].
    var bbox: [Double]
    /// `reader: "text"` only: Vision's confidence in `text`, and its next-best readings.
    var confidence: Double? = nil
    var alternatives: [String]? = nil
}

struct Options: Decodable {
    var languages: [String] = ["en-US"]
    var autoDetectLanguage = false
    var languageCorrection = true
    var customWords: [String] = []
    /// Optional normalized crop [x, y, w, h], origin top-left, applied to the rendered page.
    var crop: [Double]? = nil
    /// Also return Vision's line-level observations (`lines`); off by default. Optional so a
    /// request without the key still decodes (a failed decode would silently reset every option).
    var lines: Bool? = nil
    /// `"text"`: read lines with `RecognizeTextRequest` (line-level, with confidence and
    /// alternatives) instead of `RecognizeDocumentsRequest`. Used as a second reading.
    var reader: String? = nil
}

struct PageOut: Encodable {
    var ok: Bool
    var error: String? = nil
    var width: Int = 0
    var height: Int = 0
    var blocks: [BlockOut] = []
    /// Physical text lines; filled only when `Options.lines` is set.
    var lines: [LineOut] = []
}

struct InfoOut: Encodable {
    var ok: Bool
    var error: String? = nil
    var kind: String? = nil
    var pages: Int = 0
}

struct TextOut: Encodable {
    var ok: Bool
    var error: String? = nil
    var text: String = ""
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

private func isPDF(_ path: String) -> Bool {
    guard let h = try? FileHandle(forReadingFrom: URL(fileURLWithPath: path)) else { return false }
    defer { try? h.close() }
    return (try? h.read(upToCount: 5)) == Data("%PDF-".utf8)
}

private struct BridgeError: Error, CustomStringConvertible {
    let description: String
}

private func json<T: Encodable>(_ value: T) -> UnsafeMutablePointer<CChar>? {
    let data = (try? JSONEncoder().encode(value)) ?? Data(#"{"ok":false,"error":"encode failed"}"#.utf8)
    return strdup(String(decoding: data, as: UTF8.self))
}

/// Largest bitmap side we will render, to bound memory on oversized page boxes.
private let maxSide = 16384.0

private func bitmapContext(width: Int, height: Int) throws -> CGContext {
    guard let ctx = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else { throw BridgeError(description: "cannot create \(width)x\(height) bitmap") }
    ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    ctx.interpolationQuality = .high
    return ctx
}

/// `targetSide`, if set, replaces `dpi`: the page is scaled so the longest side of `crop` (a
/// normalized [x, y, w, h], or the whole page) comes out at that many pixels.
private func renderPDFPage(
    path: String, index: Int, dpi: Double, targetSide: Double? = nil, crop: [Double]? = nil
) throws -> CGImage {
    guard let doc = CGPDFDocument(URL(fileURLWithPath: path) as CFURL) else {
        throw BridgeError(description: "cannot open PDF")
    }
    guard index >= 0, index < doc.numberOfPages, let page = doc.page(at: index + 1) else {
        throw BridgeError(description: "page \(index + 1) out of range")
    }
    let box = page.getBoxRect(.cropBox)
    let rotated = page.rotationAngle % 180 != 0
    let (pw, ph) = rotated ? (box.height, box.width) : (box.width, box.height)
    guard pw > 0, ph > 0 else { throw BridgeError(description: "empty page box") }
    var scale = dpi / 72.0
    if let side = targetSide {
        let (cw, ch) = crop.map { ($0[2] * pw, $0[3] * ph) } ?? (pw, ph)
        scale = side / max(cw, ch, 1e-6)
    }
    scale = min(scale, maxSide / max(pw, ph))
    let w = max(1, Int((pw * scale).rounded()))
    let h = max(1, Int((ph * scale).rounded()))
    let ctx = try bitmapContext(width: w, height: h)
    // getDrawingTransform never scales up, so scale to pixels first and fit the box in point space.
    ctx.scaleBy(x: CGFloat(w) / pw, y: CGFloat(h) / ph)
    ctx.concatenate(page.getDrawingTransform(.cropBox, rect: CGRect(x: 0, y: 0, width: pw, height: ph), rotate: 0, preserveAspectRatio: true))
    ctx.drawPDFPage(page)
    guard let image = ctx.makeImage() else { throw BridgeError(description: "render failed") }
    return image
}

private func loadImage(path: String, index: Int, dpi: Double, fitToSide: Bool = false) throws -> CGImage {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil) else {
        throw BridgeError(description: "cannot open image")
    }
    guard index >= 0, index < CGImageSourceGetCount(src),
        let image = CGImageSourceCreateImageAtIndex(src, index, nil)
    else { throw BridgeError(description: "image frame \(index + 1) out of range") }
    let longest = Double(max(image.width, image.height))
    let target = 11.0 * dpi
    guard !fitToSide, longest > target else { return image }
    let scale = target / longest
    let w = max(1, Int((Double(image.width) * scale).rounded()))
    let h = max(1, Int((Double(image.height) * scale).rounded()))
    let ctx = try bitmapContext(width: w, height: h)
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    guard let out = ctx.makeImage() else { throw BridgeError(description: "downscale failed") }
    return out
}

private func structure(_ doc: DocumentObservation.Container) -> (title: BlockOut?, paragraphs: [BlockOut], structured: [BlockOut]) {
    var structured: [BlockOut] = []
    for table in doc.tables {
        let rows = table.rows.map { row in
            row.map { $0.content.text.transcript }
        }
        structured.append(BlockOut(kind: "table", bbox: topLeftBox(table.boundingRegion), rows: rows))
    }
    for list in doc.lists {
        let items = list.items.map { ListItemOut(marker: $0.markerString, text: $0.itemString) }
        structured.append(BlockOut(kind: "list", bbox: topLeftBox(list.boundingRegion), items: items))
    }
    var title: BlockOut? = nil
    if let t = doc.title {
        title = BlockOut(kind: "title", bbox: topLeftBox(t.boundingRegion), text: t.transcript)
    }
    // Paragraphs can repeat text already captured inside tables/lists; drop those.
    var paragraphs: [BlockOut] = []
    for para in doc.paragraphs {
        let box = topLeftBox(para.boundingRegion)
        if structured.contains(where: { overlaps(box, $0.bbox) }) { continue }
        if let t = doc.title, para.transcript == t.transcript { continue }
        paragraphs.append(BlockOut(kind: "paragraph", bbox: box, text: para.transcript))
    }
    return (title, paragraphs, structured)
}

/// Vision's paragraph order is kept; each table/list goes before the first paragraph below its top.
private func readingOrder(title: BlockOut?, paragraphs: [BlockOut], structured: [BlockOut]) -> [BlockOut] {
    var blocks = paragraphs
    for s in structured.sorted(by: { $0.bbox[1] < $1.bbox[1] }) {
        if let at = blocks.firstIndex(where: { $0.kind == "paragraph" && $0.bbox[1] > s.bbox[1] }) {
            blocks.insert(s, at: at)
        } else {
            blocks.append(s)
        }
    }
    if let t = title { blocks.insert(t, at: 0) }
    return blocks
}

private func runVision(_ perform: (RecognizeDocumentsRequest) async throws -> [DocumentObservation], options: Options) async -> PageOut {
    do {
        var request = RecognizeDocumentsRequest()
        request.textRecognitionOptions.recognitionLanguages = options.languages.map { Locale.Language(identifier: $0) }
        request.textRecognitionOptions.automaticallyDetectLanguage = options.autoDetectLanguage
        request.textRecognitionOptions.useLanguageCorrection = options.languageCorrection
        request.textRecognitionOptions.customWords = options.customWords
        let observations = try await perform(request)
        var blocks: [BlockOut] = []
        var lines: [LineOut] = []
        for obs in observations {
            if options.lines == true {
                for line in obs.document.text.lines where !line.transcript.isEmpty {
                    lines.append(LineOut(text: line.transcript, bbox: topLeftBox(line.boundingRegion)))
                }
            }
            let s = structure(obs.document)
            blocks.append(contentsOf: readingOrder(title: s.title, paragraphs: s.paragraphs, structured: s.structured))
        }
        return PageOut(ok: true, blocks: blocks, lines: lines)
    } catch {
        return PageOut(ok: false, error: String(describing: error))
    }
}

/// Line-level reading with the older text recognizer: each line with its confidence and up to
/// two alternative readings. No blocks.
private func runTextLines(_ image: CGImage, options: Options) async -> PageOut {
    do {
        var request = RecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = options.languages.map { Locale.Language(identifier: $0) }
        request.automaticallyDetectsLanguage = options.autoDetectLanguage
        request.usesLanguageCorrection = options.languageCorrection
        request.customWords = options.customWords
        let observations = try await request.perform(on: image)
        var lines: [LineOut] = []
        for obs in observations {
            let candidates = obs.topCandidates(3)
            guard let best = candidates.first, !best.string.isEmpty else { continue }
            let r = obs.boundingBox.cgRect
            lines.append(LineOut(
                text: best.string,
                bbox: [r.minX, 1.0 - r.maxY, r.width, r.height],
                confidence: Double(best.confidence),
                alternatives: candidates.dropFirst().map(\.string)))
        }
        return PageOut(ok: true, lines: lines)
    } catch {
        return PageOut(ok: false, error: String(describing: error))
    }
}

private func recognize(path: String, options: Options) async -> PageOut {
    await runVision({ try await $0.perform(on: URL(fileURLWithPath: path)) }, options: options)
}

private func recognizePage(path: String, index: Int, dpi: Double, options: Options) async -> PageOut {
    do {
        let image = isPDF(path)
            ? try renderPDFPage(path: path, index: index, dpi: dpi)
            : try loadImage(path: path, index: index, dpi: dpi)
        var target = image
        var crop: [Double]? = nil
        if let c = options.crop, c.count == 4 {
            let w = Double(image.width), h = Double(image.height)
            let rect = CGRect(x: c[0] * w, y: c[1] * h, width: c[2] * w, height: c[3] * h)
                .integral.intersection(CGRect(x: 0, y: 0, width: w, height: h))
            guard !rect.isEmpty, let cropped = image.cropping(to: rect) else {
                throw BridgeError(description: "empty crop")
            }
            target = cropped
            // Normalized crop actually applied, for mapping boxes back to page coordinates.
            crop = [rect.minX / w, rect.minY / h, rect.width / w, rect.height / h]
        }
        let input = target
        var out = options.reader == "text"
            ? await runTextLines(input, options: options)
            : await runVision({ try await $0.perform(on: input) }, options: options)
        if let c = crop {
            for i in out.blocks.indices {
                let b = out.blocks[i].bbox
                out.blocks[i].bbox = [c[0] + b[0] * c[2], c[1] + b[1] * c[3], b[2] * c[2], b[3] * c[3]]
            }
            for i in out.lines.indices {
                let b = out.lines[i].bbox
                out.lines[i].bbox = [c[0] + b[0] * c[2], c[1] + b[1] * c[3], b[2] * c[2], b[3] * c[3]]
            }
        }
        out.width = image.width
        out.height = image.height
        return out
    } catch {
        return PageOut(ok: false, error: String(describing: error))
    }
}

private func decodeOptions(_ optionsJSON: UnsafePointer<CChar>?) -> Options {
    optionsJSON
        .flatMap { try? JSONDecoder().decode(Options.self, from: Data(String(cString: $0).utf8)) } ?? Options()
}

/// Caps concurrent Vision text requests process-wide. Several requests starting at once crash
/// Apple's TextRecognition (EXC_BAD_ACCESS/SIGSEGV in libobjc objc_retain under TextRecognition,
/// 0.03-0.81 s after launch; docs/spike.md). A first-use gate and warm-up did not help; a cap of
/// 2 did (0 crashes in 100 launches vs about 5%), and the user chose it. The `ocr scan`
/// supervisor retries from checkpoints as a safety net. `OCR_VISION_MAX_CONCURRENT` (1-64)
/// overrides the cap for measurement.
private let visionLimit: Int = {
    let v = ProcessInfo.processInfo.environment["OCR_VISION_MAX_CONCURRENT"].flatMap { Int($0) }
    return v.map { min(max($0, 1), 64) } ?? 2
}()
private let visionSlots = DispatchSemaphore(value: visionLimit)

/// Highest number of requests seen in flight at once (for tests: ocr_bridge_max_concurrent).
private final class ConcurrencyStats: @unchecked Sendable {
    private let lock = NSLock()
    private var current = 0
    private(set) var peak = 0

    func enter() {
        lock.lock()
        current += 1
        peak = max(peak, current)
        lock.unlock()
    }

    func leave() {
        lock.lock()
        current -= 1
        lock.unlock()
    }

    func read() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return peak
    }
}

private let visionStats = ConcurrencyStats()

@_cdecl("ocr_bridge_max_concurrent")
public func ocr_bridge_max_concurrent() -> Int32 {
    Int32(visionStats.read())
}

/// Runs async work to completion on the calling (non-Swift-executor) thread. Every Vision text
/// request (recognize_document, recognize_page) goes through here, hence through the cap.
private func blocking(_ work: @escaping @Sendable () async -> PageOut) -> UnsafeMutablePointer<CChar>? {
    visionSlots.wait()
    visionStats.enter()
    defer {
        visionStats.leave()
        visionSlots.signal()
    }
    let box = ResultBox()
    let done = DispatchSemaphore(value: 0)
    Task.detached {
        let page = await work()
        let data = (try? JSONEncoder().encode(page)) ?? Data(#"{"ok":false,"error":"encode failed"}"#.utf8)
        box.json = String(decoding: data, as: UTF8.self)
        done.signal()
    }
    done.wait()
    return strdup(box.json)
}

@_cdecl("ocr_bridge_recognize_document")
public func ocr_bridge_recognize_document(
    _ path: UnsafePointer<CChar>, _ optionsJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    let options = decodeOptions(optionsJSON)
    return blocking { await recognize(path: p, options: options) }
}

@_cdecl("ocr_bridge_recognize_page")
public func ocr_bridge_recognize_page(
    _ path: UnsafePointer<CChar>, _ pageIndex: Int32, _ dpi: Int32, _ optionsJSON: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    let options = decodeOptions(optionsJSON)
    return blocking { await recognizePage(path: p, index: Int(pageIndex), dpi: Double(dpi), options: options) }
}

@_cdecl("ocr_bridge_document_info")
public func ocr_bridge_document_info(_ path: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    let url = URL(fileURLWithPath: p)
    if isPDF(p) {
        guard let doc = CGPDFDocument(url as CFURL) else {
            return json(InfoOut(ok: false, error: "cannot open PDF"))
        }
        if doc.isEncrypted && !doc.isUnlocked {
            return json(InfoOut(ok: false, error: "PDF is encrypted"))
        }
        return json(InfoOut(ok: true, kind: "pdf", pages: doc.numberOfPages))
    }
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else {
        return json(InfoOut(ok: false, error: "cannot open image"))
    }
    return json(InfoOut(ok: true, kind: "image", pages: CGImageSourceGetCount(src)))
}

@_cdecl("ocr_bridge_text_layer")
public func ocr_bridge_text_layer(_ path: UnsafePointer<CChar>, _ pageIndex: Int32) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    guard isPDF(p) else { return json(TextOut(ok: true)) }
    guard let page = PDFDocument(url: URL(fileURLWithPath: p))?.page(at: Int(pageIndex)) else {
        return json(TextOut(ok: false, error: "page \(Int(pageIndex) + 1) not found"))
    }
    return json(TextOut(ok: true, text: page.string ?? ""))
}

struct RegionsOut: Encodable {
    var ok: Bool
    var error: String? = nil
    var texts: [String] = []
}

/// Embedded text inside each region of a PDF page. `regionsJSON` is `[[x, y, w, h], ...]`,
/// normalized with a top-left origin on the page as it is rendered (crop box, rotation applied),
/// the same frame as Vision's boxes. Each region is mapped back to PDF page space with the
/// inverse of the render transform. Images and failures give `ok: false`.
@_cdecl("ocr_bridge_text_in_regions")
public func ocr_bridge_text_in_regions(
    _ path: UnsafePointer<CChar>, _ pageIndex: Int32, _ regionsJSON: UnsafePointer<CChar>,
    _ geometric: Int32
) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    guard isPDF(p) else { return json(RegionsOut(ok: false, error: "not a PDF")) }
    guard let data = String(cString: regionsJSON).data(using: .utf8),
          let regions = try? JSONDecoder().decode([[Double]].self, from: data) else {
        return json(RegionsOut(ok: false, error: "bad regions"))
    }
    guard let page = PDFDocument(url: URL(fileURLWithPath: p))?.page(at: Int(pageIndex)),
          let ref = page.pageRef else {
        return json(RegionsOut(ok: false, error: "page \(Int(pageIndex) + 1) not found"))
    }
    let box = ref.getBoxRect(.cropBox)
    let rotated = ref.rotationAngle % 180 != 0
    let (pw, ph) = rotated ? (box.height, box.width) : (box.width, box.height)
    guard pw > 0, ph > 0 else { return json(RegionsOut(ok: false, error: "empty page box")) }
    // The render maps page space to a pw x ph display rect (origin bottom-left); invert it.
    let toDisplay = ref.getDrawingTransform(.cropBox, rect: CGRect(x: 0, y: 0, width: pw, height: ph), rotate: 0, preserveAspectRatio: true)
    let toPage = toDisplay.inverted()
    if geometric == 0 {
        var texts: [String] = []
        for r in regions where r.count == 4 {
            let display = CGRect(x: r[0] * pw, y: (1 - r[1] - r[3]) * ph, width: r[2] * pw, height: r[3] * ph)
            texts.append(page.selection(for: display.applying(toPage))?.string ?? "")
        }
        return json(RegionsOut(ok: true, texts: texts))
    }
    // `geometric`: each character's box, once per page. A region's text is the characters whose centre lies
    // inside it, in text order. PDFKit's `selection(for:)` selects like a mouse drag, from the
    // first character to the last in text order, which on a 2- or 4-up transcript or a
    // two-column page takes in the neighbouring column's lines. Spaces and line breaks come from
    // the characters' positions, since many PDFs place words apart without a space character.
    // (`characterBounds(at:)` counts characters differently from `page.string`; a one-character
    // selection's bounds are the character's own.)
    let chars = Array((page.string ?? "").utf16)
    let bounds: [CGRect?] = (0..<chars.count).map { i in
        guard let b = page.selection(for: NSRange(location: i, length: 1))?.bounds(for: page),
              !b.isEmpty, !b.isNull else { return nil }
        return b
    }
    var texts: [String] = []
    for r in regions where r.count == 4 {
        let display = CGRect(x: r[0] * pw, y: (1 - r[1] - r[3]) * ph, width: r[2] * pw, height: r[3] * ph)
        let rect = display.applying(toPage).standardized
        var out: [UInt16] = []
        var last: CGRect? = nil
        var spaced = false
        for (i, c) in chars.enumerated() {
            let space = c == 0x20 || c == 0x0A || c == 0x0D || c == 0x09
            guard let b = bounds[i] else { continue }
            if space {
                if last != nil, rect.contains(CGPoint(x: b.midX, y: b.midY)) { spaced = true }
                continue
            }
            guard rect.contains(CGPoint(x: b.midX, y: b.midY)) else { continue }
            if let l = last {
                let h = max(min(l.height, b.height), 1)
                let sameLine = abs(l.midY - b.midY) < h * 0.6
                if !sameLine {
                    out.append(0x0A)
                } else if spaced || b.minX - l.maxX > h * 0.25 || b.minX < l.minX {
                    out.append(0x20)
                }
            }
            out.append(c)
            last = b
            spaced = false
        }
        texts.append(String(utf16CodeUnits: out, count: out.count))
    }
    return json(RegionsOut(ok: true, texts: texts))
}

/// Renders a page exactly as `recognize_page` does and returns it as 8-bit DeviceGray, one byte
/// per pixel, row 0 at the top. Release with ocr_bridge_free_bytes. Null on failure.
struct InvisibleOut: Encodable {
    var ok: Bool
    var error: String? = nil
    /// Text-showing operators drawn visibly, and with render mode 3 (invisible).
    var shown: Int = 0
    var invisible: Int = 0
}

/// Text state while scanning a content stream: the render mode, saved and restored with the
/// graphics state.
private final class TextModes {
    var mode = 0
    var saved: [Int] = []
    var shown = 0
    var invisible = 0
    func show() { if mode == 3 { invisible += 1 } else { shown += 1 } }
}

/// How much of a page's text is drawn invisibly (render mode 3), as OCR tools lay a text layer
/// over a scan. Such text is another OCR's reading, not the document's own characters. Form
/// XObjects are not entered.
@_cdecl("ocr_bridge_invisible_text")
public func ocr_bridge_invisible_text(_ path: UnsafePointer<CChar>, _ pageIndex: Int32) -> UnsafeMutablePointer<CChar>? {
    let p = String(cString: path)
    guard isPDF(p), let doc = CGPDFDocument(URL(fileURLWithPath: p) as CFURL),
          let page = doc.page(at: Int(pageIndex) + 1) else {
        return json(InvisibleOut(ok: false, error: "page \(Int(pageIndex) + 1) not found"))
    }
    let state = TextModes()
    guard let table = CGPDFOperatorTableCreate() else {
        return json(InvisibleOut(ok: false, error: "no operator table"))
    }
    // The callbacks get the scanner's info pointer: `state`.
    CGPDFOperatorTableSetCallback(table, "Tr") { s, info in
        var m: CGPDFInteger = 0
        if CGPDFScannerPopInteger(s, &m), let info {
            Unmanaged<TextModes>.fromOpaque(info).takeUnretainedValue().mode = Int(m)
        }
    }
    CGPDFOperatorTableSetCallback(table, "q") { _, info in
        guard let info else { return }
        let t = Unmanaged<TextModes>.fromOpaque(info).takeUnretainedValue()
        t.saved.append(t.mode)
    }
    CGPDFOperatorTableSetCallback(table, "Q") { _, info in
        guard let info else { return }
        let t = Unmanaged<TextModes>.fromOpaque(info).takeUnretainedValue()
        t.mode = t.saved.popLast() ?? 0
    }
    for op in ["Tj", "TJ", "'", "\""] {
        CGPDFOperatorTableSetCallback(table, op) { _, info in
            guard let info else { return }
            Unmanaged<TextModes>.fromOpaque(info).takeUnretainedValue().show()
        }
    }
    let stream = CGPDFContentStreamCreateWithPage(page)
    let scanner = CGPDFScannerCreate(stream, table, Unmanaged.passUnretained(state).toOpaque())
    CGPDFScannerScan(scanner)
    CGPDFScannerRelease(scanner)
    CGPDFContentStreamRelease(stream)
    CGPDFOperatorTableRelease(table)
    return json(InvisibleOut(ok: true, shown: state.shown, invisible: state.invisible))
}

@_cdecl("ocr_bridge_render_gray")
public func ocr_bridge_render_gray(
    _ path: UnsafePointer<CChar>, _ pageIndex: Int32, _ dpi: Int32,
    _ outWidth: UnsafeMutablePointer<Int32>?, _ outHeight: UnsafeMutablePointer<Int32>?
) -> UnsafeMutablePointer<UInt8>? {
    outWidth?.pointee = 0
    outHeight?.pointee = 0
    let p = String(cString: path)
    do {
        let image = isPDF(p)
            ? try renderPDFPage(path: p, index: Int(pageIndex), dpi: Double(dpi))
            : try loadImage(path: p, index: Int(pageIndex), dpi: Double(dpi))
        let w = image.width, h = image.height
        guard let buffer = calloc(w * h, 1)?.assumingMemoryBound(to: UInt8.self),
            let ctx = CGContext(
                data: buffer, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue)
        else { return nil }
        ctx.setFillColor(gray: 1, alpha: 1)
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        ctx.interpolationQuality = .high
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        outWidth?.pointee = Int32(w)
        outHeight?.pointee = Int32(h)
        return buffer
    } catch {
        return nil
    }
}

/// Resamples to `side` pixels on the longest side, exactly (drawn into an exactly sized context).
private func fitLongestSide(_ image: CGImage, side: Int) throws -> CGImage {
    let longest = max(image.width, image.height)
    if longest == side { return image }
    let scale = Double(side) / Double(longest)
    let w = image.width >= image.height ? side : max(1, Int((Double(image.width) * scale).rounded()))
    let h = image.height > image.width ? side : max(1, Int((Double(image.height) * scale).rounded()))
    let ctx = try bitmapContext(width: w, height: h)
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    guard let out = ctx.makeImage() else { throw BridgeError(description: "resize failed") }
    return out
}

/// Rotates counter-clockwise by `degrees` (0, 90, 180 or 270): PIL's `Image.transpose(ROTATE_*)`,
/// which is what olmOCR's pipeline applies for a `rotation_correction`.
private func rotateCounterClockwise(_ image: CGImage, degrees: Int) throws -> CGImage {
    let turns = ((degrees % 360) + 360) % 360 / 90
    if turns == 0 { return image }
    let w = image.width, h = image.height
    let (ow, oh) = turns % 2 == 1 ? (h, w) : (w, h)
    let ctx = try bitmapContext(width: ow, height: oh)
    ctx.translateBy(x: CGFloat(ow) / 2, y: CGFloat(oh) / 2)
    // Positive angles turn counter-clockwise in the bitmap's y-up space.
    ctx.rotate(by: CGFloat(turns) * .pi / 2)
    ctx.draw(image, in: CGRect(x: -CGFloat(w) / 2, y: -CGFloat(h) / 2, width: CGFloat(w), height: CGFloat(h)))
    guard let out = ctx.makeImage() else { throw BridgeError(description: "rotate failed") }
    return out
}

/// Renders a page exactly as `recognize_page` does, optionally cropped to a normalized
/// top-left-origin rect [x, y, w, h], and encodes it as PNG for model engines.
/// `maxSideArg` > 0 ignores `dpi` and sizes the result (the crop, if any) to exactly that many
/// pixels on its longest side; `rotation` (0/90/180/270) then turns it counter-clockwise.
/// Release with ocr_bridge_free_bytes. Null on failure.
@_cdecl("ocr_bridge_render_png")
public func ocr_bridge_render_png(
    _ path: UnsafePointer<CChar>, _ pageIndex: Int32, _ dpi: Int32,
    _ crop: UnsafePointer<Double>?, _ maxSideArg: Int32, _ rotation: Int32,
    _ outLen: UnsafeMutablePointer<Int>?
) -> UnsafeMutablePointer<UInt8>? {
    outLen?.pointee = 0
    let p = String(cString: path)
    let cropRect = crop.map { [$0[0], $0[1], $0[2], $0[3]] }
    let side = maxSideArg > 0 ? Int(maxSideArg) : nil
    do {
        var image = isPDF(p)
            ? try renderPDFPage(
                path: p, index: Int(pageIndex), dpi: Double(dpi), targetSide: side.map(Double.init),
                crop: cropRect)
            : try loadImage(path: p, index: Int(pageIndex), dpi: Double(dpi), fitToSide: side != nil)
        if let c = cropRect {
            let w = Double(image.width), h = Double(image.height)
            let rect = CGRect(x: c[0] * w, y: c[1] * h, width: c[2] * w, height: c[3] * h).integral
            guard let cropped = image.cropping(to: rect) else { return nil }
            image = cropped
        }
        if let side { image = try fitLongestSide(image, side: side) }
        image = try rotateCounterClockwise(image, degrees: Int(rotation))
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil) else {
            return nil
        }
        CGImageDestinationAddImage(dest, image, nil)
        guard CGImageDestinationFinalize(dest), let buffer = malloc(data.length) else { return nil }
        memcpy(buffer, data.bytes, data.length)
        outLen?.pointee = data.length
        return buffer.assumingMemoryBound(to: UInt8.self)
    } catch {
        return nil
    }
}

@_cdecl("ocr_bridge_free_bytes")
public func ocr_bridge_free_bytes(_ ptr: UnsafeMutablePointer<UInt8>?) {
    free(ptr)
}

@_cdecl("ocr_bridge_free")
public func ocr_bridge_free(_ ptr: UnsafeMutablePointer<CChar>?) {
    free(ptr)
}
