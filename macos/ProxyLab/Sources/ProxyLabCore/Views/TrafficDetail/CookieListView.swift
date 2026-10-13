//  Cookies, as a list of names and values.
//
//  Request cookies are the pairs in `Cookie`. Response cookies are each
//  `Set-Cookie`, with the attributes under the cookie they belong to.
//  Nothing here is stored.

import SwiftUI

/// The cookies `presentation` parsed out of one message.
struct CookieListView: View {
    let presentation: CookiePresentation
    var emptyText: String
    var copy: ((String, String) -> Void)?

    var body: some View {
        if presentation.isEmpty {
            InspectorEmptyNote(text: emptyText)
        } else {
            AdaptiveKeyColumn {
                list(stacked: false)
            } narrow: {
                list(stacked: true)
            }
        }
    }

    private func list(stacked: Bool) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(presentation.cookies) { cookie in
                VStack(alignment: .leading, spacing: stacked ? 6 : 2) {
                    KeyValueRow(name: cookie.name, stacked: stacked) {
                        Text(cookie.value)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                    }
                    .contextMenu {
                        if let copy {
                            Button("Copy Name") { copy(cookie.name, "the cookie name") }
                            Button("Copy Value") { copy(cookie.value, "the cookie value") }
                            Button("Copy Cookie") { copy(cookie.parameterText, "the cookie") }
                        }
                    }

                    ForEach(cookie.attributes) { attribute in
                        KeyValueRow(name: attribute.name, stacked: stacked) {
                            Text(attribute.value)
                                .font(.system(size: 11, design: .monospaced))
                                .textSelection(.enabled)
                        }
                        .padding(.leading, stacked ? 0 : 12)
                        .contextMenu {
                            if let copy {
                                Button("Copy Name") { copy(attribute.name, "the attribute name") }
                                Button("Copy Value") { copy(attribute.value, "the attribute value") }
                            }
                        }
                    }
                }
            }
        }
    }
}
