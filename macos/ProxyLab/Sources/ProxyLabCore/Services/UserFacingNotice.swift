//  A status-bar error, as a person reads it.
//
//  The sentence is short. A path, an OSStatus, or a long diagnostic stays
//  behind Details…, which is the same split the capture-source panel uses.
//  Diagnostics is where the technical text belongs in full.

import Foundation

/// The words a status line shows, and the technical text behind Details… when
/// the message is not already something a person can act on.
struct UserFacingNotice: Equatable, Sendable {
    var summary: String
    /// Nil when `summary` is the whole message.
    var detail: String?

    init(_ raw: String) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            summary = "Something went wrong."
            detail = nil
            return
        }

        if Self.isTechnical(text) {
            let short = Self.summary(forTechnical: text)
            summary = short
            detail = short == text ? nil : text
            return
        }

        if text.count > 180 {
            summary = Self.lead(text)
            detail = text
            return
        }

        summary = text
        detail = nil
    }

    /// A path, a status code, or a source location. Those stay out of the
    /// primary line.
    private static func isTechnical(_ text: String) -> Bool {
        let folded = text.lowercased()
        if folded.contains("osstatus") || folded.contains("nserror") { return true }
        if text.contains("/Users/") || text.contains("/tmp/") || text.contains("/private/")
            || text.contains("/var/")
        {
            return true
        }
        if folded.contains(".rs:") || folded.contains("src/") { return true }
        if folded.contains("debug description") { return true }
        return false
    }

    private static func summary(forTechnical text: String) -> String {
        let folded = text.lowercased()
        if folded.contains("could not reach the proxylab core") {
            return "Could not reach the ProxyLab core."
        }
        if folded.contains("not connected to the proxylab core") {
            return "Not connected to the ProxyLab core."
        }
        if folded.contains("socket path") {
            return "ProxyLab could not find the core."
        }
        if let head = text.split(separator: ":", maxSplits: 1).first {
            let sentence = head.trimmingCharacters(in: .whitespacesAndNewlines)
            if sentence.count >= 12, sentence.count <= 120, !isTechnical(sentence) {
                return sentence.hasSuffix(".") ? sentence : sentence + "."
            }
        }
        return "Something went wrong."
    }

    private static func lead(_ text: String) -> String {
        let sentence = text.split(whereSeparator: { $0 == "." || $0 == "\n" }).first.map(String.init) ?? text
        let trimmed = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.count <= 120 {
            return trimmed.hasSuffix(".") ? trimmed : trimmed + "."
        }
        return String(trimmed.prefix(117)) + "…"
    }
}
