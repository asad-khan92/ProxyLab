//  The CA certificate the core stored, as bytes.
//
//  The core writes one PEM file. This reads that file and nothing beside it:
//  the private key is the core's, and a path to it is not built here. The
//  identity of the certificate is the DER encoding. Two certificates with the
//  same subject are not the same certificate.

import CryptoKit
import Foundation
import Security

/// What is at the CA path the core uses.
enum CaptureCertificateFile: Equatable {
    /// A certificate Security could parse. `der` is its exact encoding.
    case certificate(Data)
    /// The path is not known, or no file is there.
    case missing
    /// A file is there and it is not a certificate.
    case malformed
}

/// Reads the core's CA certificate. It does not talk to a keychain.
enum CaptureCertificateReader {
    /// The certificate at `path`, or why there isn't one.
    static func load(at path: String?) -> CaptureCertificateFile {
        guard let path, !path.isEmpty else { return .missing }
        guard FileManager.default.fileExists(atPath: path) else { return .missing }

        guard let bytes = try? Data(contentsOf: URL(fileURLWithPath: path)), !bytes.isEmpty else {
            return .malformed
        }
        guard let der = certificateDER(from: bytes) else { return .malformed }

        return .certificate(der)
    }

    /// SHA-256 of `der`, grouped in the form a certificate viewer shows.
    static func fingerprint(of der: Data) -> String {
        SHA256.hash(data: der).map { String(format: "%02X", $0) }.joined(separator: ":")
    }

    // MARK: - PEM

    /// DER for the first certificate in `bytes`, which may be PEM or DER.
    ///
    /// A PEM block that is not a certificate — a private key, in particular —
    /// is not a certificate. A PEM block that says it is a certificate and
    /// does not parse is not one either.
    private static func certificateDER(from bytes: Data) -> Data? {
        if let text = String(data: bytes, encoding: .utf8), text.contains("BEGIN CERTIFICATE") {
            guard let der = derFromPEM(text) else { return nil }
            return SecCertificateCreateWithData(nil, der as CFData) != nil ? der : nil
        }

        return SecCertificateCreateWithData(nil, bytes as CFData) != nil ? bytes : nil
    }

    private static func derFromPEM(_ text: String) -> Data? {
        var body = ""
        var inside = false

        for line in text.split(whereSeparator: \.isNewline) {
            if line.contains("BEGIN CERTIFICATE") {
                inside = true
                continue
            }
            if line.contains("END CERTIFICATE") { break }
            if inside { body.append(contentsOf: line) }
        }

        guard inside, !body.isEmpty else { return nil }

        return Data(base64Encoded: body, options: .ignoreUnknownCharacters)
    }
}
