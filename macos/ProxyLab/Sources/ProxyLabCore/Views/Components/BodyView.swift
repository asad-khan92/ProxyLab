//  A captured body, shown as what it is.
//
//      JSON        ──▶  Pretty, Tree, or Raw
//      text        ──▶  as it arrived, whitespace and all
//      an image    ──▶  drawn, or the presented bytes as Base64
//      not text    ──▶  said to be binary, with its size and a way to keep it
//
//  Three things stay distinct. The capture is what Save writes, content
//  coding included. The presented bytes are that capture after a declared
//  Content-Encoding was removed, and Raw shows those. Formatted JSON is a
//  layout of the presented JSON for Pretty and for Copy JSON. Nothing here
//  writes any of them back.
//
//  Neither is anything decoded here. The reading was done once, when the
//  exchange arrived and off the main actor. Coloring Pretty uses the spans
//  from that reading. A redraw does not parse the JSON again.

import AppKit
import SwiftUI

/// One captured body.
struct BodyView: View {
    let inspection: BodyInspection
    var contentEncoding: String?
    var status: String?
    /// What a status-bar message calls this body: "the response body".
    var copySubject: String = "the body"
    /// Writes the captured bytes. The panel and the file name belong to the caller.
    var save: (() -> Void)?
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    /// Which mode is selected. A body that cannot show it falls back, and a
    /// new body starts at Pretty when Pretty exists.
    @State private var mode: BodyViewMode = .pretty
    @State private var query = ""
    @State private var hits: [BodyMatch] = []
    @State private var searchCapped = false
    @State private var matchIndex = 0
    /// Bumped when the inspection changes so a tree does not keep another
    /// document's open rows. Every JSON root has the same path id.
    @State private var treeGeneration = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header

            if showsActions {
                actions
            }

            if inspection.capture.truncated {
                TruncationNotice(capture: inspection.capture)
            }

            content
        }
        .onChange(of: inspection) { _, new in
            mode = BodyPresentation.defaultMode(for: new)
            query = ""
            matchIndex = 0
            treeGeneration += 1
            refreshSearch()
        }
        .onChange(of: mode) { _, _ in
            matchIndex = 0
            refreshSearch()
        }
        .onChange(of: query) { _, _ in
            matchIndex = 0
            refreshSearch()
        }
    }

    private var availableModes: [BodyViewMode] {
        BodyPresentation.modes(for: inspection)
    }

    private var effectiveMode: BodyViewMode {
        if availableModes.contains(mode) { return mode }

        return availableModes.first ?? .raw
    }

    private var modeBinding: Binding<BodyViewMode> {
        Binding(get: { effectiveMode }, set: { mode = $0 })
    }

    /// The text Find searches: the formatted JSON in Pretty, the presented
    /// text in Raw. Tree is not searched; opening every node to look would
    /// throw away the point of leaving them closed.
    private var searchableText: String? {
        guard case let .text(text) = inspection.content else { return nil }

        switch effectiveMode {
        case .pretty: return text.formatted ?? text.text
        case .raw: return text.text
        case .tree, .preview: return nil
        }
    }

    private var showsActions: Bool {
        !inspection.isEmpty && (copy != nil || save != nil || searchableText != nil)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            facts

            Spacer(minLength: 8)

            if availableModes.count > 1 {
                Picker("Body view", selection: modeBinding) {
                    ForEach(availableModes, id: \.self) { item in
                        Text(item.title).tag(item)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
                .help(modeHelp)
            }
        }
    }

    private var facts: some View {
        let items = BodyFacts.make(
            inspection: inspection,
            contentEncoding: contentEncoding,
            status: status
        ).items

        return HStack(spacing: 10) {
            ForEach(items) { item in
                Text(item.text)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(
                        item.id == "truncated" ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary)
                    )
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: item.id == "type" ? 240 : nil, alignment: .leading)
                    .help(item.help)
            }
        }
    }

    private var modeHelp: String {
        switch inspection.content {
        case .image:
            "Preview draws the image. Raw is the presented bytes as Base64. Save writes the capture."
        case .text:
            "Pretty is formatted JSON. Tree is collapsible. Raw is the presented text, without that layout."
        case .empty, .binary, .undecodable:
            "How this body is shown"
        }
    }

    private var actions: some View {
        HStack(alignment: .center, spacing: 12) {
            if let copy, !inspection.isEmpty {
                BodyTextButton(title: "Copy", help: copyHelp) {
                    copy(BodyCopy.presentedText(inspection, mode: effectiveMode), copyLabel)
                }

                if let json = BodyCopy.formattedJSON(inspection) {
                    BodyTextButton(title: "Copy JSON", help: "Copy the complete formatted JSON") {
                        copy(json, "the formatted JSON")
                    }
                }
            }

            if let save, !inspection.isEmpty {
                BodyTextButton(
                    title: "Save Body…",
                    help: "Write the captured bytes to a file you choose"
                ) {
                    save()
                }
            }

            Spacer(minLength: 8)

            if searchableText != nil {
                findControls
            }
        }
    }

    private var copyHelp: String {
        switch effectiveMode {
        case .pretty, .tree:
            "Copy the formatted JSON shown here"
        case .raw, .preview:
            switch inspection.content {
            case .image, .binary, .undecodable:
                "Copy the presented bytes as Base64"
            case .empty, .text:
                "Copy the presented text"
            }
        }
    }

    private var copyLabel: String {
        switch effectiveMode {
        case .pretty, .tree:
            if BodyCopy.formattedJSON(inspection) != nil { return "the formatted JSON" }
            return copySubject
        case .raw, .preview:
            switch inspection.content {
            case .image, .binary, .undecodable: return "\(copySubject) as Base64"
            case .empty, .text: return copySubject
            }
        }
    }

    private var findControls: some View {
        HStack(spacing: 4) {
            TextField("Find", text: $query)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 11))
                .frame(width: 140)
                .controlSize(.small)
                .accessibilityLabel("Find in body")
                .onSubmit { advance(1) }

            Button {
                advance(-1)
            } label: {
                Image(systemName: "chevron.up")
                    .font(.system(size: 10))
            }
            .buttonStyle(.borderless)
            .help("Previous match")
            .disabled(hits.isEmpty)

            Button {
                advance(1)
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 10))
            }
            .buttonStyle(.borderless)
            .help("Next match")
            .disabled(hits.isEmpty)

            if !query.isEmpty {
                Text(matchSummary)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .fixedSize()
            }
        }
    }

    private var matchSummary: String {
        guard !hits.isEmpty else { return "No matches" }

        let count = searchCapped ? "\(hits.count)+" : "\(hits.count)"
        return "\(matchIndex + 1) of \(count)"
    }

    @ViewBuilder
    private var content: some View {
        switch inspection.content {
        case .empty:
            Text("No body")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)

        case let .text(text):
            VStack(alignment: .leading, spacing: 6) {
                textContent(text)

                if text.shortened {
                    Note(
                        symbol: "text.append",
                        text: "Only the first "
                            + "\(BodyInspection.displayedCharacterLimit.formatted()) characters are "
                            + "shown here. Copy or save the body for all of it."
                    )
                }
            }

        case let .image(image):
            // Raw is the presented bytes, after a content coding was removed,
            // written as Base64. It is not a UTF-8 decoding, and it is not
            // the capture Save writes.
            if effectiveMode == .raw {
                Base64Body(data: inspection.presented)
            } else {
                ImagePreview(image: image, bytes: inspection.presented, capture: inspection.capture)
            }

        case .undecodable:
            VStack(alignment: .leading, spacing: 8) {
                Note(
                    symbol: "exclamationmark.triangle",
                    text: "Unable to decode body as text"
                        + (inspection.contentType.map { ", though it was sent as \($0.raw)." } ?? ".")
                )
                Base64Body(data: inspection.presented)
            }

        case .binary:
            VStack(alignment: .leading, spacing: 6) {
                Text("Binary body")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                Text("Not shown as text.")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                Base64Body(data: inspection.presented)
            }
        }
    }

    @ViewBuilder
    private func textContent(_ text: TextBody) -> some View {
        switch effectiveMode {
        case .tree:
            if let tree = text.tree {
                JSONTreeView(root: tree, formatted: text.formatted, copy: copy)
                    .id(treeGeneration)
            } else {
                source(text.text, spans: [], wraps: true)
            }
        case .pretty:
            source(
                text.formatted ?? text.text,
                spans: text.formatted == nil ? [] : text.highlights,
                wraps: text.formatted == nil
            )
        case .raw, .preview:
            source(text.text, spans: [], wraps: true)
        }
    }

    private func source(_ text: String, spans: [JSONSpan], wraps: Bool) -> some View {
        SourceTextView(
            text: text,
            spans: spans,
            wraps: wraps,
            matches: hits,
            currentMatch: hits.indices.contains(matchIndex) ? matchIndex : nil
        )
    }

    private func refreshSearch() {
        guard let text = searchableText, !query.isEmpty else {
            hits = []
            searchCapped = false
            matchIndex = 0
            return
        }

        let result = BodySearch.search(query, in: text)
        hits = result.hits
        searchCapped = result.capped
        if matchIndex >= hits.count { matchIndex = 0 }
    }

    private func advance(_ delta: Int) {
        guard !hits.isEmpty else { return }

        let count = hits.count
        matchIndex = (matchIndex + delta % count + count) % count
    }
}

/// Copy, Copy JSON, and Save, in the same quiet style as the rest of the pane.
private struct BodyTextButton: View {
    let title: String
    let help: String
    let action: () -> Void

    var body: some View {
        Button(title, action: action)
            .buttonStyle(.borderless)
            .font(.system(size: 11))
            .help(help)
    }
}

/// Pretty or raw text: monospaced, selectable, and scrolled.
///
/// Line breaks are the text's own. Pretty does not wrap, so indentation
/// survives and a long line scrolls sideways. Raw wraps, because a captured
/// body is often one line and a page-wide line is not a document. The line
/// list is built once per text, not on every redraw, and coloring uses the
/// spans the reading already recorded.
private struct SourceTextView: View {
    let text: String
    let spans: [JSONSpan]
    let wraps: Bool
    let matches: [BodyMatch]
    let currentMatch: Int?

    @State private var cache = BodyDocumentCache()

    var body: some View {
        let lines = cache.lines(for: text, spans: spans)
        let gutter = gutterWidth(for: lines.count)
        let currentOffset: Int? = {
            guard let currentMatch, matches.indices.contains(currentMatch) else { return nil }
            return matches[currentMatch].utf8Start
        }()

        ScrollViewReader { proxy in
            ScrollView(wraps ? .vertical : [.horizontal, .vertical]) {
                lineStack(lines, gutter: gutter)
                    .padding(8)
                    .frame(
                        minWidth: wraps ? nil : CGFloat(cache.longestBytes) * 7.2 + gutter + 16,
                        alignment: .leading
                    )
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .frame(height: viewportHeight(text: text, lines: lines.count))
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 4))
            .onChange(of: currentOffset) { _, offset in
                guard let offset else { return }
                proxy.scrollTo(lineNumber(containing: offset, in: lines), anchor: .center)
            }
        }
    }

    @ViewBuilder
    private func lineStack(_ lines: [BodyLine], gutter: CGFloat) -> some View {
        if lines.count > 400 {
            LazyVStack(alignment: .leading, spacing: 0) {
                lineRows(lines, gutter: gutter)
            }
        } else {
            VStack(alignment: .leading, spacing: 0) {
                lineRows(lines, gutter: gutter)
            }
        }
    }

    private func lineRows(_ lines: [BodyLine], gutter: CGFloat) -> some View {
        ForEach(lines) { line in
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text("\(line.number)")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .frame(width: gutter, alignment: .trailing)
                    .textSelection(.disabled)

                Text(paint(line))
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: wraps ? .infinity : nil, alignment: .leading)
                    .fixedSize(horizontal: !wraps, vertical: true)
            }
            .id(line.number)
        }
    }

    private func paint(_ line: BodyLine) -> AttributedString {
        var result = AttributedString(line.text)
        for span in line.spans {
            guard let range = utf8Range(start: span.start, end: span.end, in: line.text, attributed: result)
            else { continue }

            result[range].foregroundColor = JSONTone.color(for: span.role)
            result[range].font = JSONTone.font(for: span.role)
        }

        let lineEnd = line.utf8Start + line.text.utf8.count
        for (offset, match) in matches.enumerated() {
            guard match.utf8End > line.utf8Start, match.utf8Start < lineEnd else { continue }

            let start = max(match.utf8Start, line.utf8Start) - line.utf8Start
            let end = min(match.utf8End, lineEnd) - line.utf8Start
            guard start < end,
                let range = utf8Range(start: start, end: end, in: line.text, attributed: result)
            else { continue }

            result[range].backgroundColor = Color.accentColor.opacity(offset == currentMatch ? 0.38 : 0.16)
        }

        return result
    }

    private func viewportHeight(text: String, lines: Int) -> CGFloat {
        let visual = wraps ? max(lines, text.utf8.count / 72) : lines
        return min(440, max(44, CGFloat(visual) * 15 + 12))
    }

    private func gutterWidth(for count: Int) -> CGFloat {
        CGFloat(max(2, String(count).count)) * 8 + 4
    }

    private func lineNumber(containing offset: Int, in lines: [BodyLine]) -> Int {
        var found = 1
        for line in lines {
            if line.utf8Start > offset { break }
            found = line.number
        }
        return found
    }

    private func utf8Range(
        start: Int,
        end: Int,
        in text: String,
        attributed: AttributedString
    ) -> Range<AttributedString.Index>? {
        let utf8 = text.utf8
        guard start >= 0, end >= start, end <= utf8.count else { return nil }

        let lower = utf8.index(utf8.startIndex, offsetBy: start)
        let upper = utf8.index(lower, offsetBy: end - start)
        guard let stringStart = String.Index(lower, within: text),
            let stringEnd = String.Index(upper, within: text),
            let attributeStart = AttributedString.Index(stringStart, within: attributed),
            let attributeEnd = AttributedString.Index(stringEnd, within: attributed)
        else { return nil }

        return attributeStart..<attributeEnd
    }
}

/// Holds the line split for one string so a redraw does not walk it again.
private final class BodyDocumentCache {
    private var cachedText: String?
    private var cachedSpanCount = -1
    private var lines: [BodyLine] = []
    private(set) var longestBytes = 0

    func lines(for text: String, spans: [JSONSpan]) -> [BodyLine] {
        if cachedText == text, cachedSpanCount == spans.count { return lines }

        cachedText = text
        cachedSpanCount = spans.count
        lines = BodyLines.split(text, spans: spans)
        longestBytes = lines.reduce(0) { max($0, $1.text.utf8.count) }
        return lines
    }
}

/// Restrained syntax colors. Keys stay in the label color and are set in a
/// heavier weight; punctuation is left alone. Two hues and a third, muted,
/// for booleans — not a color per token.
enum JSONTone {
    static func color(for role: JSONSpan.Role) -> Color {
        switch role {
        case .key: .primary
        case .string: stringColor
        case .number: numberColor
        case .boolean: booleanColor
        case .null: .secondary
        }
    }

    static func font(for role: JSONSpan.Role) -> Font {
        switch role {
        case .key:
            .system(size: 11, weight: .semibold, design: .monospaced)
        case .boolean:
            .system(size: 11, weight: .medium, design: .monospaced)
        case .null:
            .system(size: 11, design: .monospaced).italic()
        case .string, .number:
            .system(size: 11, design: .monospaced)
        }
    }

    static func color(for node: JSONNode) -> Color {
        switch node.kind {
        case .string: color(for: .string)
        case .number: color(for: .number)
        case .literal: color(for: node.raw == "null" ? JSONSpan.Role.null : .boolean)
        case .object, .array: .secondary
        }
    }

    static func font(for node: JSONNode) -> Font {
        switch node.kind {
        case .string: font(for: .string)
        case .number: font(for: .number)
        case .literal: font(for: node.raw == "null" ? JSONSpan.Role.null : .boolean)
        case .object, .array: .system(size: 11, design: .monospaced)
        }
    }

    private static let stringColor = adaptive(
        name: "proxylab.json.string",
        light: (0.11, 0.38, 0.24),
        dark: (0.58, 0.78, 0.66)
    )
    private static let numberColor = adaptive(
        name: "proxylab.json.number",
        light: (0.14, 0.33, 0.56),
        dark: (0.64, 0.76, 0.90)
    )
    private static let booleanColor = adaptive(
        name: "proxylab.json.boolean",
        light: (0.38, 0.28, 0.52),
        dark: (0.78, 0.70, 0.88)
    )

    private static func adaptive(
        name: String,
        light: (CGFloat, CGFloat, CGFloat),
        dark: (CGFloat, CGFloat, CGFloat)
    ) -> Color {
        Color(nsColor: NSColor(name: NSColor.Name(name)) { appearance in
            let darkMode = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let channel = darkMode ? dark : light
            return NSColor(srgbRed: channel.0, green: channel.1, blue: channel.2, alpha: 1)
        })
    }
}

/// The body's text, selectable and monospaced, when it is not a document
/// with lines — Base64, mostly.
struct BodyText: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(8)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 4))
    }
}

/// A captured image, drawn if AppKit can draw it.
///
/// `NSImage` is what decodes it, which is every format ImageIO supports and no
/// library this app had to add. A capture that will not decode — a format
/// macOS has no reader for, or an image cut off by the capture limit — falls
/// back to the bytes rather than to an empty frame. The bytes drawn are the
/// presented bytes, after a content coding was removed.
private struct ImagePreview: View {
    let image: ImageBody
    /// The bytes to draw: the reading, after a content coding was removed.
    let bytes: Data
    let capture: BodyCapture

    var body: some View {
        if let decoded = NSImage(data: bytes), decoded.size.width > 0 {
            VStack(alignment: .leading, spacing: 6) {
                // Never drawn larger than it is: an eight-pixel icon blown up
                // to fill the pane is a picture of the renderer's guesswork
                // rather than of what the server sent.
                Image(nsImage: decoded)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: decoded.size.width, maxHeight: 280, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: 4).fill(Color.primary.opacity(0.04))
                    )
                    .accessibilityLabel("\(image.format) image from the response")

                Text(
                    "\(image.format) · \(Int(decoded.size.width))×\(Int(decoded.size.height))"
                )
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary)
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                Note(
                    symbol: "photo",
                    text: capture.truncated
                        ? "This \(image.format) image could not be drawn — the capture is "
                            + "truncated, so it is missing its end."
                        : "This \(image.format) image could not be drawn."
                )
                Base64Body(data: bytes)
            }
        }
    }
}

/// Presented bytes as Base64, when there are few enough of them to put on screen.
///
/// This is not a text decoding. A body that is not text stays bytes.
private struct Base64Body: View {
    let data: Data

    var body: some View {
        if data.count <= BodyInspection.displayedBase64ByteLimit {
            BodyText(text: data.base64EncodedString())
        } else {
            Note(
                symbol: "doc.badge.ellipsis",
                text: "\(TrafficDisplay.size(UInt64(data.count))) of bytes — too much to show as "
                    + "Base64. Copy or save the body to keep it."
            )
        }
    }
}

/// Says that what is shown is not all of it.
private struct TruncationNotice: View {
    let capture: BodyCapture

    var body: some View {
        Label(
            "Truncated — captured \(TrafficDisplay.size(UInt64(capture.capturedByteCount))) of "
                + "\(TrafficDisplay.size(capture.size)). The rest was forwarded but not kept.",
            systemImage: "scissors"
        )
        .font(.system(size: 10))
        .foregroundStyle(.orange)
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// A line of explanation under a body.
private struct Note: View {
    let symbol: String
    let text: String

    var body: some View {
        Label(text, systemImage: symbol)
            .font(.system(size: 10))
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}
