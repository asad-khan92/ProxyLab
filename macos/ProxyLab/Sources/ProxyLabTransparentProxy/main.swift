//  ProxyLab's transparent proxy provider: a Network Extension, run by macOS
//  as a system extension.
//
//      application ──▶ macOS ──▶ this process ──▶ 127.0.0.1:9091 ──▶ the core
//
//  This process holds no proxy logic. It does not terminate TLS, parse HTTP,
//  evaluate rules, or keep a record of anything: it decides which TCP flows
//  to take (`FlowPolicy`), hands each one to the core's bridge with one line
//  saying where it was going (`BridgeHandshake`), and copies bytes. Everything
//  a flow is made of is the core's, in the same code the explicit proxy uses.
//
//  It is started by `nesessionmanager`, never by a person, and it lives as
//  long as the configuration the app saved is connected. What class to
//  instantiate is named in this bundle's `Info.plist`, under
//  `NetworkExtension › NEProviderClasses`.

import Foundation
import NetworkExtension

autoreleasepool {
    NEProvider.startSystemExtensionMode()
}

dispatchMain()
