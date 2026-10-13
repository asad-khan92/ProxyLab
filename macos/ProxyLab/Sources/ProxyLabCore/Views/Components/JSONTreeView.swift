//  A JSON object or array as a collapsible tree.
//
//  One row per object member or array element, in the order they were sent.
//  A collapsed node draws its own row and does not build a view for anything
//  under it. The nodes themselves were parsed once, with the rest of the
//  reading, off the main actor.
//
//  Copy Value copies a string without its quotes. Copy Subtree JSON is that
//  node's compact JSON. Copy JSON is the formatted document, the same string
//  Pretty shows.

import SwiftUI

/// Which container rows are open. The root starts open; everything under it
/// stays closed until it is asked for.
@MainActor
@Observable
final class JSONTreeExpansion {
    private(set) var expanded: Set<String>

    init(showing rootID: String) {
        expanded = [rootID]
    }

    func contains(_ id: String) -> Bool {
        expanded.contains(id)
    }

    func toggle(_ id: String) {
        if expanded.contains(id) {
            expanded.remove(id)
        } else {
            expanded.insert(id)
        }
    }

    func expandAll(_ root: JSONNode) {
        expanded = JSONTreeIDs.containers(in: root)
    }

    func collapseAll() {
        expanded = []
    }
}

/// One JSON document as a tree of rows.
struct JSONTreeView: View {
    let root: JSONNode
    var formatted: String?
    var copy: ((String, String) -> Void)?
    @State private var expansion: JSONTreeExpansion

    init(root: JSONNode, formatted: String? = nil, copy: ((String, String) -> Void)? = nil) {
        self.root = root
        self.formatted = formatted
        self.copy = copy
        _expansion = State(initialValue: JSONTreeExpansion(showing: root.id))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !root.children.isEmpty {
                HStack(spacing: 12) {
                    Button("Expand All") { expansion.expandAll(root) }
                    Button("Collapse All") { expansion.collapseAll() }
                }
                .buttonStyle(.borderless)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            }

            ScrollView([.horizontal, .vertical]) {
                JSONTreeLevel(
                    nodes: [root],
                    depth: 0,
                    expansion: expansion,
                    formatted: formatted,
                    copy: copy
                )
                .padding(.horizontal, 6)
                .padding(.vertical, 4)
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .frame(height: viewportHeight)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 4))
        }
    }

    /// Tall enough for what is open, and no taller than a page of it.
    private var viewportHeight: CGFloat {
        let rows = visibleCount(root)
        return min(440, max(52, CGFloat(rows) * 22 + 12))
    }

    /// Rows that will actually be built: a closed node counts as one and its
    /// descendants are not visited.
    private func visibleCount(_ node: JSONNode) -> Int {
        guard !node.children.isEmpty, expansion.contains(node.id) else { return 1 }

        return 1 + node.children.reduce(0) { $0 + visibleCount($1) }
    }
}

/// One level of a tree. Its grid aligns the keys at this depth only.
private struct JSONTreeLevel: View {
    let nodes: [JSONNode]
    let depth: Int
    let expansion: JSONTreeExpansion
    let formatted: String?
    let copy: ((String, String) -> Void)?

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 2) {
            ForEach(Array(nodes.enumerated()), id: \.element.id) { index, node in
                GridRow {
                    label(node, index: index)
                        .contextMenu { menu(node, index: index) }
                    value(node, index: index)
                        .contextMenu { menu(node, index: index) }
                }

                // The next level is a view only while this node is open.
                if !node.children.isEmpty, expansion.contains(node.id) {
                    GridRow {
                        JSONTreeLevel(
                            nodes: node.children,
                            depth: depth + 1,
                            expansion: expansion,
                            formatted: formatted,
                            copy: copy
                        )
                        .padding(.leading, 14)
                        .gridCellColumns(2)
                    }
                }
            }
        }
    }

    private func label(_ node: JSONNode, index: Int) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            disclosure(node)

            if let name = name(of: node, index: index) {
                Text(name)
                    .font(
                        .system(
                            size: 11,
                            weight: node.key == nil ? .regular : .semibold,
                            design: .monospaced
                        )
                    )
                    .foregroundStyle(node.key == nil ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.primary))
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(name)
                    .textSelection(.enabled)
            } else {
                Text(containerToken(node))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder
    private func value(_ node: JSONNode, index: Int) -> some View {
        if node.kind == .object || node.kind == .array {
            // The root already shows its count where a key would be.
            if name(of: node, index: index) != nil {
                Text(containerToken(node))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }
        } else {
            Text(node.raw)
                .font(JSONTone.font(for: node))
                .foregroundStyle(JSONTone.color(for: node))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 520, alignment: .leading)
        }
    }

    @ViewBuilder
    private func disclosure(_ node: JSONNode) -> some View {
        if node.children.isEmpty {
            Color.clear.frame(width: 11, height: 11)
        } else {
            let open = expansion.contains(node.id)
            Button {
                expansion.toggle(node.id)
            } label: {
                Image(systemName: open ? "chevron.down" : "chevron.right")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(.secondary)
                    .frame(width: 11, height: 11)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel((open ? "Collapse " : "Expand ") + (node.key ?? "group"))
        }
    }

    /// An object member's key, an array element's index, or nothing at the root.
    private func name(of node: JSONNode, index: Int) -> String? {
        if let key = node.key { return key }
        if depth == 0 { return nil }

        return String(index)
    }

    private func containerToken(_ node: JSONNode) -> String {
        let empty = node.children.isEmpty
        switch node.kind {
        case .array: return empty ? "[]" : "[\(node.children.count)]"
        case .object: return empty ? "{}" : "{\(node.children.count)}"
        case .string, .number, .literal: return node.raw
        }
    }

    @ViewBuilder
    private func menu(_ node: JSONNode, index: Int) -> some View {
        if let copy {
            if let key = node.key {
                Button("Copy Key") { copy(key, "the JSON key") }
            } else if depth > 0 {
                Button("Copy Index") { copy(String(index), "the array index") }
            }
            Button("Copy Value") { copy(node.copyableValue, "the JSON value") }
            Button("Copy Subtree JSON") { copy(node.serialized, "the JSON subtree") }
            if let formatted {
                Button("Copy JSON") { copy(formatted, "the formatted JSON") }
            }
        }
    }
}
