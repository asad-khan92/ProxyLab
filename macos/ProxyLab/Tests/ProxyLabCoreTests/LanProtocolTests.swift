//  The LAN half of the wire protocol, in both directions.
//
//  What a LAN request must carry, and what a LAN response must be read as. The
//  shapes here are the ones `core/ipc` parses and serializes: three operations,
//  one of which takes an address, and one status shape that all three answer
//  with.

import Foundation
import Testing

@testable import ProxyLabCore

@Suite("LAN request encoding")
struct LanRequestEncodingTests {
    private let encoder = JSONEncoder()

    private func encoded(_ operation: IPCOperation, id: String = "r1") throws -> [String: Any] {
        let data = try encoder.encode(IPCRequest(requestID: id, operation: operation))
        let object = try JSONSerialization.jsonObject(with: data)

        return try #require(object as? [String: Any])
    }

    @Test("the LAN operation names are the ones the server implements")
    func operationNames() {
        #expect(IPCOperation.lanStatus.name == "lan_status")
        #expect(IPCOperation.enableLan(address: "192.168.1.42").name == "enable_lan")
        #expect(IPCOperation.disableLan.name == "disable_lan")
    }

    @Test("asking for the status takes no parameters, so none are sent")
    func statusTakesNothing() throws {
        // The server refuses a parameter it does not know, and a key whose
        // value is null is still a key.
        #expect(try Set(encoded(.lanStatus).keys) == ["version", "request_id", "type"])
    }

    @Test("turning it off takes no parameters either")
    func disableTakesNothing() throws {
        #expect(try Set(encoded(.disableLan).keys) == ["version", "request_id", "type"])
    }

    @Test("turning it on sends the address and nothing else")
    func enableSendsTheAddress() throws {
        let json = try encoded(.enableLan(address: "192.168.1.42"))

        #expect(Set(json.keys) == ["version", "request_id", "type", "address"])
        #expect(json["address"] as? String == "192.168.1.42")
    }
}

@Suite("LAN response decoding")
struct LanResponseDecodingTests {
    private let decoder = JSONDecoder()

    private func message(_ json: String) throws -> IPCMessage {
        try decoder.decode(IPCMessage.self, from: Data(json.utf8))
    }

    private func status(_ message: IPCMessage) throws -> LanStatusResult {
        guard case let .response(response) = message, let result = response.result else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: [], debugDescription: "not a successful response")
            )
        }

        switch result {
        case let .lanStatus(status), let .enableLan(status), let .disableLan(status):
            return status
        default:
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: [], debugDescription: "not a LAN result")
            )
        }
    }

    @Test("a status names the port, the addresses, and that nothing is bound")
    func decodesADisabledStatus() throws {
        let status = try status(
            try message(
                """
                {"version":1,"request_id":"r1","ok":true,"type":"lan_status",
                 "result":{"enabled":false,"address":null,"port":9090,
                 "available":[{"interface":"en0","address":"192.168.1.42"}]}}
                """
            )
        )

        #expect(!status.enabled)
        #expect(status.address == nil)
        #expect(status.port == 9_090)
        #expect(status.available == [LanAddress(interface: "en0", address: "192.168.1.42")])
        #expect(status.endpoint == nil)
    }

    @Test("an enabled status carries the address and port a phone is given")
    func decodesAnEnabledStatus() throws {
        let status = try status(
            try message(
                """
                {"version":1,"request_id":"r1","ok":true,"type":"enable_lan",
                 "result":{"enabled":true,"address":"192.168.1.42","port":9090,
                 "available":[{"interface":"en0","address":"192.168.1.42"}]}}
                """
            )
        )

        #expect(status.enabled)
        #expect(status.endpoint == "192.168.1.42:9090")
    }

    @Test("each of the three operations is read as the operation it answers")
    func eachOperationIsNamed() throws {
        let result = """
            "result":{"enabled":false,"address":null,"port":9090,"available":[]}
            """

        guard
            case let .response(status) = try message(
                #"{"version":1,"request_id":"r1","ok":true,"type":"lan_status",\#(result)}"#
            ),
            case let .response(enabled) = try message(
                #"{"version":1,"request_id":"r1","ok":true,"type":"enable_lan",\#(result)}"#
            ),
            case let .response(disabled) = try message(
                #"{"version":1,"request_id":"r1","ok":true,"type":"disable_lan",\#(result)}"#
            )
        else {
            Issue.record("all three are responses")
            return
        }

        if case .lanStatus = enabled.result { Issue.record("enable_lan read as lan_status") }
        if case .lanStatus = disabled.result { Issue.record("disable_lan read as lan_status") }
        if case .lanStatus = status.result {} else { Issue.record("lan_status was not read") }
    }

    @Test("an address the core will not bind comes back as a code the app can match on")
    func decodesTheRefusal() throws {
        let message = try message(
            """
            {"version":1,"request_id":"r1","ok":false,
             "error":{"code":"lan_unavailable","message":"that address is not one of this \
            machine's usable local network addresses"}}
            """
        )

        guard case let .response(response) = message, let error = response.error else {
            Issue.record("a failed response")
            return
        }

        #expect(error.code == .lanUnavailable)
    }

    @Test("a status with no addresses is a status, not a failure")
    func decodesAnEmptyMachine() throws {
        let status = try status(
            try message(
                """
                {"version":1,"request_id":"r1","ok":true,"type":"lan_status",
                 "result":{"enabled":false,"address":null,"port":9090,"available":[]}}
                """
            )
        )

        #expect(status.available.isEmpty)
    }
}

@Suite("A LAN address")
struct LanAddressTests {
    @Test("is labelled by its interface and its address")
    func label() {
        #expect(
            LanAddress(interface: "en0", address: "192.168.1.42").label == "en0 — 192.168.1.42"
        )
    }

    @Test("is identified by the address, which is what is picked")
    func identity() {
        #expect(LanAddress(interface: "en0", address: "192.168.1.42").id == "192.168.1.42")
    }
}
