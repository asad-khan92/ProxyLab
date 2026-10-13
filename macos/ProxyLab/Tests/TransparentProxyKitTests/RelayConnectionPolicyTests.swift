//  A claimed flow must not be dropped just because a connection is waiting.
//
//  Enabling the provider installs network settings, and a new connection
//  waits while that route comes up. Treating that wait as failure claims the
//  flow and then ends it, which is the Mac losing the traffic the provider
//  took. The bridge being refused is the one wait that will not recover, and
//  that one is carried directly rather than ended.

import Testing

@testable import TransparentProxyKit

@Suite("What a claimed flow does with a connection")
struct RelayConnectionPolicyTests {
    @Test("a bridge that is ready is used, not abandoned")
    func bridgeReady() {
        #expect(RelayConnectionPolicy.bridge(.ready) == .proceed)
    }

    @Test("a bridge nothing is listening on is given up, so the flow can be carried directly")
    func bridgeRefused() {
        #expect(RelayConnectionPolicy.bridge(.waiting(.refused)) == .giveUp)
        #expect(RelayConnectionPolicy.bridge(.failed(.refused)) == .giveUp)
        #expect(RelayConnectionPolicy.bridge(.failed(.other)) == .giveUp)
    }

    @Test("a bridge that is only waiting for a route is left to become ready")
    func bridgeWaitingForARoute() {
        #expect(RelayConnectionPolicy.bridge(.waiting(.other)) == .wait)
        #expect(RelayConnectionPolicy.bridge(.other) == .wait)
    }

    @Test("a direct connection that is waiting is not a dropped flow")
    func directWaitingIsNotFailure() {
        #expect(RelayConnectionPolicy.direct(.waiting(.other)) == .wait)
        #expect(RelayConnectionPolicy.direct(.waiting(.refused)) == .wait)
        #expect(RelayConnectionPolicy.direct(.ready) == .proceed)
    }

    @Test("a direct connection that has failed ends the flow")
    func directFailed() {
        #expect(RelayConnectionPolicy.direct(.failed(.other)) == .giveUp)
        #expect(RelayConnectionPolicy.direct(.failed(.refused)) == .giveUp)
    }
}
