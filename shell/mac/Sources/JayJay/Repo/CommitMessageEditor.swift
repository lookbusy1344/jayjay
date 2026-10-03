import AppKit
import SwiftUI

struct CommitMessageEditor: View {
    @Binding var summary: String
    @Binding var details: String
    var bodyHeight: CGFloat?
    var focusSummaryAtEnd = false
    var participatesInPaneNavigation = false
    @FocusState private var focusedField: Field?
    @State private var didSetInitialSelection = false

    private enum Field { case summary }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Summary", text: $summary)
                .textFieldStyle(.plain)
                // The pane-navigation modifier below owns the focus binding in the commit box; a second one would compete with it.
                .focused($focusedField, equals: focusSummaryAtEnd ? .summary : nil)
                .jayjayFont(13, design: .monospaced)
                .padding(.horizontal, 11)
                .padding(.vertical, 6)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(Color.primary.opacity(0.1), lineWidth: 1)
                )
                .accessibilityIdentifier(AID.CommitBox.summary)
                .keyboardFocusInput(.commitSummary, isAvailable: participatesInPaneNavigation)

            // TextEditor has no native placeholder; overlay one while empty.
            TextEditor(text: $details)
                .jayjayFont(13, design: .monospaced)
                .scrollContentBackground(.hidden)
                .accessibilityIdentifier(AID.CommitBox.draft)
                .keyboardFocusInput(.commitDescription, isAvailable: participatesInPaneNavigation)
                .overlay(alignment: .topLeading) {
                    if details.isEmpty {
                        Text("Description (optional)")
                            .jayjayFont(13, design: .monospaced)
                            .foregroundStyle(.tertiary)
                            // Match the TextEditor's text origin so the placeholder aligns with the cursor.
                            .padding(.leading, 5)
                            .padding(.top, 0)
                            .allowsHitTesting(false)
                            .accessibilityHidden(true)
                    }
                }
                .padding(6)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(Color.primary.opacity(0.1), lineWidth: 1)
                )
                .frame(minHeight: bodyHeight ?? 50, maxHeight: bodyHeight ?? 100)
        }
        .onChange(of: focusedField) { _, field in
            guard focusSummaryAtEnd, !didSetInitialSelection, field == .summary else { return }
            didSetInitialSelection = true
            // Set on the field editor once focus settles: SwiftUI follows its select-all with a hit-test caret, which beat a binding-set caret.
            DispatchQueue.main.async {
                guard let editor = NSApp.keyWindow?.firstResponder as? NSTextView,
                      editor.isFieldEditor, editor.string == summary
                else { return }
                let end = NSRange(location: (summary as NSString).length, length: 0)
                editor.setSelectedRange(end)
                editor.scrollRangeToVisible(end)
            }
        }
    }
}
