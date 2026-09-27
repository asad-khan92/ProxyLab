//  A captured body, shown as what it is.
//
//      JSON        ──▶ laid out, with the bytes a click away
//      text        ──▶ as it arrived, whitespace and all
//      an image    ──▶ drawn, with its Base64 a click away
//      not text    ──▶ said to be binary, with its size and a way to keep it
//
//  Nothing is decoded that cannot honestly be decoded, and nothing is
//  rewritten: this shows the bytes the core captured. When the capture was
//  truncated it says so, because a body that ends early looks exactly like one
//  that was short.
//
//  Neither is anything decoded *here*. The reading was done once, when the
//  exchange arrived and off the main actor; this draws its result, and the
//  only work it does on a redraw is laying out the text it was handed.

import AppKit
import SwiftUI

/// One captured body.
struct BodyView: View {
    let inspection: BodyInspection
    /// Copies something, when the pane that owns this can.
    var copy: ((String, String) -> Void)?

    /// Which of the two ways of looking at this body is showing.
    ///
    /// Reset whenever the body changes, so a formatted view is not carried
    /// over onto a body that has no formatted form.
    @State private var raw = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                BodyMeta(inspection: inspection)

                Spacer(minLength: 8)

                if let names = modeNames {
                    Picker("", selection: $raw) {
                        Text(names.formatted).tag(false)
                        Text(names.raw).tag(true)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .controlSize(.small)
                    .fixedSize()
                }
            }

            if inspection.capture.truncated {
                TruncationNotice(capture: inspection.capture)
            }

            content
        }
        .onChange(of: inspection) { raw = false }
    }

    /// What the two sides of the switch are called, or nothing when there is
    /// only one way to look at this body.
    private var modeNames: (formatted: String, raw: String)? {
        switch inspection.content {
        case let .text(text) where text.formatted != nil:
            ("Formatted", "Raw")
        case .image:
            ("Preview", "Base64")
        case .text, .empty, .binary, .undecodable:
            nil
        }
    }

    @ViewBuilder
    private var content: some View {
        switch inspection.content {
        case .empty:
            Text("No body")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)

        case let .text(text):
            if let tree = text.tree, !raw {
                JSONTreeView(root: tree, formatted: text.formatted, copy: copy)
            } else if let formatted = text.formatted, !raw {
                BodyText(text: formatted)
            } else {
                BodyText(text: text.text)
            }

            if text.shortened {
                Note(
                    symbol: "text.append",
                    text: "Only the first "
                        + "\(BodyInspection.displayedCharacterLimit.formatted()) characters are "
                        + "shown here. Copy or save the body for all of it."
                )
            }

        case let .image(image):
            if raw {
                Base64Body(data: inspection.capture.data)
            } else {
                ImagePreview(image: image, capture: inspection.capture)
            }

        case .undecodable:
            VStack(alignment: .leading, spacing: 8) {
                Note(
                    symbol: "exclamationmark.triangle",
                    text: "Unable to decode body as text"
                        + (inspection.contentType.map { ", though it was sent as \($0.raw)." }
                            ?? ".")
                )
                Base64Body(data: inspection.capture.data)
            }

        case .binary:
            VStack(alignment: .leading, spacing: 8) {
                Text("Binary body — \(TrafficDisplay.size(inspection.capture.size))")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Base64Body(data: inspection.capture.data)
            }
        }
    }
}

/// What the body is, and how much of it there is.
private struct BodyMeta: View {
    let inspection: BodyInspection

    var body: some View {
        HStack(spacing: 6) {
            Text(kind)
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(.tertiary)

            if !inspection.isEmpty {
                Text("·")
                    .font(.system(size: 9))
                    .foregroundStyle(.quaternary)
                Text(TrafficDisplay.size(inspection.capture.size))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
            }

            if let contentType = inspection.contentType {
                Text("·")
                    .font(.system(size: 9))
                    .foregroundStyle(.quaternary)
                Text(contentType.raw)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help("The Content-Type the message declared")
            }
        }
    }

    /// The shortest true word for what this body turned out to be.
    private var kind: String {
        switch inspection.content {
        case .empty: "EMPTY"
        case let .text(text):
            text.encoding == .utf8
                ? text.language.rawValue.uppercased()
                : "\(text.language.rawValue.uppercased()) · \(text.encodingName)"
        case let .image(image): image.format.uppercased()
        case .undecodable: "UNDECODABLE"
        case .binary: "BINARY"
        }
    }
}

/// The body's text, selectable and monospaced.
struct BodyText: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(8)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 5))
    }
}

/// A captured image, drawn if AppKit can draw it.
///
/// `NSImage` is what decodes it, which is every format ImageIO supports and no
/// library this app had to add. A capture that will not decode — a format
/// macOS has no reader for, or an image cut off by the capture limit — falls
/// back to the bytes rather than to an empty frame.
private struct ImagePreview: View {
    let image: ImageBody
    let capture: BodyCapture

    var body: some View {
        if let decoded = NSImage(data: capture.data), decoded.size.width > 0 {
            VStack(alignment: .leading, spacing: 6) {
                // Never drawn larger than it is: an eight-pixel icon blown up
                // to fill the pane is a picture of the renderer's guesswork
                // rather than of what the server sent.
                Image(nsImage: decoded)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: decoded.size.width, maxHeight: 280, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: 5).fill(Color.primary.opacity(0.04))
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
                Base64Body(data: capture.data)
            }
        }
    }
}

/// Bytes as Base64, when there are few enough of them to put on screen.
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
