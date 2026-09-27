//  A JSON object or array as a collapsible tree.
//
//  One node per object member or array element, in the order they were sent.
//  Copying a value copies that node; copying formatted JSON is the whole
//  pretty-printed document, offered next to the tree.

import SwiftUI

/// One JSON document as a tree of disclosure rows.
struct JSONTreeView: View {
    let root: JSONNode
    var formatted: String?
    var copy: ((String, String) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let copy, let formatted {
                HStack(spacing: 10) {
                    Button("Copy JSON") {
                        copy(formatted, "the formatted JSON")
                    }
                    .buttonStyle(.borderless)
                    .font(.system(size: 10))
                    .help("Copy the formatted JSON")

                    Spacer(minLength: 0)
                }
            }

            JSONNodeRow(node: root, copy: copy)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 5))
        }
    }
}

private struct JSONNodeRow: View {
    let node: JSONNode
    var copy: ((String, String) -> Void)?

    var body: some View {
        if node.children.isEmpty {
            leaf
        } else {
            DisclosureGroup {
                ForEach(node.children) { child in
                    JSONNodeRow(node: child, copy: copy)
                }
            } label: {
                label
            }
            .contextMenu { copyMenu }
        }
    }

    private var leaf: some View {
        label
            .contextMenu { copyMenu }
    }

    private var label: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            if let key = node.key {
                Text(key)
                    .font(.system(size: 11, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
                Text(":")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.quaternary)
            }
            Text(node.preview)
                .font(.system(size: 11, design: .monospaced))
                .textSelection(.enabled)
        }
    }

    @ViewBuilder
    private var copyMenu: some View {
        if let copy {
            Button("Copy Value") { copy(node.copyableValue, "the JSON value") }
            Button("Copy JSON") { copy(node.serialized, "the JSON") }
        }
    }
}
