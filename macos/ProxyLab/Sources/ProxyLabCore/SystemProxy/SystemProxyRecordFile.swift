//  The ownership record on disk.
//
//  `~/.proxylab/run/system-proxy-ownership.json`, beside the socket. It is
//  written before the system proxy is changed and removed only after every
//  owned service has been restored or deliberately left alone. A crash, a
//  force-quit, or a restart leaves the file in place, and the next Start
//  reads it instead of treating ProxyLab's own proxy as the previous one.
//
//  The file is this user's. It is not a shell configuration and it is not a
//  preference. The proxy hostnames in it are the settings that were saved,
//  which Stop needs, and nothing else is written there.

import Foundation

struct SystemProxyRecordFile {
    let url: URL

    func load() -> SystemProxyRecord? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(SystemProxyRecord.self, from: data)
    }

    func save(_ record: SystemProxyRecord) throws {
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let data = try JSONEncoder.record.encode(record)
        let temporary = url.appendingPathExtension("tmp")
        try data.write(to: temporary, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: temporary.path)
        if FileManager.default.fileExists(atPath: url.path) {
            _ = try FileManager.default.replaceItemAt(url, withItemAt: temporary)
        } else {
            try FileManager.default.moveItem(at: temporary, to: url)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    func remove() {
        try? FileManager.default.removeItem(at: url)
    }
}

extension JSONEncoder {
    fileprivate static let record: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }()
}
