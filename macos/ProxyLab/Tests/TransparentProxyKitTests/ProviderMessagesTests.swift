//  The two things that cross from the app to the provider: the configuration
//  it is started with, and the status it answers with.

import Foundation
import Testing

@testable import TransparentProxyKit

@Suite("What the app and the provider say to each other")
struct ProviderMessagesTests {
    @Test("the configuration survives the dictionary the system stores it in")
    func configurationRoundTrip() {
        let configuration = ProviderConfiguration(bridgePort: 9_191)

        #expect(ProviderConfiguration(dictionary: configuration.dictionary) == configuration)
    }

    @Test("no configuration is the core's own default port")
    func defaultPort() {
        #expect(ProviderConfiguration(dictionary: nil)?.bridgePort == 9_091)
        #expect(ProviderConfiguration(dictionary: [:])?.bridgePort == 9_091)
    }

    @Test("a port that is not a port is refused, not replaced with the default")
    func badPort() {
        for value: Any in [0, 70_000, -1, "9091", 9_091.5] {
            #expect(ProviderConfiguration(dictionary: ["bridgePort": value]) == nil, "\(value)")
        }
    }

    @Test("a status request is one small JSON object")
    func statusRequest() {
        let encoded = ProviderMessage.status.encoded

        #expect(String(data: encoded, encoding: .utf8) == #"{"type":"status"}"#)
        #expect(ProviderMessage.decode(encoded) == .status)
        #expect(ProviderMessage.decode(Data("nonsense".utf8)) == nil)
        #expect(ProviderMessage.decode(Data(#"{"type":"reboot"}"#.utf8)) == nil)
    }

    @Test("a status round-trips, and carries counts and a sentence and nothing else")
    func statusRoundTrip() throws {
        let status = ProviderStatus(
            bridge: .unreachable,
            counters: .init(bridged: 4, passedThrough: 2, proxyLab: 1),
            lastBridgeFailure: "connection refused"
        )

        let decoded = try #require(ProviderStatus.decode(status.encoded))
        let fields = try #require(
            JSONSerialization.jsonObject(with: status.encoded) as? [String: Any]
        )

        #expect(decoded == status)
        #expect(Set(fields.keys) == ["bridge", "counters", "lastBridgeFailure"])
    }
}
