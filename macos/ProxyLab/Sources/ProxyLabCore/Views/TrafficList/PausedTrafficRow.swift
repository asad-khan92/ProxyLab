//  One exchange the core is holding, drawn above the captured rows.
//
//  It is not a row of the traffic query. The request being held has not been
//  published yet, so it cannot be selected out of the list. This row is that
//  hold, and choosing it opens the same editor the Breakpoints screen uses.

import SwiftUI

/// One held exchange in the traffic list.
struct PausedTrafficRow: View {
    let exchange: PausedExchange
    var selected: Bool

    var body: some View {
        let row = PausedTrafficRowContent(exchange)

        HStack(spacing: 8) {
            Image(systemName: "pause.fill")
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .frame(width: 14)

            VStack(alignment: .leading, spacing: 1) {
                Text(row.title)
                    .font(.system(size: 12, weight: .semibold))
                Text(row.detail)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 8)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(
            selected ? Color.primary.opacity(0.06) : Color.clear,
            in: RoundedRectangle(cornerRadius: 5)
        )
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(row.accessibilityLabel)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .help(row.help)
    }
}
